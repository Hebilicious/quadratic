//! `quadratique upgrade` against a local fake of the GitHub release source. The binary under test
//! is a debug build, the only kind that reads QUADRATIQUE_UPGRADE_SOURCE; nothing here reaches
//! the network. Every run finds a fake `systemctl` first on its PATH, so no test reads or
//! changes this machine's user services.
//!
//! The upgrade finds the home's server on its socket and stops only the generation it saw before
//! the rename, and only when that server says it runs the replaced file
//! (docs/decisions/0004-a-socket-for-commands-and-a-token-for-the-browser.md, §3).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::support::{
    BINARY, Home, Quadratique, Repository, Run, Step, Watch, await_exit, git_stdout,
};

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
/// environment, from `/`, and when run through the descriptor of the written file; its skill
/// report names its version, so a test can tell which build made it.
fn new_binary(version: &str) -> Vec<u8> {
    format!(
        "#!/bin/sh\ncase \"$1 $2\" in\n--version*)\n  if [ -n \"$HOME$QUADRATIQUE_HOME$QUADRATIQUE_UPGRADE_SOURCE\" ] || [ \"$(pwd)\" != / ]; then echo 'not an empty environment' >&2; exit 9; fi\n  case \"$0\" in /proc/self/fd/*) ;; *) echo \"run by its path $0, not by the written file\" >&2; exit 8 ;; esac\n  echo {version}; exit 0 ;;\n'upgrade --skills') echo '{{\"skills\":{{\"madeBy\":\"{version}\"}}}}'; exit 0 ;;\nesac\nexit 2\n"
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

    /// Writes into the install lock the device and inode of the installed file, as a run does
    /// just before its rename.
    fn record_replaced(&self) {
        use std::os::unix::fs::MetadataExt;

        let meta = std::fs::metadata(&self.binary).unwrap();
        std::fs::write(
            self.binary.with_file_name(LOCK),
            format!("replaced {} {}\n", meta.dev(), meta.ino()),
        )
        .unwrap();
    }

    /// Starts the home's server from the installed file, then replaces the file by another copy
    /// as a run that was killed after its rename would, so that the server runs the replaced
    /// file. Returns the server's pid.
    fn server_of_an_earlier_build(&self, releases: &Releases) -> u32 {
        self.quadratique(releases)
            .args(["project", "list"])
            .run()
            .ok();
        let servers = self.home.servers();
        assert_eq!(servers.len(), 1, "{servers:?}");
        self.record_replaced();
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

/// A refusal after the upgrade took its lock: the lock file exists and leftovers may be gone, so
/// it says only that the installed binary was not changed.
fn refused_after_the_lock(run: &Run, words: &str) {
    assert_ne!(run.code, 0, "{run:?}");
    assert!(run.stderr.contains(words), "{run:?}");
    assert!(
        run.stderr.contains("The installed binary was not changed.")
            && !run.stderr.contains("Nothing was changed."),
        "{run:?}"
    );
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
    assert!(
        !installed.home.path().join("server.lock").exists(),
        "looking at the home created its lock file"
    );
}

#[test]
fn a_failed_download_leaves_the_old_binary_in_place_and_working() {
    let releases = Releases::start();
    let installed = Installed::file();
    let archive = format!("/download/{}", archive_name(LATEST));
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));

    releases.set(&archive, Reply::Status(500));
    refused_after_the_lock(&installed.upgrade(&releases, &[]), "answered 500");
    installed.still_runs();

    releases.set(&archive, Reply::Cut(b"the start of an archive".to_vec()));
    refused_after_the_lock(&installed.upgrade(&releases, &[]), "The download of");
    installed.still_runs();
}

#[test]
fn an_archive_is_installed_only_with_the_digest_that_the_api_gives_for_it() {
    let releases = Releases::start();
    let installed = Installed::file();
    let published = assets(LATEST, &new_binary(LATEST));

    releases.publish_with(LATEST, published.clone(), Digest256::Wrong);
    refused_after_the_lock(
        &installed.upgrade(&releases, &[]),
        "does not match the SHA-256 that the GitHub API gives for it",
    );
    installed.still_runs();

    releases.publish_with(LATEST, published, Digest256::Absent);
    refused_after_the_lock(
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

    refused_after_the_lock(&installed.upgrade(&releases, &[]), "for this platform");
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

        refused_after_the_lock(&installed.upgrade(&releases, &[]), "is refused");
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
        refused_after_the_lock(&installed.upgrade(&releases, &[]), words);
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
    refused_after_the_lock(
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
    refused_after_the_lock(
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
    for mode in [0o757, 0o775] {
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(mode)).unwrap();
        refused(
            &installed.upgrade(&releases, &[]),
            "other accounts can write",
        );
    }
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

    refused_after_the_lock(
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
        .with_file_name(".quadratique-upgrade-0123456789abcdef01234567");
    std::fs::create_dir(&leftover).unwrap();
    std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(leftover.join("quadratique"), "half written").unwrap();
    let persons = installed
        .binary
        .with_file_name(".quadratique-upgrade-notes");
    std::fs::create_dir(&persons).unwrap();
    std::fs::set_permissions(&persons, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(persons.join("quadratique"), "a person's file").unwrap();

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["removedLeftovers"], json!([leftover]), "{answer}");
    assert_eq!(answer["leftBesideTheBinary"], json!([persons]), "{answer}");
    assert_eq!(installed.beside(), [".quadratique-upgrade-notes"]);
    assert_eq!(
        std::fs::read_to_string(persons.join("quadratique")).unwrap(),
        "a person's file"
    );
}

/// A fake `systemctl` that runs `quadratique.service` as systemd would: its main process is the
/// pid in `<state>/pid` while that process lives (a zombie has exited), and a start of an
/// inactive unit starts the installed binary's server with take-over. It logs every call.
///
/// With a file `<state>/start-another`, the first `show` that finds the unit inactive first runs a
/// command of the installed binary, which starts a server on demand and returns once it is ready,
/// and writes that server's `server status` to `<state>/another.json`: a command that started a
/// server between the upgrade's stop and the unit's start.
fn service(state: &Path, binary: &Path, needs_reload: bool) -> String {
    format!(
        "#!/bin/sh\nstate={state}\necho \"$*\" >> \"$state/calls\"\nalive() {{ pid=$(cat \"$state/pid\" 2>/dev/null) && [ -r \"/proc/$pid/stat\" ] && [ \"$(sed 's/^.*) //' \"/proc/$pid/stat\" | cut -c1)\" != Z ]; }}\ncase \"$1 $2\" in\n'--user show') if alive; then echo LoadState=loaded; echo ActiveState=active; echo \"MainPID=$pid\"; else if [ -f \"$state/start-another\" ] && [ ! -f \"$state/another.json\" ]; then {binary} project list > /dev/null 2>&1; {binary} server status > \"$state/another.json\"; fi; echo LoadState=loaded; echo ActiveState=inactive; echo MainPID=0; fi; echo NeedDaemonReload={reload} ;;\n'--user start') alive || {{ nohup {binary} serve --take-over > \"$state/log\" 2>&1 & echo $! > \"$state/pid\"; }} ;;\nesac\n",
        state = state.display(),
        binary = binary.display(),
        reload = if needs_reload { "yes" } else { "no" },
    )
}

/// The generation of the home's server, from `server status` over its socket.
fn generation_of(installed: &Installed, releases: &Releases) -> String {
    let status = installed
        .quadratique(releases)
        .args(["server", "status"])
        .run()
        .ok();
    status["generation"]
        .as_str()
        .unwrap_or_else(|| panic!("no generation: {status}"))
        .to_owned()
}

/// The interrupted upgrade under the service: the generation that runs the replaced file is
/// asked to stop over the socket, and once systemd has seen its main process leave, the unit is
/// started again and its `serve --take-over` runs the installed file. Nothing restarts the unit.
#[test]
fn under_the_service_the_generation_on_the_replaced_file_is_stopped_and_the_unit_started_until_the_new_build_serves()
 {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(CURRENT, assets(CURRENT, b"never installed"));
    let earlier = installed.server_of_an_earlier_build(&releases);
    let generation = generation_of(&installed, &releases);
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
        format!(
            "stopped the earlier build and started quadratique.service again; {CURRENT} serves this home (pid {now}, generation {})",
            generation_of(&installed, &releases)
        ),
        "{answer}"
    );
    let status = installed
        .quadratique(&releases)
        .args(["server", "status"])
        .run()
        .ok();
    assert_eq!(
        status["running"], true,
        "answered before the new build served: {status}"
    );
    assert_eq!(status["sameInterface"], true, "{status}");
    assert_eq!(status["pid"], now, "{status}");
    assert_ne!(status["generation"], json!(generation), "{status}");
    await_exit(earlier);
    assert_eq!(
        std::fs::canonicalize(format!("/proc/{now}/exe")).unwrap(),
        installed.binary
    );
    let calls = std::fs::read_to_string(state.join("calls")).unwrap();
    assert!(
        calls.contains("--user start quadratique.service"),
        "{calls}"
    );
    assert!(!calls.contains("restart"), "{calls}");
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
    assert!(warning.contains("nothing was stopped"), "{warning}");
    let calls = std::fs::read_to_string(state.join("calls")).unwrap();
    assert!(
        !calls.contains("restart") && !calls.contains("start"),
        "{calls}"
    );
    assert_eq!(installed.home.servers(), vec![earlier]);
}

/// An upgrade killed between its rename and its stop left the server on the replaced file; the
/// next run, which finds the binary up to date, stops that generation.
#[test]
fn an_interrupted_upgrade_is_finished_by_the_next_run_which_stops_the_generation_on_the_replaced_file()
 {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(CURRENT, assets(CURRENT, b"never installed"));
    let earlier = installed.server_of_an_earlier_build(&releases);
    let generation = generation_of(&installed, &releases);

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["upToDate"], true, "{answer}");
    assert_eq!(
        answer["server"],
        format!(
            "stopped the server of the earlier build (pid {earlier}, generation {generation}); the next quadratique command starts this build"
        ),
        "{answer}"
    );
    await_exit(earlier);
    assert!(installed.home.servers().is_empty());

    let again = installed.upgrade(&releases, &[]).ok();
    assert!(again.get("server").is_none(), "{again}");
}

/// The server was started from the installed file, which the upgrade replaces: the generation
/// read before the rename is stopped after it.
#[test]
fn a_server_that_runs_the_replaced_file_is_stopped_by_the_generation_seen_before_the_rename() {
    let releases = Releases::start();
    let installed = Installed::file();
    let binary = new_binary(LATEST);
    releases.publish(LATEST, assets(LATEST, &binary));
    installed
        .quadratique(&releases)
        .args(["project", "list"])
        .run()
        .ok();
    let servers = installed.home.servers();
    assert_eq!(servers.len(), 1, "{servers:?}");
    let generation = generation_of(&installed, &releases);

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["to"], LATEST, "{answer}");
    assert_eq!(
        answer["server"],
        format!(
            "stopped the server of the earlier build (pid {}, generation {generation}); the next quadratique command starts this build",
            servers[0]
        ),
        "{answer}"
    );
    assert!(answer.get("warnings").is_none(), "{answer}");
    assert_eq!(installed.bytes(), binary);
    await_exit(servers[0]);
    assert!(installed.home.servers().is_empty());
}

/// The home's server runs a copy installed elsewhere: the upgrade replaces its own file and
/// leaves that server running.
#[test]
fn a_server_that_runs_another_file_is_left_running_with_a_message() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let elsewhere = installed.root().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let other = elsewhere.join("quadratique");
    copy_binary(&other);
    installed
        .home
        .quadratique_at(&other, installed.home.path())
        .args(["project", "list"])
        .run()
        .ok();
    let servers = installed.home.servers();
    assert_eq!(servers.len(), 1, "{servers:?}");

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["to"], LATEST, "{answer}");
    assert_eq!(
        answer["server"],
        format!(
            "the server of this home (pid {}) runs {}, not the replaced binary: it was left running",
            servers[0],
            other.display()
        ),
        "{answer}"
    );
    assert_eq!(installed.home.servers(), servers);
}

