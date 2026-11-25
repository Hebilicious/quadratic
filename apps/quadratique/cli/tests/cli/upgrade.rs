//! `quadratique upgrade` against a local fake of the GitHub release source. The binary under test
//! is a debug build, the only kind that reads QUADRATIQUE_UPGRADE_SOURCE; nothing here reaches
//! the network. Every run finds a fake `systemctl` first on its PATH, so no test reads or
//! changes this machine's user services.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::support::{BINARY, Home, Quadratique, Repository, Run, await_exit, git_stdout};

const CURRENT: &str = env!("CARGO_PKG_VERSION");
const LATEST: &str = "99.0.0";
const SKILL: &str = include_str!("../../../skill/quadratique/SKILL.md");
const SKILL_PATH: &str = ".agents/skills/quadratique/SKILL.md";
const PLANNING_PATH: &str = ".agents/skills/quadratique/planning.md";
const LOCK: &str = ".quadratique-upgrade.lock";
const NO_SERVICE: &str = "#!/bin/sh\necho LoadState=not-found\necho ActiveState=inactive\necho MainPID=0\necho NeedDaemonReload=no\n";

fn this_target() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64-unknown-linux-gnu",
        "aarch64" => "aarch64-unknown-linux-gnu",
        other => panic!("no release target for {other}"),
    }
}

type Hook = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
enum Reply {
    Body(Vec<u8>),
    Status(u16),
    Redirect(String),
    /// Announces more bytes than it sends, then closes the connection.
    Cut(Vec<u8>),
    /// Runs the hook, then answers.
    After(Hook, Box<Reply>),
}

#[derive(Clone, Copy, PartialEq)]
enum Digest256 {
    Right,
    Wrong,
    Absent,
}

/// A release source on the loopback interface: it answers each path with its reply.
struct Releases {
    port: u16,
    replies: Arc<Mutex<BTreeMap<String, Reply>>>,
    stopped: Arc<AtomicBool>,
}

