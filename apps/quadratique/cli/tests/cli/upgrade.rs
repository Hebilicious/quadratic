//! `quadratique upgrade` against a local fake of the GitHub release source. The binary under test
//! is a debug build, the only kind that reads QUADRATIQUE_UPGRADE_SOURCE; nothing here reaches
//! the network.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::support::{BINARY, Home, Quadratique, Repository, Run, git_stdout};

const CURRENT: &str = env!("CARGO_PKG_VERSION");
const LATEST: &str = "99.0.0";
const SKILL: &str = include_str!("../../../skill/quadratique/SKILL.md");
const SKILL_PATH: &str = ".agents/skills/quadratique/SKILL.md";

fn this_target() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64-unknown-linux-gnu",
        "aarch64" => "aarch64-unknown-linux-gnu",
        other => panic!("no release target for {other}"),
    }
}

#[derive(Clone)]
enum Reply {
    Body(Vec<u8>),
    Status(u16),
    Redirect(String),
    /// Announces more bytes than it sends, then closes the connection.
    Cut(Vec<u8>),
}

/// A release source on the loopback interface: it answers each path with its reply.
struct Releases {
    port: u16,
    replies: Arc<Mutex<BTreeMap<String, Reply>>>,
    stopped: Arc<AtomicBool>,
}

impl Releases {
    fn start() -> Releases {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let replies: Arc<Mutex<BTreeMap<String, Reply>>> = Arc::default();
        let stopped = Arc::new(AtomicBool::new(false));
        let (shared, stop) = (replies.clone(), stopped.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(stream) = stream {
                    let replies = shared.clone();
                    std::thread::spawn(move || answer(stream, &replies));
                }
            }
        });
        Releases {
            port,
            replies,
            stopped,
        }
    }

    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn set(&self, path: &str, reply: Reply) {
        self.replies.lock().unwrap().insert(path.to_owned(), reply);
    }

    /// Publishes a release with these assets, each served at /download/<name>.
    fn publish(&self, version: &str, assets: Vec<(String, Vec<u8>)>) {
        let described: Vec<Value> = assets
            .iter()
            .map(|(name, _)| {
                json!({
                    "name": name,
                    "browser_download_url": format!("{}/download/{name}", self.origin()),
                })
            })
            .collect();
        self.set(
            "/repos/Hebilicious/quadratique/releases/latest",
            Reply::Body(
                json!({ "tag_name": format!("v{version}"), "assets": described })
                    .to_string()
                    .into_bytes(),
            ),
        );
        for (name, bytes) in assets {
            self.set(&format!("/download/{name}"), Reply::Body(bytes));
        }
    }
}

impl Drop for Releases {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn answer(mut stream: TcpStream, replies: &Mutex<BTreeMap<String, Reply>>) {
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => request.extend_from_slice(&buffer[..read]),
        }
    }
    let text = String::from_utf8_lossy(&request);
    let path = text.split(' ').nth(1).unwrap_or("/").to_owned();
    let reply = replies
        .lock()
        .unwrap()
        .get(&path)
        .cloned()
        .unwrap_or(Reply::Status(404));
    let (head, body) = match reply {
        Reply::Body(body) => (
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n", body.len()),
            body,
        ),
        Reply::Status(code) => (
            format!("HTTP/1.1 {code} Status\r\nContent-Length: 0\r\n"),
            Vec::new(),
        ),
        Reply::Redirect(location) => (
            format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n"),
            Vec::new(),
        ),
        Reply::Cut(body) => (
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n", body.len() * 10),
            body,
        ),
    };
    let _ = stream.write_all(format!("{head}Connection: close\r\n\r\n").as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.shutdown(Shutdown::Both);
}

/// A tar.gz written header by header, so that it can hold entries a careful archiver refuses.
fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (path, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_cksum();
        builder.append(&header, *bytes).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn folder(version: &str) -> String {
    format!("quadratique-{version}-{}", this_target())
}

/// A stand-in for the binary of the release `version`: it answers --version with that version
/// and hands every other command to the binary under test.
fn new_binary(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo {version}; exit 0; fi\nexec {BINARY} \"$@\"\n")
        .into_bytes()
}

fn sha256_line(bytes: &[u8], name: &str) -> Vec<u8> {
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{digest}  {name}\n").into_bytes()
}