/// A server started with `quadratique serve` from the replaced file is stopped, and the answer
/// says to restart it the way it was started.
#[test]
fn a_server_started_with_serve_is_stopped_and_the_person_reads_how_to_restart_it() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let mut watch = Watch::of(&installed.home);
    let serve = installed
        .home
        .quadratique_at(&installed.binary, installed.home.path())
        .args(["serve"])
        .spawn();
    watch.await_steps(&[Step::Published("server.json")]);
    let generation = generation_of(&installed, &releases);

    let answer = installed.upgrade(&releases, &[]).ok();

    let warning = answer["warnings"][0].as_str().unwrap_or_default();
    assert!(
        warning.contains("stopped the server of the earlier build")
            && warning.contains(&generation)
            && warning
                .contains("started with `quadratique serve`: restart it the way you started it"),
        "{answer}"
    );
    let served = serve.finish();
    assert_eq!(served.code, 0, "{served:?}");
}

const OTHER_GENERATION: &str = "fedcba9876543210fedcba9876543210";
const GENERATION: &str = "0123456789abcdef0123456789abcdef";

/// What a [`FakeServer`] answers to a stop request of generation `GENERATION`.
#[derive(Clone, Copy, PartialEq)]
enum OnStop {
    /// `409` naming another generation: the generation the upgrade saw no longer serves.
    Conflict,
    /// `202`, then it stops listening, as a server does that shuts down.
    Accept,
}