const LATEST_PATH: &str = "/repos/Hebilicious/quadratique/releases/latest";

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

    fn get(&self, path: &str) -> Reply {
        self.replies.lock().unwrap().get(path).cloned().unwrap()
    }

    /// Publishes a release with these assets, each served at /download/<name>, and described
    /// with its digest as `digest` says.
    fn publish_with(&self, version: &str, assets: Vec<(String, Vec<u8>)>, digest: Digest256) {
        let described: Vec<Value> = assets
            .iter()
            .map(|(name, bytes)| {
                let mut asset = json!({
                    "name": name,
                    "browser_download_url": format!("{}/download/{name}", self.origin()),
                });
                match digest {
                    Digest256::Right => {
                        asset["digest"] = json!(format!("sha256:{}", sha256(bytes)))
                    }
                    Digest256::Wrong => {
                        asset["digest"] = json!(format!("sha256:{}", "0".repeat(64)))
                    }
                    Digest256::Absent => {}
                }
                asset
            })
            .collect();
        self.set(
            LATEST_PATH,
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

    fn publish(&self, version: &str, assets: Vec<(String, Vec<u8>)>) {
        self.publish_with(version, assets, Digest256::Right);
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
    let mut reply = replies
        .lock()
        .unwrap()
        .get(&path)
        .cloned()
        .unwrap_or(Reply::Status(404));
    while let Reply::After(hook, then) = reply {
        hook();
        reply = *then;
    }
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
        Reply::After(..) => unreachable!("hooks were run"),
    };
    let _ = stream.write_all(format!("{head}Connection: close\r\n\r\n").as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.shutdown(Shutdown::Both);
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

/// A stand-in for the binary of the release `version`. Its `--version` answers only in an empty
/// environment and from `/`, and its skill report names its version, so a test can tell which
/// build made it.
fn new_binary(version: &str) -> Vec<u8> {
    format!(
        "#!/bin/sh\ncase \"$1 $2\" in\n--version*)\n  if [ -n \"$HOME$QUADRATIQUE_HOME$QUADRATIQUE_UPGRADE_SOURCE\" ] || [ \"$(pwd)\" != / ]; then echo 'not an empty environment' >&2; exit 9; fi\n  echo {version}; exit 0 ;;\n'upgrade --skills') echo '{{\"skills\":{{\"madeBy\":\"{version}\"}}}}'; exit 0 ;;\nesac\nexit 2\n"
    )
    .into_bytes()
}

fn archive_name(version: &str) -> String {
    format!("{}.tar.gz", folder(version))
}

/// The assets of a release whose archive holds `binary`.
fn assets(version: &str, binary: &[u8]) -> Vec<(String, Vec<u8>)> {
    let bytes = archive(&[
        (&format!("{}/bin/quadratique", folder(version)), binary),
        (&format!("{}/install.sh", folder(version)), b"#!/bin/sh\n"),
    ]);
    vec![(archive_name(version), bytes)]
}

/// At most four installed copies of the binary under test exist at once: where the temporary
/// folder is on another file system than the build, each is a full copy.
struct Gate {
    taken: Mutex<usize>,
    freed: Condvar,
}

static GATE: Gate = Gate {
    taken: Mutex::new(0),
    freed: Condvar::new(),
};

struct Pass;

impl Pass {
    fn take() -> Pass {
        let mut taken = GATE.taken.lock().unwrap();
        while *taken >= 4 {
            taken = GATE.freed.wait(taken).unwrap();
        }
        *taken += 1;
        Pass
    }
}

impl Drop for Pass {
    fn drop(&mut self) {
        *GATE.taken.lock().unwrap() -= 1;
        GATE.freed.notify_one();
    }
}

/// Writes an executable file through a child process. A file that this test process held open
/// for writing could be inherited by a process that another test forks at that moment, and an
/// exec of the file would then fail with "Text file busy".
fn write_executable(path: &Path, from: Option<&Path>, text: &str) {
    let mut command = std::process::Command::new("sh");
    match from {
        Some(source) => command
            .args(["-c", "cp -- \"$1\" \"$0\""])
            .arg(path)
            .arg(source),
        None => command.args(["-c", "cat > \"$0\""]).arg(path),
    };
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("sh runs");
    let mut stdin = child.stdin.take().expect("a piped standard input");
    stdin
        .write_all(text.as_bytes())
        .expect("the text is written");
    drop(stdin);
    assert!(child.wait().expect("sh ends").success());
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A copy of the binary under test: a new file, written by a child process.
fn copy_binary(path: &Path) {
    write_executable(path, Some(Path::new(BINARY)), "");
}

/// A copy of the binary under test at `path`: a hard link where the file system allows it. An
/// upgrade renames a new file over it and never writes into the linked file.
fn place_binary(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    if std::fs::hard_link(BINARY, path).is_err() {
        copy_binary(path);
    }
}

/// A binary installed as a file in a folder of its own, outside any git working tree, with a
/// home for its server and a fake `systemctl`.
struct Installed {
    home: Home,
    folder: TempDir,
    binary: PathBuf,
    tools: PathBuf,
    _pass: Pass,
}

impl Installed {
    fn at(relative: &str) -> Installed {
        let pass = Pass::take();
        let folder = tempfile::Builder::new()
            .prefix("quadratique-prefix-")
            .tempdir()
            .unwrap();
        let root = folder.path().canonicalize().unwrap();
        let binary = root.join(relative);
        place_binary(&binary);
        let tools = root.join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        let installed = Installed {
            home: Home::new(),
            folder,
            binary,
            tools,
            _pass: pass,
        };
        installed.systemctl(NO_SERVICE);
        installed
    }

    fn file() -> Installed {
        Installed::at("bin/quadratique")
    }

    fn root(&self) -> PathBuf {
        self.folder.path().canonicalize().unwrap()
    }

    fn systemctl(&self, script: &str) {
        let staged = self.tools.join("systemctl.new");
        write_executable(&staged, None, script);
        std::fs::rename(&staged, self.tools.join("systemctl")).unwrap();
    }

    fn quadratique(&self, releases: &Releases) -> Quadratique {
        let path = format!(
            "{}:{}",
            self.tools.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        self.home
            .quadratique_at(&self.binary, self.home.path())
            .env("PATH", &path)
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

    fn beside(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.binary.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "quadratique" && name != LOCK)
            .collect();
        names.sort();
        names
    }

    /// The installed binary still runs and says the version of the build under test, and the
    /// upgrade left nothing beside it but its lock file.
    fn still_runs(&self) {
        let output = std::process::Command::new(&self.binary)
            .arg("--version")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), CURRENT);
        assert!(
            self.beside().is_empty(),
            "left beside the binary: {:?}",
            self.beside()
        );
    }

    /// Starts the home's server from the installed file, then replaces the file by another copy,
    /// so that the server runs an earlier build of it. Returns the server's pid.
    fn server_of_an_earlier_build(&self, releases: &Releases) -> u32 {
        self.quadratique(releases)
            .args(["project", "list"])
            .run()
            .ok();
        let servers = self.home.servers();
        assert_eq!(servers.len(), 1, "{servers:?}");
        let copy = self.binary.with_file_name("copy");
        copy_binary(&copy);
        std::fs::rename(&copy, &self.binary).unwrap();
        servers[0]
    }

    fn store(&self) -> PathBuf {
        self.home.path().join("workspace.db")
    }
}

fn refused(run: &Run, words: &str) {
    assert_ne!(run.code, 0, "{run:?}");
    assert!(run.stderr.contains(words), "{run:?}");
    assert!(run.stderr.contains("Nothing was changed."), "{run:?}");
}

#[test]
fn upgrade_installs_the_latest_release_verified_by_the_api_s_digest_and_the_new_build_reports() {
    let releases = Releases::start();
    let installed = Installed::file();
    let binary = new_binary(LATEST);
    releases.publish(LATEST, assets(LATEST, &binary));
    let archive = format!("/download/{}", archive_name(LATEST));
    releases.set("/objects/archive", releases.get(&archive));
    releases.set(&archive, Reply::Redirect("/objects/archive".to_owned()));

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["from"], CURRENT, "{answer}");
    assert_eq!(answer["to"], LATEST, "{answer}");
    assert_eq!(
        answer["sha256"], "verified against the GitHub API",
        "{answer}"
    );
    assert_eq!(answer["binary"], json!(installed.binary), "{answer}");
    assert_eq!(
        answer["server"],
        "no server runs for this home; the next quadratique command starts this build",
        "{answer}"
    );
    assert_eq!(answer["skills"], json!({ "madeBy": LATEST }), "{answer}");
    assert_eq!(installed.bytes(), binary);
    assert!(installed.beside().is_empty(), "{:?}", installed.beside());
    assert!(installed.home.servers().is_empty());
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
    assert!(installed.home.servers().is_empty());
    assert!(!installed.store().exists());
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
fn check_and_skills_change_nothing_and_start_no_server() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));

    let answer = installed.upgrade(&releases, &["--check"]).ok();

    assert_eq!(answer["upToDate"], false, "{answer}");
    assert_eq!(answer["installed"], CURRENT, "{answer}");
    assert_eq!(answer["latest"], LATEST, "{answer}");
    assert_eq!(answer["run"], "quadratique upgrade", "{answer}");
    assert_eq!(
        answer["skills"],
        json!({ "current": [], "older": [] }),
        "{answer}"
    );
    let answer = installed.upgrade(&releases, &["--skills"]).ok();
    assert_eq!(
        answer["skills"],
        json!({ "current": [], "older": [] }),
        "{answer}"
    );
    installed.still_runs();
    assert!(installed.home.servers().is_empty());
    assert!(!installed.store().exists());
    assert!(!installed.home.path().join("server.json").exists());
}