/// The assets of a release whose archive holds `binary`.
fn assets(version: &str, binary: &[u8]) -> Vec<(String, Vec<u8>)> {
    let name = format!("{}.tar.gz", folder(version));
    let bytes = archive(&[
        (&format!("{}/bin/quadratique", folder(version)), binary),
        (&format!("{}/install.sh", folder(version)), b"#!/bin/sh\n"),
    ]);
    vec![
        (format!("{name}.sha256"), sha256_line(&bytes, &name)),
        (name, bytes),
    ]
}

/// A binary installed as a file in a folder of its own, with a home for its server.
struct Installed {
    home: Home,
    _folder: TempDir,
    binary: PathBuf,
}

impl Installed {
    fn at(relative: &str) -> Installed {
        // Beside the binary under test, so that a hard link can stand for a copy: an upgrade
        // renames a new file over it and never writes into the linked file.
        let folder = tempfile::Builder::new()
            .prefix("quadratique-prefix-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let binary = folder.path().canonicalize().unwrap().join(relative);
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        if std::fs::hard_link(BINARY, &binary).is_err() {
            std::fs::copy(BINARY, &binary).unwrap();
        }
        Installed {
            home: Home::new(),
            _folder: folder,
            binary,
        }
    }

    fn file() -> Installed {
        Installed::at("bin/quadratique")
    }

    fn root(&self) -> PathBuf {
        self._folder.path().canonicalize().unwrap()
    }

    fn quadratique(&self, releases: &Releases) -> Quadratique {
        self.home
            .quadratique_at(&self.binary, self.home.path())
            .env("QUADRATIQUE_UPGRADE_SOURCE", &releases.origin())
    }

    fn upgrade(&self, releases: &Releases, args: &[&str]) -> Run {
        self.quadratique(releases)
            .args(["upgrade"])
            .args(args)
            .run()
    }

    fn bytes(&self) -> Vec<u8> {
        std::fs::read(&self.binary).unwrap()
    }

    /// The installed binary still runs and says the version of the build under test.
    fn still_runs(&self) {
        let output = std::process::Command::new(&self.binary)
            .arg("--version")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), CURRENT);
        let left: Vec<_> = std::fs::read_dir(self.binary.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name != "quadratique")
            .collect();
        assert!(left.is_empty(), "files left beside the binary: {left:?}");
    }
}

fn refused(run: &Run, words: &str) {
    assert_ne!(run.code, 0, "{run:?}");
    assert!(run.stderr.contains(words), "{run:?}");
    assert!(run.stderr.contains("Nothing was changed."), "{run:?}");
}

#[test]
fn upgrade_installs_the_latest_release_and_says_the_versions_it_left_and_reached() {
    let releases = Releases::start();
    let installed = Installed::file();
    let binary = new_binary(LATEST);
    releases.publish(LATEST, assets(LATEST, &binary));
    let archive = format!("/download/{}.tar.gz", folder(LATEST));
    let bytes = match releases.replies.lock().unwrap().get(&archive) {
        Some(Reply::Body(bytes)) => bytes.clone(),
        _ => unreachable!("the archive is published"),
    };
    releases.set("/objects/archive", Reply::Body(bytes));
    releases.set(&archive, Reply::Redirect("/objects/archive".to_owned()));

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["from"], CURRENT, "{answer}");
    assert_eq!(answer["to"], LATEST, "{answer}");
    assert_eq!(answer["checksum"], "verified", "{answer}");
    assert_eq!(answer["binary"], json!(installed.binary), "{answer}");
    assert!(answer["server"].is_string(), "{answer}");
    assert!(answer["skills"]["current"].is_array(), "{answer}");
    assert_eq!(installed.bytes(), binary);
    let version = std::process::Command::new(&installed.binary)
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&version.stdout).trim(), LATEST);
}

#[test]
fn upgrade_says_up_to_date_and_changes_nothing_when_the_latest_release_is_this_build() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(CURRENT, assets(CURRENT, b"never installed"));
    let before = installed.bytes();

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["upToDate"], true, "{answer}");
    assert_eq!(answer["installed"], CURRENT, "{answer}");
    assert_eq!(installed.bytes(), before);
    installed.still_runs();
}

#[test]
fn upgrade_does_not_downgrade_unless_asked() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish("0.0.1", assets("0.0.1", &new_binary("0.0.1")));

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["upToDate"], true, "{answer}");
    assert_eq!(answer["latest"], "0.0.1", "{answer}");
    assert!(
        answer["note"]
            .as_str()
            .unwrap()
            .contains("--allow-downgrade")
    );
    installed.still_runs();

    let answer = installed.upgrade(&releases, &["--allow-downgrade"]).ok();
    assert_eq!(answer["to"], "0.0.1", "{answer}");
}

