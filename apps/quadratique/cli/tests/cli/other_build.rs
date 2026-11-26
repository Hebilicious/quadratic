//! The row of the record's state table (docs/decisions/0004-…, §2) for a server from before the socket, as release
//! 0.1 runs it: the home's lock held, no socket, a runtime file without a control version, and a TCP listener that
//! answers `GET /server`. The command line uses no such server, and stops it only with proof from `/proc` that the
//! process runs the installed binary and holds the exclusive lock of this home's `server.lock` (§3). The fake plays
//! both sides: a process that holds the lock and runs the installed binary, and processes that miss part of it.

use std::fs::{File, TryLockError};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::support::{Home, Run, Step, Watch, await_exit, await_until, kill, outside};

const UNAVAILABLE: i32 = 4;
/// A shell plays the old server: it takes the home's lock on its own descriptor 9 and keeps it, as a server of release
/// 0.1 keeps `server.lock` locked, and says so once it holds it.
const HOLDS_THE_LOCK: &str = "exec 9>>\"$0\"; flock -x 9; echo ready; read -r _";
/// The same, and on a line from the test it closes the descriptor, which frees the lock, and says so.
const HOLDS_THE_LOCK_UNTIL_TOLD: &str =
    "exec 9>>\"$0\"; flock -x 9; echo ready; read -r _; exec 9>&-; echo released; read -r _";
/// The same, ignoring SIGTERM: its children close descriptor 9, so only the shell holds the lock.
const HOLDS_THE_LOCK_IGNORING_SIGTERM: &str = "exec 9>>\"$0\"; flock -x 9; trap 'touch \"$1\"' TERM; echo ready; while :; do tail -f /dev/null 9>&- & wait $!; done";
/// No descriptor of the lock at all: a process that only answers for the server, once it runs (it says so).
const ANSWERS_ONLY: &str = "echo ready; read -r _";
/// A descriptor of the lock file of its own, and no lock: the test holds the lock.
const OPENS_THE_LOCK: &str = "exec 9<\"$0\"; echo ready; read -r _";

struct LegacyServer {
    process: Child,
    output: BufReader<ChildStdout>,
    requests: Arc<Mutex<Vec<String>>>,
    _lock: Option<File>,
}

impl LegacyServer {
    /// Runs `program`, a shell, as the old server's process: `script` on the home's lock file (with `extra` arguments),
    /// and waits until it says it is ready. With `test_holds_the_lock`, the test takes the lock first. Then starts its
    /// TCP listener and writes its runtime file.
    fn start(
        home: &Home,
        program: &Path,
        script: &str,
        test_holds_the_lock: bool,
        extra: &[&Path],
    ) -> LegacyServer {
        let lock_path = home.path().join("server.lock");
        let lock = test_holds_the_lock.then(|| hold(&lock_path));
        let mut process = Command::new(program)
            .arg("-c")
            .arg(script)
            .arg(&lock_path)
            .args(extra)
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the old server's process starts");
        let mut output = BufReader::new(process.stdout.take().expect("its output"));
        let mut said = String::new();
        output
            .read_line(&mut said)
            .expect("the old server says it is ready");
        assert_eq!(said.trim(), "ready");
        let pid = process.id();
        let database = home
            .path()
            .canonicalize()
            .expect("a real path")
            .join("workspace.db");
        let identity = json!({ "pid": pid, "database": database });
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let address = listener.local_addr().expect("an address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().expect("a stream"));
                let mut first = String::new();
                let _ = reader.read_line(&mut first);
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                    line.clear();
                }
                recorded
                    .lock()
                    .expect("the request log")
                    .push(first.trim().to_owned());
                let body = identity.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        write_runtime(home, pid, &address.to_string());
        LegacyServer {
            process,
            output,
            requests,
            _lock: lock,
        }
    }

    fn pid(&self) -> u32 {
        self.process.id()
    }