#[test]
fn a_failed_download_leaves_the_old_binary_in_place_and_working() {
    let releases = Releases::start();
    let installed = Installed::file();
    let archive = format!("/download/{}", archive_name(LATEST));
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));

    releases.set(&archive, Reply::Status(500));
    refused(&installed.upgrade(&releases, &[]), "answered 500");
    installed.still_runs();

    releases.set(&archive, Reply::Cut(b"the start of an archive".to_vec()));
    refused(&installed.upgrade(&releases, &[]), "The download of");
    installed.still_runs();
}

#[test]
fn an_archive_is_installed_only_with_the_digest_that_the_api_gives_for_it() {
    let releases = Releases::start();
    let installed = Installed::file();
    let published = assets(LATEST, &new_binary(LATEST));

    releases.publish_with(LATEST, published.clone(), Digest256::Wrong);
    refused(
        &installed.upgrade(&releases, &[]),
        "does not match the SHA-256 that the GitHub API gives for it",
    );
    installed.still_runs();

    releases.publish_with(LATEST, published, Digest256::Absent);
    refused(
        &installed.upgrade(&releases, &[]),
        "gives no SHA-256 digest",
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
    let binary = new_binary(LATEST);
    for outside in ["/etc/quadratique-planted", "../quadratique-planted"] {
        let bytes = archive(&[
            (outside, b"planted"),
            (&format!("{}/bin/quadratique", folder(LATEST)), &binary),
        ]);
        releases.publish(LATEST, vec![(archive_name(LATEST), bytes)]);

        refused(&installed.upgrade(&releases, &[]), "is refused");
        installed.still_runs();
    }
}

#[test]
fn a_new_binary_that_does_not_run_says_another_version_or_too_much_is_not_installed() {
    let releases = Releases::start();
    let installed = Installed::file();
    for (binary, words) in [
        (b"\x7fELF not a program".to_vec(), "does not start"),
        (b"#!/bin/sh\nexit 3\n".to_vec(), "--version failed"),
        (new_binary("98.0.0"), "not 99.0.0"),
        (
            b"#!/bin/sh\nwhile :; do echo 99.0.0; done\n".to_vec(),
            "printed more than 4096 bytes",
        ),
    ] {
        releases.publish(LATEST, assets(LATEST, &binary));
        refused(&installed.upgrade(&releases, &[]), words);
        installed.still_runs();
    }
}

#[test]
fn a_download_reaches_only_the_release_source() {
    let releases = Releases::start();
    let installed = Installed::file();
    let archive = format!("/download/{}", archive_name(LATEST));
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));

    releases.set(
        &archive,
        Reply::Redirect("http://127.0.0.1:1/elsewhere".to_owned()),
    );
    refused(
        &installed.upgrade(&releases, &[]),
        "this request may reach only",
    );
    installed.still_runs();

    let described = json!({ "tag_name": format!("v{LATEST}"), "assets": [{
        "name": archive_name(LATEST),
        "browser_download_url": "http://127.0.0.1:1/download/archive",
        "digest": format!("sha256:{}", "0".repeat(64)),
    }]});
    releases.set(LATEST_PATH, Reply::Body(described.to_string().into_bytes()));
    refused(
        &installed.upgrade(&releases, &[]),
        "is not a release download",
    );
    installed.still_runs();
}

