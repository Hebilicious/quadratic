use std::fs::{File, TryLockError};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::support::{Home, await_until, kill, outside};

const UNAVAILABLE: i32 = 4;

struct OtherBuild {
    pid: u32,
    requests: Arc<Mutex<Vec<String>>>,
}

impl OtherBuild {
    fn start(home: &Home, interface: Option<&str>) -> OtherBuild {
        OtherBuild::start_as(home, interface, Command::new("cat"), None)
    }

    fn start_as(
        home: &Home,
        interface: Option<&str>,
        mut process: Command,
        stops_answering_once_it_exists: Option<PathBuf>,
    ) -> OtherBuild {
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(home.path().join("server.lock"))
            .expect("the lock file");
        lock.lock().expect("the other build holds the lock");
        let mut process = process
            .env_clear()
            .env("HOME", home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("cat starts");
        let pid = process.id();
        let input = process.stdin.take();
        std::thread::spawn(move || {
            let _ = process.wait();
            drop(input);
            drop(lock);
        });

        let database = home
            .path()
            .canonicalize()
            .expect("a real path")
            .join("workspace.db");
        let mut identity = json!({ "pid": pid, "database": database });
        if let Some(interface) = interface {
            identity["interface"] = json!(interface);
        }
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
                let silent = stops_answering_once_it_exists
                    .as_ref()
                    .is_some_and(|file| file.exists());
                let (status, body) = if first.starts_with("GET /server ") && !silent {
                    ("200 OK", body)
                } else {
                    ("404 Not Found", String::new())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
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
        OtherBuild { pid, requests }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("the request log").clone()
    }
}

#[test]
fn a_command_that_meets_a_server_of_another_interface_fails_says_how_to_recover_and_calls_nothing()
{
    for interface in [Some("fnv1a64:0000000000000000"), None] {
        let home = Home::new();
        let elsewhere = outside();
        let other = OtherBuild::start(&home, interface);

        let refused = home
            .quadratique(elsewhere.path())
            .args(["show", "--issue", "DEM-1"])
            .run();

        assert_eq!(refused.code, UNAVAILABLE, "{interface:?}: {refused:?}");
        assert!(
            refused.stderr.contains("quadratique server stop"),
            "{interface:?}: {refused:?}"
        );
        let requests = other.requests();
        assert!(
            !requests.is_empty(),
            "{interface:?}: the identity was not asked"
        );
        assert!(
            requests
                .iter()
                .all(|request| request.starts_with("GET /server ")),
            "{interface:?}: {requests:?}"
        );
    }
}

#[test]
fn server_status_and_stop_reach_a_server_of_another_interface_and_the_next_command_starts_this_build()
 {
    let home = Home::new();
    let elsewhere = outside();
    let other = OtherBuild::start(&home, None);
    let run = |args: &[&str]| home.quadratique(elsewhere.path()).args(args).run();

    let status = run(&["server", "status"]).ok();
    assert_eq!(status["running"], true, "{status}");
    assert_eq!(status["pid"], other.pid, "{status}");
    assert_eq!(status["sameInterface"], false, "{status}");

    let stopped = run(&["server", "stop"]).ok();
    assert_eq!(stopped["stopped"], true, "{stopped}");
    assert_eq!(stopped["pid"], other.pid, "{stopped}");

    let projects: Value = run(&["project", "list"]).ok();
    assert_eq!(projects, json!({ "items": [] }));
    let status = run(&["server", "status"]).ok();
    assert_eq!(status["sameInterface"], true, "{status}");
    assert_ne!(status["pid"], other.pid, "{status}");
}

#[test]
fn a_command_during_a_take_over_waits_for_the_new_server_and_uses_it() {
    let home = Home::new();
    let elsewhere = outside();
    let signaled = elsewhere.path().join("signaled");
    let mut ignores_sigterm = Command::new("sh");
    ignores_sigterm.args([
        "-c",
        "trap 'touch \"$0\"' TERM; while :; do tail -f /dev/null & wait $!; done",
        &signaled.display().to_string(),
    ]);
    let interface = quadratique_server::app::interface();
    let old = OtherBuild::start_as(
        &home,
        Some(&interface),
        ignores_sigterm,
        Some(signaled.clone()),
    );

    let mut take_over = home
        .quadratique(elsewhere.path())
        .args(["serve", "--take-over"])
        .spawn();
    let new_owner = take_over.id();
    await_until("the take-over's SIGTERM to the owner", || signaled.exists());
    let during = home.quadratique(elsewhere.path()).args(["ui"]).spawn();
    let start_lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.path().join("start.lock"))
        .expect("the start lock");
    assert!(
        matches!(start_lock.try_lock(), Err(TryLockError::WouldBlock)),
        "a command could start a server while the take-over stops the owner"
    );
    drop(start_lock);
    kill(old.pid);

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