/// A fake server on the home's socket that says it runs the installed file, in generation
/// `GENERATION`. A stop request of any other generation gets `400`. It records the generation of
/// each stop request it receives.
struct FakeServer {
    asked: Arc<Mutex<Vec<String>>>,
}

impl FakeServer {
    fn start(installed: &Installed, on_stop: OnStop) -> FakeServer {
        use std::os::unix::fs::MetadataExt;

        let path = installed.home.path().join("server.sock");
        let listener = UnixListener::bind(&path).expect("the fake server's socket");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let meta = std::fs::metadata(&installed.binary).unwrap();
        let identity = json!({
            "pid": 4242,
            "database": installed.home.path().canonicalize().unwrap().join("workspace.db"),
            "interface": quadratique_server::app::interface(),
            "schema": 1,
            "generation": GENERATION,
            "control": 1,
            "controls": [1],
            "executable": { "path": installed.binary, "device": meta.dev(), "inode": meta.ino() },
            "started": "on-demand",
            "url": "http://127.0.0.1:9/cli",
        })
        .to_string();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&asked);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                let _ = reader.read_line(&mut first);
                let mut length = 0;
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let stop = first.starts_with("POST /server/stop ");
                let (status, answer) = if stop {
                    let generation =
                        serde_json::from_slice::<Value>(&body).unwrap_or_default()["generation"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned();
                    recorded.lock().unwrap().push(generation.clone());
                    match on_stop {
                        _ if generation != GENERATION => ("400 Bad Request", String::new()),
                        OnStop::Conflict => (
                            "409 Conflict",
                            json!({ "generation": OTHER_GENERATION }).to_string(),
                        ),
                        OnStop::Accept => (
                            "202 Accepted",
                            json!({ "generation": GENERATION }).to_string(),
                        ),
                    }
                } else {
                    ("200 OK", identity.clone())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                if stop && status.starts_with("202") {
                    return;
                }
            }
        });
        FakeServer { asked }
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