#[test]
fn a_binary_that_mise_installed_is_left_to_mise_and_an_unclear_marker_to_the_person() {
    let releases = Releases::start();
    let tool = "mise/installs/github-hebilicious-quadratique";
    let installed = Installed::at(&format!(
        "{tool}/{CURRENT}/{}/bin/quadratique",
        folder(CURRENT)
    ));
    let marker = installed.root().join(tool).join(".mise.backend.toml");
    std::fs::write(
        &marker,
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

    std::fs::write(
        &marker,
        "short = \"github:someone/else\"\nfull = \"github:someone/else\"\n",
    )
    .unwrap();
    refused(
        &installed.upgrade(&releases, &[]),
        "not this repository's tool",
    );
    assert_eq!(installed.bytes(), before);
}

#[test]
fn a_binary_that_cargo_installed_is_left_to_cargo_with_its_root() {
    let releases = Releases::start();
    let installed = Installed::at("my cargo/bin/quadratique");
    let root = installed.root().join("my cargo");
    std::fs::write(
        root.join(".crates2.json"),
        json!({ "installs": { format!("quadratique {CURRENT} (registry+https://github.com/rust-lang/crates.io-index)"): { "bins": ["quadratique"] } } }).to_string(),
    )
    .unwrap();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let before = installed.bytes();

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["managedBy"], "cargo", "{answer}");
    assert_eq!(
        answer["run"],
        format!("cargo install --root '{}' quadratique", root.display()),
        "{answer}"
    );
    assert_eq!(installed.bytes(), before);
}

#[test]
fn an_install_folder_that_other_accounts_can_write_is_refused() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let bin = installed.binary.parent().unwrap().to_owned();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o757)).unwrap();

    refused(
        &installed.upgrade(&releases, &[]),
        "other accounts can write",
    );
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    installed.still_runs();
}