    fn still_runs(&mut self) -> bool {
        self.process
            .try_wait()
            .expect("the process's state")
            .is_none()
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("the request log").clone()
    }

    /// Sends a line and waits for the process's answer line.
    fn tell(&mut self, expected: &str) {
        writeln!(self.process.stdin.as_mut().expect("its input")).expect("a line to the process");
        let mut said = String::new();
        self.output
            .read_line(&mut said)
            .expect("the process answers");
        assert_eq!(said.trim(), expected);
    }
}

impl Drop for LegacyServer {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn hold(path: &Path) -> File {
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .expect("the lock file");
    lock.lock().expect("the test holds the lock");
    lock
}

fn write_runtime(home: &Home, pid: u32, address: &str) {
    let database = home
        .path()
        .canonicalize()
        .expect("a real path")
        .join("workspace.db");
    let runtime = json!({
        "pid": pid,
        "address": address,
        "url": format!("http://{address}/cli"),
        "database": database,
    });
    std::fs::write(home.runtime_path(), runtime.to_string()).expect("the runtime file");
}

/// Where `install.sh` of release 0.1 put the binary, for a command whose `HOME` is the test home: a copy of `program`,
/// which the old server then runs.
fn install(home: &Home, program: &str) -> PathBuf {
    let folder = home.path().join(".local/bin");
    std::fs::create_dir_all(&folder).expect("the install folder");
    let installed = folder.join("quadratique");
    copy(program, &installed);
    installed
}

/// Copies through `cp`: a file this process wrote would stay open for writing in any child that another test forks
/// at that moment, until that child's `exec`, and running the copy would then fail with "Text file busy".
fn copy(program: &str, to: &Path) {
    let copied = Command::new("cp")
        .arg(program)
        .arg(to)
        .status()
        .expect("cp runs");
    assert!(copied.success(), "cp {program} {}", to.display());
}

fn run(home: &Home, args: &[&str]) -> Run {
    home.quadratique(outside().path()).args(args).run()
}

fn message(refused: &Run) -> String {
    let failure: Value = serde_json::from_str(refused.stderr.lines().last().unwrap_or_default())
        .unwrap_or_else(|error| panic!("standard error is not JSON ({error}): {refused:?}"));
    failure["message"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn a_server_from_before_the_socket_is_found_used_by_no_command_and_stopped_with_its_proof() {
    let home = Home::new();
    let installed = install(&home, "/bin/sh");
    let old = LegacyServer::start(&home, &installed, HOLDS_THE_LOCK, false, &[]);
    let pid = old.pid();

    let status = run(&home, &["server", "status"]).ok();
    assert_eq!(status["running"], true, "{status}");
    assert_eq!(status["legacy"], true, "{status}");
    assert_eq!(status["pid"], pid, "{status}");
    assert_eq!(status["sameInterface"], false, "{status}");
    assert_eq!(status["provenStop"], true, "{status}");
    assert_eq!(status["stop"], "quadratique server stop", "{status}");

    let refused = run(&home, &["project", "list"]);
    assert_eq!(refused.code, UNAVAILABLE, "{refused:?}");
    let refusal = message(&refused);
    assert!(
        refusal.contains("from before the socket (release 0.1)")
            && refusal.contains("Nothing was started. Run `quadratique server stop`"),
        "{refusal}"
    );
    assert!(home.servers().is_empty(), "a server was started");
    assert!(
        !home.path().join("server.log").exists(),
        "a server was spawned"
    );
    assert!(
        !home.path().join("start.lock").exists(),
        "a refusal reserved a start"
    );

    let stopped = run(&home, &["server", "stop"]).ok();
    assert_eq!(stopped["stopped"], true, "{stopped}");
    assert_eq!(stopped["pid"], pid, "{stopped}");
    await_exit(pid);
    assert!(old.requests().is_empty(), "{:?}", old.requests());

    let projects: Value = run(&home, &["project", "list"]).ok();
    assert_eq!(projects, json!({ "items": [] }));
    let status = run(&home, &["server", "status"]).ok();
    assert_eq!(status["sameInterface"], true, "{status}");
    assert!(status.get("legacy").is_none(), "{status}");
    let new = status["pid"].as_u64().expect("a pid") as u32;
    let stopped = run(&home, &["server", "stop"]).ok();
    assert_eq!(stopped["pid"], new, "{stopped}");
}

/// One way the proof can be missing.
struct Unproven {
    case: &'static str,
    /// What the home's install path holds: a copy of a program, or a link to it.
    installed: Installed,
    /// The old server's program: the installed file, or another one.
    runs_installed: bool,
    program: &'static str,
    script: &'static str,
    test_holds_the_lock: bool,
    why: &'static str,
}

enum Installed {
    Nothing,
    CopyOf(&'static str),
    LinkTo(&'static str),
    /// A copy in a folder that the install path reaches through a link.
    CopyBehindALinkedFolder(&'static str),
}

/// Without the proof, no process gets a signal, whatever answers on the port. Each command names what to run instead,
/// and a refusal reserves nothing.
#[test]
fn a_server_from_before_the_socket_without_proof_gets_no_signal_and_the_person_reads_what_to_stop()
{
    let cases = [
        Unproven {
            case: "answers on the port and holds no lock",
            installed: Installed::CopyOf("/bin/sh"),
            runs_installed: true,
            program: "",
            script: ANSWERS_ONLY,
            test_holds_the_lock: true,
            why: "does not hold this home's lock",
        },
        Unproven {
            case: "holds a descriptor of the lock file of its own while another process holds the lock",
            installed: Installed::CopyOf("/bin/sh"),
            runs_installed: true,
            program: "",
            script: OPENS_THE_LOCK,
            test_holds_the_lock: true,
            why: "does not hold this home's lock",
        },
        Unproven {
            case: "holds the lock and runs another binary",
            installed: Installed::Nothing,
            runs_installed: false,
            program: "/bin/sh",
            script: HOLDS_THE_LOCK,
            test_holds_the_lock: false,
            why: "the installed binary does not count",
        },
        Unproven {
            case: "holds the lock and the install path is a link to the program it runs",
            installed: Installed::LinkTo("/bin/sh"),
            runs_installed: false,
            program: "/bin/sh",
            script: HOLDS_THE_LOCK,
            test_holds_the_lock: false,
            why: "is not a regular file",
        },
        Unproven {
            case: "holds the lock and runs a binary in a folder reached through a link",
            installed: Installed::CopyBehindALinkedFolder("/bin/sh"),
            runs_installed: true,
            program: "",
            script: HOLDS_THE_LOCK,
            test_holds_the_lock: false,
            why: "is reached through a link",
        },
    ];
    for unproven in cases {
        let case = unproven.case;
        let home = Home::new();
        let installed = match unproven.installed {
            Installed::Nothing => None,
            Installed::CopyOf(program) => Some(install(&home, program)),
            Installed::LinkTo(program) => {
                let folder = home.path().join(".local/bin");
                std::fs::create_dir_all(&folder).expect("the install folder");
                let link = folder.join("quadratique");
                std::os::unix::fs::symlink(program, &link).expect("a link");
                Some(link)
            }
            Installed::CopyBehindALinkedFolder(program) => {
                let real = home.path().join("real-bin");
                std::fs::create_dir_all(&real).expect("a folder");
                copy(program, &real.join("quadratique"));
                std::fs::create_dir_all(home.path().join(".local")).expect("a folder");
                std::os::unix::fs::symlink(&real, home.path().join(".local/bin"))
                    .expect("a linked folder");
                Some(home.path().join(".local/bin/quadratique"))
            }
        };
        let program = match (&installed, unproven.runs_installed) {
            (Some(installed), true) => installed.clone(),
            _ => PathBuf::from(unproven.program),
        };
        let mut old = LegacyServer::start(
            &home,
            &program,
            unproven.script,
            unproven.test_holds_the_lock,
            &[],
        );
        let pid = old.pid();

        let status = run(&home, &["server", "status"]).ok();
        let stopped = run(&home, &["server", "stop"]);
        let refused = run(&home, &["project", "list"]);

        assert_eq!(status["legacy"], true, "{case}: {status}");
        assert_eq!(status["provenStop"], false, "{case}: {status}");
        let why = unproven.why;
        assert!(
            status["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains(why)),
            "{case}: {status}"
        );
        let what_to_run = [
            "if the user unit `quadratique.service` of an earlier install runs it, stop that unit"
                .to_owned(),
            format!("ps -p {pid} -o args"),
            format!(
                "only if it shows this data folder's Quadratique server, end it with `kill {pid}`"
            ),
        ];
        let stop = status["stop"].as_str().unwrap_or_default();
        assert!(
            what_to_run.iter().all(|command| stop.contains(command)),
            "{case}: {status}"
        );
        assert_eq!(stopped.code, UNAVAILABLE, "{case}: {stopped:?}");
        let stop_refusal = message(&stopped);
        assert_eq!(refused.code, UNAVAILABLE, "{case}: {refused:?}");
        let start_refusal = message(&refused);
        for (refusal, outcome) in [
            (&stop_refusal, "Nothing was stopped."),
            (&start_refusal, "Nothing was started."),
        ] {
            assert!(
                refusal.contains(outcome) && refusal.contains(why),
                "{case}: {refusal}"
            );
            assert!(
                what_to_run.iter().all(|command| refusal.contains(command)),
                "{case}: {refusal}"
            );
        }
        assert!(old.still_runs(), "{case}: the process was signalled");
        assert!(old.requests().is_empty(), "{case}: {:?}", old.requests());
        assert!(home.servers().is_empty(), "{case}: a server was started");
        assert!(
            !home.path().join("start.lock").exists(),
            "{case}: a refusal reserved a start"
        );
    }
}

/// A runtime file that names pid 0 gives no process command: `kill 0` would signal the caller's own process group.
#[test]
fn a_runtime_file_that_names_no_valid_process_gives_no_process_command() {
    let home = Home::new();
    let _lock = hold(&home.path().join("server.lock"));
    write_runtime(&home, 0, "127.0.0.1:9");

    let stopped = run(&home, &["server", "stop"]);

    assert_eq!(stopped.code, UNAVAILABLE, "{stopped:?}");
    let refusal = message(&stopped);
    assert!(
        refusal.contains("otherwise find the process that serves this data folder and end it"),
        "{refusal}"
    );
    assert!(
        !refusal.contains("kill 0") && !refusal.contains("ps -p 0"),
        "{refusal}"
    );
}

/// The last check under `start.lock`: the stop found its proof, then waits for `start.lock`, which the test holds.
/// Meanwhile the old server lets go of the lock and the test takes it, so the process still runs the installed binary
/// and the home still shows the row, but the process no longer holds the lock: the stop signals nothing.
#[test]
fn a_process_that_let_go_of_the_lock_while_the_stop_waited_for_start_lock_gets_no_signal() {
    let home = Home::new();
    let installed = install(&home, "/bin/sh");
    let mut old = LegacyServer::start(&home, &installed, HOLDS_THE_LOCK_UNTIL_TOLD, false, &[]);
    let start_lock = hold(&home.path().join("start.lock"));
    let mut watch = Watch::of(&home);

    let stop = home
        .quadratique(outside().path())
        .args(["server", "stop"])
        .spawn();
    watch.await_steps(&[Step::Opened("start.lock")]);
    old.tell("released");
    let _held_by_the_test = hold(&home.path().join("server.lock"));
    drop(start_lock);
    let stopped = stop.finish();

    assert_eq!(stopped.code, UNAVAILABLE, "{stopped:?}");
    let refusal = message(&stopped);
    assert!(
        refusal.contains("does not hold this home's lock")
            && refusal.contains("Nothing was stopped."),
        "{refusal}"
    );
    assert!(old.still_runs(), "the process was signalled");
}

/// A take-over stops the old server with its proof and holds `start.lock` until its own server is ready. A command
/// that comes while the old server still holds the lock after its SIGTERM is refused at once, as for any server from
/// before the socket; once the old server has gone, a new command uses the take-over's server.
#[test]
fn a_command_during_a_take_over_is_refused_and_one_after_it_uses_the_new_server() {
    let home = Home::new();
    let elsewhere = outside();
    let signaled = elsewhere.path().join("signaled");
    let installed = install(&home, "/bin/sh");
    let old = LegacyServer::start(
        &home,
        &installed,
        HOLDS_THE_LOCK_IGNORING_SIGTERM,
        false,
        &[&signaled],
    );

    let mut take_over = home
        .quadratique(elsewhere.path())
        .args(["serve", "--take-over"])
        .spawn();
    let new_owner = take_over.id();
    await_until("the take-over's SIGTERM to the old server", || {
        signaled.exists()
    });
    let start_lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.path().join("start.lock"))
        .expect("the start lock");
    assert!(
        matches!(start_lock.try_lock(), Err(TryLockError::WouldBlock)),
        "a command could start a server while the take-over stops the old server"
    );
    drop(start_lock);
    let during = home.quadratique(elsewhere.path()).args(["ui"]).run();
    assert_eq!(during.code, UNAVAILABLE, "{during:?}");
    assert!(
        message(&during).contains("Nothing was started."),
        "{during:?}"
    );
    kill(old.pid());
    await_until("the take-over's server", || {
        home.served_pid() == Some(new_owner)
    });

    let after = home.quadratique(elsewhere.path()).args(["ui"]).run().ok();
    let served = home.served().expect("the new server answers");
    assert_eq!(after["url"], served["web"], "{after}");
    assert_eq!(home.servers(), vec![new_owner]);
}

/// A stop that first saw a server on the socket stops that generation and nothing else: when, by the time it holds
/// `start.lock`, the socket is gone and the home shows a server from before the socket, it refuses, even with the proof.
#[test]
fn a_stop_that_meant_a_generation_signals_no_server_from_before_the_socket() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    let home = Home::new();
    let installed = install(&home, "/bin/sh");
    let mut old = LegacyServer::start(&home, &installed, HOLDS_THE_LOCK, false, &[]);
    let socket = home.path().join("server.sock");
    let listener = UnixListener::bind(&socket).expect("a socket");
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).expect("mode 0600");
    let identity = json!({
        "pid": old.pid(),
        "database": home.path().canonicalize().expect("a real path").join("workspace.db"),
        "interface": quadratique_server::app::interface(),
        "schema": 1,
        "generation": "0123456789abcdef0123456789abcdef",
        "control": 1,
        "controls": [1],
        "started": "serve",
        "url": "http://127.0.0.1:9/cli",
    })
    .to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().expect("a stream"));
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                line.clear();
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{identity}",
                identity.len()
            );
        }
    });
    let start_lock = hold(&home.path().join("start.lock"));
    let mut watch = Watch::of(&home);

    let stop = home
        .quadratique(outside().path())
        .args(["server", "stop"])
        .spawn();
    watch.await_steps(&[Step::Opened("start.lock")]);
    std::fs::remove_file(&socket).expect("the socket goes");
    drop(start_lock);
    let stopped = stop.finish();

    assert_eq!(stopped.code, UNAVAILABLE, "{stopped:?}");
    let refusal = message(&stopped);
    assert!(
        refusal.contains("no longer generation 0123456789abcdef0123456789abcdef")
            && refusal.contains("Nothing was stopped."),
        "{refusal}"
    );
    assert!(old.still_runs(), "the process was signalled");
}