#[test]
fn a_stop_answered_with_409_stops_nothing_and_says_another_generation_serves() {
    let releases = Releases::start();
    let installed = Installed::file();
    let binary = new_binary(LATEST);
    releases.publish(LATEST, assets(LATEST, &binary));
    let fake = FakeServer::start(&installed, OnStop::Conflict);

    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(answer["to"], LATEST, "{answer}");
    assert_eq!(installed.bytes(), binary);
    assert_eq!(fake.asked(), [GENERATION], "{answer}");
    assert_eq!(
        answer["warnings"],
        json!([format!(
            "the server of this home is no longer generation {GENERATION}, which this command asked to stop: another generation ({OTHER_GENERATION}) serves now. Nothing was stopped."
        )]),
        "{answer}"
    );
}

/// A command holds `start.lock` while the server it starts becomes ready. The upgrade waits for
/// `start.lock` before its last look, so it sees that server, which runs the file it replaces, and
/// stops it after the rename. Without the wait it would have looked first, seen nobody, and left
/// that server on the replaced file.
#[test]
fn a_server_that_becomes_ready_while_the_upgrade_waits_for_start_lock_is_seen_and_stopped() {
    let releases = Releases::start();
    let installed = Installed::file();
    let binary = new_binary(LATEST);
    releases.publish(LATEST, assets(LATEST, &binary));
    let starter = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(installed.home.path().join("start.lock"))
        .unwrap();
    starter.lock().unwrap();
    let mut watch = Watch::of(&installed.home);
    let upgrade = installed.quadratique(&releases).args(["upgrade"]).spawn();

    watch.await_steps(&[Step::Opened("start.lock")]);
    let fake = FakeServer::start(&installed, OnStop::Accept);
    drop(starter);
    let answer = upgrade.finish().ok();

    assert_eq!(answer["to"], LATEST, "{answer}");
    assert_eq!(installed.bytes(), binary);
    assert_eq!(fake.asked(), [GENERATION], "{answer}");
    assert_eq!(
        answer["server"],
        format!(
            "stopped the server of the earlier build (pid 4242, generation {GENERATION}); the next quadratique command starts this build"
        ),
        "{answer}"
    );
}