#[test]
fn a_binary_inside_a_git_working_tree_is_not_replaced() {
    let releases = Releases::start();
    let repository = Repository::new();
    let installed = Installed::file();
    let binary = repository.root.join("target/debug/quadratique");
    place_binary(&binary);
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));

    let run = installed
        .home
        .quadratique_at(&binary, installed.home.path())
        .env(
            "PATH",
            &format!(
                "{}:{}",
                installed.tools.display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("QUADRATIQUE_UPGRADE_SOURCE", &releases.origin())
        .args(["upgrade"])
        .run();

    refused(&run, "is inside a git working tree");
    assert_eq!(
        std::fs::read(&binary).unwrap(),
        std::fs::read(BINARY).unwrap()
    );
}

#[test]
fn a_home_inside_a_git_working_tree_is_refused_before_the_binary_changes() {
    let releases = Releases::start();
    let installed = Installed::file();
    let repository = Repository::new();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));

    let run = installed
        .quadratique(&releases)
        .env(
            "QUADRATIQUE_HOME",
            &repository.root.join("home").display().to_string(),
        )
        .args(["upgrade"])
        .run();

    refused(&run, "inside the git working tree");
    installed.still_runs();
}

#[test]
fn a_second_upgrade_of_the_same_folder_waits_for_none_and_changes_nothing() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(installed.binary.with_file_name(LOCK))
        .unwrap();
    lock.lock().unwrap();

    refused(
        &installed.upgrade(&releases, &[]),
        "another quadratique upgrade",
    );
    installed.still_runs();
}

#[test]
fn a_binary_replaced_while_the_upgrade_ran_is_not_replaced_again() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let binary = installed.binary.clone();
    let described = releases.get(LATEST_PATH);
    releases.set(
        LATEST_PATH,
        Reply::After(
            Arc::new(move || {
                let copy = binary.with_file_name("copy");
                copy_binary(&copy);
                std::fs::rename(&copy, &binary).unwrap();
            }),
            Box::new(described),
        ),
    );

    refused(
        &installed.upgrade(&releases, &[]),
        "changed while this upgrade ran",
    );
    installed.still_runs();
}

#[test]
fn a_staging_folder_left_by_a_killed_upgrade_is_removed_by_the_next() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let leftover = installed
        .binary
        .with_file_name(".quadratique-upgrade-0123abcd");
    std::fs::create_dir(&leftover).unwrap();
    std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(leftover.join("quadratique"), "half written").unwrap();

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["removedLeftovers"], json!([leftover]), "{answer}");
    assert!(installed.beside().is_empty(), "{:?}", installed.beside());
}

/// A fake `systemctl` that runs `quadratique.service` as systemd would: its main process is the
/// pid in `<state>/pid`, and a restart starts the installed binary's server with take-over.
fn service(state: &Path, binary: &Path, needs_reload: bool) -> String {
    format!(
        "#!/bin/sh\nstate={state}\necho \"$*\" >> \"$state/calls\"\ncase \"$1 $2\" in\n'--user show') echo LoadState=loaded; echo ActiveState=active; echo \"MainPID=$(cat \"$state/pid\")\"; echo NeedDaemonReload={reload} ;;\n'--user restart') nohup {binary} serve --take-over > \"$state/log\" 2>&1 & echo $! > \"$state/pid\" ;;\nesac\n",
        state = state.display(),
        binary = binary.display(),
        reload = if needs_reload { "yes" } else { "no" },
    )
}

#[test]
fn a_server_left_on_the_earlier_build_is_restarted_through_the_service_until_the_new_build_serves()
{
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(CURRENT, assets(CURRENT, b"never installed"));
    let earlier = installed.server_of_an_earlier_build(&releases);
    let state = installed.root().join("systemd");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("pid"), earlier.to_string()).unwrap();
    installed.systemctl(&service(&state, &installed.binary, false));

    let answer = installed.upgrade(&releases, &[]).ok();

    let now: u32 = std::fs::read_to_string(state.join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_ne!(now, earlier);
    assert_eq!(
        answer["server"],
        format!("restarted quadratique.service; {CURRENT} serves this home (pid {now})"),
        "{answer}"
    );
    await_exit(earlier);
    assert_eq!(
        std::fs::canonicalize(format!("/proc/{now}/exe")).unwrap(),
        installed.binary
    );
}