#[test]
fn check_says_the_installed_and_the_latest_version_and_changes_nothing() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));

    let answer = installed.upgrade(&releases, &["--check"]).ok();

    assert_eq!(answer["upToDate"], false, "{answer}");
    assert_eq!(answer["installed"], CURRENT, "{answer}");
    assert_eq!(answer["latest"], LATEST, "{answer}");
    assert_eq!(answer["run"], "quadratique upgrade", "{answer}");
    assert!(answer["skills"].is_object(), "{answer}");
    installed.still_runs();
}

#[test]
fn a_failed_download_leaves_the_old_binary_in_place_and_working() {
    let releases = Releases::start();
    let installed = Installed::file();
    let archive = format!("{}.tar.gz", folder(LATEST));
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));

    releases.set(&format!("/download/{archive}"), Reply::Status(500));
    refused(&installed.upgrade(&releases, &[]), "answered 500");
    installed.still_runs();

    releases.set(
        &format!("/download/{archive}"),
        Reply::Cut(b"the start of an archive".to_vec()),
    );
    refused(&installed.upgrade(&releases, &[]), "The download of");
    installed.still_runs();
}

#[test]
fn an_archive_that_does_not_match_its_checksum_is_not_installed() {
    let releases = Releases::start();
    let installed = Installed::file();
    let archive = format!("{}.tar.gz", folder(LATEST));
    let mut published = assets(LATEST, &new_binary(LATEST));
    for (name, bytes) in &mut published {
        if *name == archive {
            bytes[20] ^= 0xff;
        }
    }
    releases.publish(LATEST, published);

    refused(
        &installed.upgrade(&releases, &[]),
        "does not match its checksum",
    );
    installed.still_runs();
}

#[test]
fn a_release_without_an_archive_for_this_platform_changes_nothing() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(
        LATEST,
        vec![(
            format!("quadratique-{LATEST}-riscv64gc-unknown-linux-gnu.tar.gz"),
            b"other".to_vec(),
        )],
    );

    refused(&installed.upgrade(&releases, &[]), "for this platform");
    installed.still_runs();
}

#[test]
fn an_archive_with_an_entry_outside_its_folder_is_refused() {
    let releases = Releases::start();
    let installed = Installed::file();
    let name = format!("{}.tar.gz", folder(LATEST));
    let binary = new_binary(LATEST);
    for outside in ["/etc/quadratique-planted", "../quadratique-planted"] {
        let bytes = archive(&[
            (outside, b"planted"),
            (&format!("{}/bin/quadratique", folder(LATEST)), &binary),
        ]);
        releases.publish(
            LATEST,
            vec![
                (format!("{name}.sha256"), sha256_line(&bytes, &name)),
                (name.clone(), bytes),
            ],
        );

        refused(&installed.upgrade(&releases, &[]), "is refused");
        installed.still_runs();
    }
}

#[test]
fn a_new_binary_that_does_not_run_or_says_another_version_is_not_installed() {
    let releases = Releases::start();
    let installed = Installed::file();
    for (binary, words) in [
        (b"\x7fELF not a program".to_vec(), "does not start"),
        (b"#!/bin/sh\nexit 3\n".to_vec(), "--version failed"),
        (new_binary("98.0.0"), "not 99.0.0"),
    ] {
        releases.publish(LATEST, assets(LATEST, &binary));
        refused(&installed.upgrade(&releases, &[]), words);
        installed.still_runs();
    }
}

#[test]
fn a_redirect_to_plain_http_elsewhere_is_refused() {
    let releases = Releases::start();
    let installed = Installed::file();
    let archive = format!("{}.tar.gz", folder(LATEST));
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    releases.set(
        &format!("/download/{archive}"),
        Reply::Redirect(format!("http://127.0.0.1:1/{archive}")),
    );

    refused(&installed.upgrade(&releases, &[]), "is not HTTPS");
    installed.still_runs();
}

#[test]
fn a_binary_that_mise_installed_is_left_to_mise() {
    let releases = Releases::start();
    let installed = Installed::at(&format!(
        "mise/installs/github-hebilicious-quadratique/{CURRENT}/{}/bin/quadratique",
        folder(CURRENT)
    ));
    std::fs::write(
        installed
            .root()
            .join("mise/installs/github-hebilicious-quadratique/.mise.backend.toml"),
        "short = \"github:Hebilicious/quadratique\"\nfull = \"github:Hebilicious/quadratique\"\nexplicit_backend = true\n",
    )
    .unwrap();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let before = installed.bytes();

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["managedBy"], "mise", "{answer}");
    assert_eq!(
        answer["run"], "mise upgrade --bump github:Hebilicious/quadratique",
        "{answer}"
    );
    assert_eq!(
        answer["then"],
        json!(["quadratique upgrade --skills"]),
        "{answer}"
    );
    assert_eq!(installed.bytes(), before);
}

