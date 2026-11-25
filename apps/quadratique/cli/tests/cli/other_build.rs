//! The row of the record's state table (docs/decisions/0004-…, §2) for a server from before the socket, as release
//! 0.1 runs it: the home's lock held, no socket, a runtime file without a control version, and a TCP listener that
//! answers `GET /server`. The command line uses no such server, and stops it only with proof from `/proc` that the
//! process runs the installed binary and holds this home's lock (§3). The fake plays both sides: a process that holds
//! the lock and runs the installed binary, and processes that miss one of the two.

use std::fs::{File, TryLockError};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::support::{Home, Run, await_exit, await_until, kill, outside};

const UNAVAILABLE: i32 = 4;
/// A shell plays the old server: it takes the home's lock on its own descriptor 9 and keeps it, as a server of release
/// 0.1 keeps `server.lock` open, and says so once it holds it.
const SHELL: &str = "/bin/sh";
const HOLDS_THE_LOCK: &str = "exec 9>>\"$0\"; flock -x 9; echo locked; read -r _";
/// The same, ignoring SIGTERM: its children close descriptor 9, so only the shell holds the lock.
const HOLDS_THE_LOCK_IGNORING_SIGTERM: &str = "exec 9>>\"$0\"; flock -x 9; trap 'touch \"$1\"' TERM; echo locked; while :; do tail -f /dev/null 9>&- & wait $!; done";

/// Who holds the home's lock.
enum Holder {
    /// The process itself, on its own descriptor.
    Process { script: &'static str },
    /// The test, while the process only answers for it.
    Test,
}

struct LegacyServer {
    process: Child,
    requests: Arc<Mutex<Vec<String>>>,
    _lock: Option<File>,
}

impl LegacyServer {
    /// Starts the old server's process (`program`, or the shell when the process holds the lock), its TCP listener,
    /// and its runtime file.
    fn start(home: &Home, holder: Holder, program: &str, extra: &[&Path]) -> LegacyServer {
        let lock_path = home.path().join("server.lock");
        let (mut command, lock) = match holder {
            Holder::Process { script } => {
                let mut command = Command::new(SHELL);
                command.arg("-c").arg(script).arg(&lock_path).args(extra);
                (command, None)
            }
            Holder::Test => {
                let lock = File::options()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .open(&lock_path)
                    .expect("the lock file");
                lock.lock().expect("the test holds the lock");
                (Command::new(program), Some(lock))
            }
        };
        let mut process = command
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the old server's process starts");
        if lock.is_none() {
            let mut said = String::new();
            BufReader::new(process.stdout.take().expect("its output"))
                .read_line(&mut said)
                .expect("the old server says it holds the lock");
            assert_eq!(said.trim(), "locked");
        }
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
        let runtime = json!({
            "pid": pid,
            "address": address.to_string(),
            "url": format!("http://{address}/cli"),
            "database": database,
        });
        std::fs::write(home.runtime_path(), runtime.to_string()).expect("the runtime file");
        LegacyServer {
            process,
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
}

impl Drop for LegacyServer {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// Where `install.sh` of release 0.1 put the binary, for a command whose `HOME` is the test home: the old server runs
/// what is installed there.
fn install(home: &Home, program: &str) -> PathBuf {
    let folder = home.path().join(".local/bin");
    std::fs::create_dir_all(&folder).expect("the install folder");
    let installed = folder.join("quadratique");
    std::os::unix::fs::symlink(program, &installed).expect("the installed binary");
    installed
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
    install(&home, SHELL);
    let old = LegacyServer::start(
        &home,
        Holder::Process {
            script: HOLDS_THE_LOCK,
        },
        SHELL,
        &[],
    );
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

/// Without the proof, no process gets a signal, whatever answers on the port: here a process that runs the installed
/// binary and answers for a lock it does not hold, and a process that holds the lock and runs another binary. Each
/// command names what to run instead.
#[test]
fn a_server_from_before_the_socket_without_proof_gets_no_signal_and_the_person_reads_what_to_stop()
{
    let cat = "/bin/cat";
    let cases: [(&str, Holder, &str, Option<&str>, &str); 2] = [
        (
            "answers on the port and holds no lock",
            Holder::Test,
            cat,
            Some(cat),
            "holds no descriptor of this home's lock",
        ),
        (
            "holds the lock and runs another binary",
            Holder::Process {
                script: HOLDS_THE_LOCK,
            },
            SHELL,
            None,
            "which is neither the installed binary",
        ),
    ];
    for (case, holder, program, installed, why) in cases {
        let home = Home::new();
        if let Some(installed) = installed {
            install(&home, installed);
        }
        let mut old = LegacyServer::start(&home, holder, program, &[]);
        let pid = old.pid();

        let status = run(&home, &["server", "status"]).ok();
        let stopped = run(&home, &["server", "stop"]);
        let refused = run(&home, &["project", "list"]);

        assert_eq!(status["legacy"], true, "{case}: {status}");
        assert_eq!(status["provenStop"], false, "{case}: {status}");
        assert!(
            status["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains(why)),
            "{case}: {status}"
        );
        let what_to_run = [
            "systemctl --user stop quadratique.service".to_owned(),
            format!("ps -p {pid} -o args"),
            format!("kill {pid}"),
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
    }
}

/// A take-over stops the old server with its proof and holds `start.lock` until its own server is ready: a command
/// that comes while the old server is still closing waits, and then uses the new server.
#[test]
fn a_command_during_a_take_over_waits_for_the_new_server_and_uses_it() {
    let home = Home::new();
    let elsewhere = outside();
    let signaled = elsewhere.path().join("signaled");
    install(&home, SHELL);
    let old = LegacyServer::start(
        &home,
        Holder::Process {
            script: HOLDS_THE_LOCK_IGNORING_SIGTERM,
        },
        SHELL,
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
    let during = home.quadratique(elsewhere.path()).args(["ui"]).spawn();
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
    kill(old.pid());

    let answered = during.finish().ok();
    let runtime = home.runtime().expect("the new server's runtime file");
    assert_eq!(runtime["pid"], new_owner, "{runtime}");
    let new_url = runtime["url"]
        .as_str()
        .expect("a URL")
        .trim_end_matches("cli")
        .to_owned();
    assert_eq!(answered["url"], new_url, "{answered}");
    assert_eq!(home.servers(), vec![new_owner]);
}