/// A command starts a server between the upgrade's stop and the unit's start (the fake runs it
/// when the upgrade first finds the unit inactive). The unit's take-over replaces it, and the
/// answer names it.
#[test]
fn a_server_started_between_the_stop_and_the_service_start_is_named_in_the_answer() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(CURRENT, assets(CURRENT, b"never installed"));
    let earlier = installed.server_of_an_earlier_build(&releases);
    let state = installed.root().join("systemd");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("pid"), earlier.to_string()).unwrap();
    std::fs::write(state.join("start-another"), "").unwrap();
    installed.systemctl(&service(&state, &installed.binary, false));

    let answer = installed.upgrade(&releases, &[]).ok();

    let another: Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("another.json")).unwrap())
            .unwrap();
    let (generation, pid) = (
        another["generation"].as_str().unwrap(),
        another["pid"].as_u64().unwrap(),
    );
    let now: u32 = std::fs::read_to_string(state.join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let told = answer["server"].as_str().unwrap_or_default();
    assert!(
        told.starts_with("stopped the earlier build and started quadratique.service again")
            && told.contains(&format!("serves this home (pid {now}, "))
            && told.contains(&format!(
                "Generation {generation} (pid {pid}), which a command started after the earlier build stopped, was replaced by the take-over of quadratique.service, whose server serves now"
            )),
        "{answer}"
    );
    await_exit(u32::try_from(pid).unwrap());
    await_exit(earlier);
    assert_eq!(installed.home.servers(), vec![now]);
}

/// The bring-up has one deadline, and each `systemctl` call gets only what is left of it: a
/// `systemctl show` that never answers ends at the deadline (1.5 seconds in this debug build),
/// not at a limit of its own, and nothing is stopped.
#[test]
fn a_systemctl_that_never_answers_ends_at_the_bring_up_deadline_and_stops_nothing() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let fake = FakeServer::start(&installed, OnStop::Accept);
    let never_written = installed.root().join("never-written");
    let made = std::process::Command::new("mkfifo")
        .arg(&never_written)
        .status()
        .unwrap();
    assert!(made.success());
    installed.systemctl(&format!(
        "#!/bin/sh\nread -r _ < {}\n",
        never_written.display()
    ));

    let answer = installed
        .quadratique(&releases)
        .env("QUADRATIQUE_UPGRADE_BRING_UP_MS", "1500")
        .args(["upgrade"])
        .run()
        .ok();

    let warning = answer["warnings"][0].as_str().unwrap_or_default();
    assert!(
        warning.contains("quadratique.service could not be inspected")
            && warning.contains("gave no answer within 1 seconds")
            && warning.contains("was left running"),
        "{answer}"
    );
    assert!(fake.asked().is_empty(), "{answer}");
}