#[test]
fn a_service_whose_unit_changed_on_disk_is_not_restarted() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(CURRENT, assets(CURRENT, b"never installed"));
    let earlier = installed.server_of_an_earlier_build(&releases);
    let state = installed.root().join("systemd");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("pid"), earlier.to_string()).unwrap();
    installed.systemctl(&service(&state, &installed.binary, true));

    let answer = installed.upgrade(&releases, &[]).ok();

    let warning = answer["warnings"][0].as_str().unwrap();
    assert!(
        warning.contains("systemctl --user daemon-reload"),
        "{warning}"
    );
    assert!(
        !std::fs::read_to_string(state.join("calls"))
            .unwrap()
            .contains("restart")
    );
    assert_eq!(installed.home.servers(), vec![earlier]);
}

#[test]
fn without_a_service_a_server_left_on_the_earlier_build_is_stopped() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(CURRENT, assets(CURRENT, b"never installed"));
    let earlier = installed.server_of_an_earlier_build(&releases);

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(
        answer["server"],
        format!(
            "stopped the server of the earlier build (pid {earlier}); the next quadratique command starts this build"
        ),
        "{answer}"
    );
    assert!(installed.home.servers().is_empty());
}

fn write(root: &Path, path: &str, text: &str) {
    std::fs::write(root.join(path), text).unwrap();
}

#[test]
fn the_report_names_each_repository_s_state_and_its_errors_and_starts_no_server() {
    let releases = Releases::start();
    let installed = Installed::file();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skill");
    let earlier = std::fs::read_to_string(fixtures.join("quadratique-acbb941/SKILL.md")).unwrap();
    let repositories: Vec<Repository> = (0..5).map(|_| Repository::new()).collect();
    for (repository, key) in repositories
        .iter()
        .zip(["OLD", "NEW", "MINE", "LOCKED", "GONE"])
    {
        installed
            .home
            .quadratique_at(&installed.binary, &repository.root)
            .args(["init", "--key", key])
            .run()
            .ok();
    }
    installed
        .home
        .quadratique_at(&installed.binary, installed.home.path())
        .args(["server", "stop"])
        .run()
        .ok();
    let [older, current, changed, locked, gone] =
        [0, 1, 2, 3, 4].map(|index| repositories[index].root.clone());
    write(&older, SKILL_PATH, &earlier);
    write(&changed, SKILL_PATH, &format!("{SKILL}\nOur own rule.\n"));
    write(&locked, SKILL_PATH, &format!("{SKILL}\nOur own rule.\n"));
    std::fs::set_permissions(
        locked.join(PLANNING_PATH),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    std::fs::rename(&gone, gone.with_file_name("moved")).unwrap();
    let before: Vec<String> = [&older, &current, &changed]
        .iter()
        .map(|root| git_stdout(root, &["status", "--porcelain"]))
        .collect();

    let answer = installed.upgrade(&releases, &["--skills"]).ok();

    std::fs::set_permissions(
        locked.join(PLANNING_PATH),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let report = &answer["skills"];
    let shown = |root: &PathBuf| root.display().to_string();
    assert_eq!(
        report["older"],
        json!([{ "repository": shown(&older), "run": format!("cd {} && quadratique init", shown(&older)) }]),
        "{report}"
    );
    assert_eq!(report["current"], json!([shown(&current)]), "{report}");
    let kept: Vec<&str> = report["kept"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["repository"].as_str().unwrap())
        .collect();
    assert_eq!(
        kept,
        [shown(&locked), shown(&changed)],
        "ordered by key: {report}"
    );
    for entry in report["kept"].as_array().unwrap() {
        let warning = entry["warnings"][0].as_str().unwrap();
        assert!(warning.contains(SKILL_PATH), "{warning}");
        assert!(
            warning.contains("quadratique init --print SKILL.md"),
            "{warning}"
        );
    }
    assert_eq!(
        report["errors"][0]["repository"],
        shown(&locked),
        "{report}"
    );
    assert!(
        report["errors"][0]["errors"][0]
            .as_str()
            .unwrap()
            .contains("planning.md"),
        "{report}"
    );
    assert_eq!(report["missing"], json!([gone.join(".git")]), "{report}");
    assert!(
        report["note"]
            .as_str()
            .unwrap()
            .contains("Nothing was written")
    );
    assert!(
        installed.home.servers().is_empty(),
        "the report started a server"
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