#[test]
fn a_binary_that_cargo_installed_is_left_to_cargo() {
    let releases = Releases::start();
    let installed = Installed::at("cargo/bin/quadratique");
    std::fs::write(
        installed.root().join("cargo/.crates2.json"),
        json!({ "installs": { format!("quadratique {CURRENT} (registry+https://github.com/rust-lang/crates.io-index)"): { "bins": ["quadratique"] } } }).to_string(),
    )
    .unwrap();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let before = installed.bytes();

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["managedBy"], "cargo", "{answer}");
    assert_eq!(answer["run"], "cargo install quadratique", "{answer}");
    assert_eq!(installed.bytes(), before);
}

#[test]
fn the_user_service_that_runs_the_binary_is_restarted() {
    let releases = Releases::start();
    let installed = Installed::file();
    let units = installed.home.path().join(".config/systemd/user");
    std::fs::create_dir_all(&units).unwrap();
    std::fs::write(
        units.join("quadratique.service"),
        format!(
            "[Service]\nExecStart=\"{}\" serve --take-over\n",
            installed.binary.display()
        ),
    )
    .unwrap();
    let tools = installed.root().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let calls = installed.root().join("systemctl-calls");
    let systemctl = tools.join("systemctl");
    std::fs::write(
        &systemctl,
        format!("#!/bin/sh\necho \"$*\" >> {}\n", calls.display()),
    )
    .unwrap();
    std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755)).unwrap();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let path = format!(
        "{}:{}",
        tools.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let answer = installed
        .quadratique(&releases)
        .env("PATH", &path)
        .args(["upgrade"])
        .run()
        .ok();

    assert_eq!(
        answer["server"], "restarted quadratique.service",
        "{answer}"
    );
    assert_eq!(
        std::fs::read_to_string(&calls).unwrap(),
        "--user restart quadratique.service\n"
    );
}

fn write(root: &Path, path: &str, text: &str) {
    std::fs::write(root.join(path), text).unwrap();
}

#[test]
fn the_report_names_each_older_skill_with_its_command_and_each_changed_file_with_its_warning() {
    let releases = Releases::start();
    let installed = Installed::file();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skill");
    let earlier = std::fs::read_to_string(fixtures.join("quadratique-acbb941/SKILL.md")).unwrap();
    let repositories: Vec<Repository> = (0..3).map(|_| Repository::new()).collect();
    for (repository, key) in repositories.iter().zip(["OLD", "NEW", "MINE"]) {
        installed
            .home
            .quadratique_at(&installed.binary, &repository.root)
            .args(["init", "--key", key])
            .run()
            .ok();
    }
    let [older, current, changed] = [0, 1, 2].map(|index| repositories[index].root.clone());
    write(&older, SKILL_PATH, &earlier);
    write(&changed, SKILL_PATH, &format!("{SKILL}\nOur own rule.\n"));
    let before: Vec<String> = [&older, &current, &changed]
        .iter()
        .map(|root| git_stdout(root, &["status", "--porcelain"]))
        .collect();

    let answer = installed.upgrade(&releases, &["--skills"]).ok();

    let report = &answer["skills"];
    let shown = |root: &PathBuf| root.display().to_string();
    assert_eq!(
        report["older"],
        json!([{ "repository": shown(&older), "run": format!("cd {} && quadratique init", shown(&older)) }]),
        "{report}"
    );
    assert_eq!(report["current"], json!([shown(&current)]), "{report}");
    assert_eq!(report["kept"][0]["repository"], shown(&changed), "{report}");
    let warning = report["kept"][0]["warnings"][0].as_str().unwrap();
    assert!(warning.contains(SKILL_PATH), "{warning}");
    assert!(
        warning.contains("quadratique init --print SKILL.md"),
        "{warning}"
    );
    assert!(
        report["note"]
            .as_str()
            .unwrap()
            .contains("Nothing was written")
    );
    let after: Vec<String> = [&older, &current, &changed]
        .iter()
        .map(|root| git_stdout(root, &["status", "--porcelain"]))
        .collect();
    assert_eq!(before, after);
    assert_eq!(
        std::fs::read_to_string(older.join(SKILL_PATH)).unwrap(),
        earlier
    );
}