/// A fake `systemctl` whose `quadratique.service` is loaded, inactive, and starts `binary`.
fn inactive_service(state: &Path, binary: &Path) -> String {
    format!(
        "#!/bin/sh\necho \"$*\" >> {state}/calls\ncase \"$1 $2\" in\n'--user show') echo LoadState=loaded; echo ActiveState=inactive; echo MainPID=0; echo NeedDaemonReload=no; echo 'ExecStart={{ path={binary} ; argv[]={binary} serve --take-over ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }}' ;;\nesac\n",
        state = state.display(),
        binary = binary.display(),
    )
}

/// An upgrade killed after it stopped the service's server and before it started the unit again
/// leaves the unit inactive. The next run starts nothing (the person may have stopped the unit),
/// and says what to run; with the unit starting another binary, it says nothing of it.
#[test]
fn an_upgrade_killed_before_the_service_start_leaves_a_run_that_says_how_to_start_the_unit() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(CURRENT, assets(CURRENT, b"never installed"));
    installed.record_replaced();
    let copy = installed.binary.with_file_name("copy");
    copy_binary(&copy);
    std::fs::rename(&copy, &installed.binary).unwrap();
    let state = installed.root().join("systemd");
    std::fs::create_dir_all(&state).unwrap();
    installed.systemctl(&inactive_service(&state, &installed.binary));

    let answer = installed.upgrade(&releases, &[]).ok();
    let checked = installed.upgrade(&releases, &["--check"]).ok();
    installed.systemctl(&inactive_service(
        &state,
        Path::new("/elsewhere/quadratique"),
    ));
    let other = installed.upgrade(&releases, &[]).ok();

    let warning = answer["warnings"][0].as_str().unwrap_or_default();
    assert!(
        warning
            .contains("quadratique.service, which starts this binary, is not running (inactive)")
            && warning.contains("systemctl --user start quadratique.service"),
        "{answer}"
    );
    assert!(
        checked["server"]
            .as_str()
            .unwrap_or_default()
            .contains("systemctl --user start quadratique.service"),
        "{checked}"
    );
    assert!(other.get("warnings").is_none(), "{other}");
    let calls = std::fs::read_to_string(state.join("calls")).unwrap();
    assert!(!calls.contains("--user start"), "{calls}");
    assert!(installed.home.servers().is_empty());
}

/// The read-only forms create neither a missing data folder nor the install lock.
#[test]
fn check_and_the_reports_create_no_data_folder_and_no_install_lock() {
    let releases = Releases::start();
    let installed = Installed::file();
    let missing = installed.home.path().join("missing");
    let read_only = |args: &[&str]| {
        installed
            .quadratique(&releases)
            .env("QUADRATIQUE_HOME", &missing.display().to_string())
            .args(["upgrade"])
            .args(args)
            .run()
            .ok()
    };

    releases.publish(CURRENT, assets(CURRENT, b"never installed"));
    let up_to_date = read_only(&["--check"]);
    let skills = read_only(&["--skills"]);
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let newer = read_only(&["--check"]);

    assert_eq!(up_to_date["upToDate"], true, "{up_to_date}");
    assert_eq!(newer["upToDate"], false, "{newer}");
    assert_eq!(
        skills["skills"],
        json!({ "current": [], "older": [] }),
        "{skills}"
    );
    assert!(!missing.exists(), "a read-only form made the data folder");
    assert!(
        !installed.binary.with_file_name(LOCK).exists(),
        "a read-only form made the install lock"
    );
}

/// A server from before the socket (release 0.1): the home's lock held by a process, no socket,
/// and a runtime file without a control version that names a TCP address. The upgrade, its
/// report and `init --dry-run` find it through the state table, connect to no TCP address, and
/// the upgrade stops nothing and says what to stop.
#[test]
fn a_server_from_before_the_socket_is_found_without_tcp_and_the_upgrade_says_what_to_stop() {
    let releases = Releases::start();
    let installed = Installed::file();
    let repository = Repository::new();
    releases.publish(LATEST, assets(LATEST, &new_binary(LATEST)));
    let lock = installed.home.path().join("server.lock");
    let mut old_server = std::process::Command::new("sh")
        .arg("-c")
        .arg("exec 9>>\"$0\"; flock -x 9; echo ready; read -r _")
        .arg(&lock)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut said = String::new();
    BufReader::new(old_server.stdout.take().unwrap())
        .read_line(&mut said)
        .unwrap();
    assert_eq!(said.trim(), "ready");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    std::fs::write(
        installed.home.runtime_path(),
        json!({
            "pid": old_server.id(),
            "address": address.to_string(),
            "url": format!("http://{address}/cli"),
            "database": installed.home.path().canonicalize().unwrap().join("workspace.db"),
        })
        .to_string(),
    )
    .unwrap();

    let report = installed.upgrade(&releases, &["--skills"]).ok();
    let dry_run = installed
        .home
        .quadratique_at(&installed.binary, &repository.root)
        .args(["init", "--dry-run"])
        .run();
    let answer = installed.upgrade(&releases, &[]).ok();

    assert_eq!(
        report["skills"],
        json!({ "current": [], "older": [] }),
        "{report}"
    );
    assert!(dry_run.stdout.contains("before the socket"), "{dry_run:?}");
    assert_eq!(answer["to"], LATEST, "{answer}");
    let warning = answer["warnings"][0].as_str().unwrap_or_default();
    assert!(
        warning.contains("release 0.1")
            && warning.contains("The upgrade stopped nothing")
            && warning.contains("systemctl --user stop quadratique.service"),
        "{answer}"
    );
    assert!(
        old_server.try_wait().unwrap().is_none(),
        "the old server was stopped"
    );
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "a command connected to the old server's TCP address"
    );
    let _ = old_server.kill();
    let _ = old_server.wait();
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

#[test]
fn a_server_on_a_file_that_no_run_recorded_as_replaced_is_left_running() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(CURRENT, assets(CURRENT, b"never installed"));
    let earlier = installed.server_of_an_earlier_build(&releases);
    std::fs::remove_file(installed.binary.with_file_name(LOCK)).unwrap();

    let answer = installed.upgrade(&releases, &[]).ok();

    assert!(answer.get("server").is_none(), "{answer}");
    assert_eq!(installed.home.servers(), vec![earlier]);
}

#[test]
fn a_service_that_cannot_be_inspected_stops_nothing() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(CURRENT, assets(CURRENT, b"never installed"));
    let earlier = installed.server_of_an_earlier_build(&releases);
    installed.systemctl("#!/bin/sh\necho 'Failed to connect to bus' >&2\nexit 1\n");

    let answer = installed.upgrade(&releases, &[]).ok();

    let warning = answer["warnings"][0].as_str().unwrap();
    assert!(warning.contains("could not be inspected"), "{warning}");
    assert_eq!(installed.home.servers(), vec![earlier]);
}

#[test]
fn the_repair_of_an_earlier_run_waits_for_no_other_upgrade() {
    let releases = Releases::start();
    let installed = Installed::file();
    releases.publish(CURRENT, assets(CURRENT, b"never installed"));
    let earlier = installed.server_of_an_earlier_build(&releases);
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(installed.binary.with_file_name(LOCK))
        .unwrap();
    lock.lock().unwrap();

    let answer = installed.upgrade(&releases, &[]).ok();

    let warning = answer["warnings"][0].as_str().unwrap();
    assert!(warning.contains("another quadratique upgrade"), "{warning}");
    assert_eq!(installed.home.servers(), vec![earlier]);
}

#[test]
fn while_a_server_runs_the_report_comes_from_it_and_not_from_the_store_file() {
    let releases = Releases::start();
    let installed = Installed::file();
    let repository = Repository::new();
    installed
        .home
        .quadratique_at(&installed.binary, &repository.root)
        .args(["init", "--key", "SRV"])
        .run()
        .ok();
    let store = installed.store();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o000)).unwrap();

    let answer = installed.upgrade(&releases, &["--skills"]).ok();

    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        answer["skills"]["current"],
        json!([repository.root.display().to_string()]),
        "{answer}"
    );
}
