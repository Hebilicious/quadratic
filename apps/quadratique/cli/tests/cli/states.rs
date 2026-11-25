//! One test per row of the state table in docs/decisions/0004-a-socket-for-commands-and-a-token-for-the-browser.md,
//! §2: what `server status`, a command that needs the server ("ensure"), `server stop` and `serve --take-over` do in
//! each state. Fake servers on the socket's name, and a lock the test holds, make each state; a [`Watch`] on the home
//! tells the test when a command has looked at the home, so no test sleeps to wait for a state.
//!
//! Two rows have their tests elsewhere: two starters at once (`server_life.rs`,
//! `commands_that_start_together_in_a_fresh_home_share_one_server`), and a server of a build before the socket
//! (`other_build.rs`).

use std::fs::{File, TryLockError};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::support::{Home, Process, Run, Step, Watch, await_exit, outside};

const UNAVAILABLE: i32 = 4;
const GENERATION: &str = "0123456789abcdef0123456789abcdef";
const FAKE_PID: u32 = 4242;

fn run(home: &Home, args: &[&str]) -> Run {
    home.quadratique(outside().path()).args(args).run()
}

fn spawn(home: &Home, args: &[&str]) -> Process {
    home.quadratique(outside().path()).args(args).spawn()
}

fn status(home: &Home) -> Value {
    run(home, &["server", "status"]).ok()
}

fn message(refused: &Run) -> String {
    let failure: Value = serde_json::from_str(refused.stderr.lines().last().unwrap_or_default())
        .unwrap_or_else(|error| panic!("standard error is not JSON ({error}): {refused:?}"));
    failure["message"].as_str().unwrap_or_default().to_owned()
}

fn hold_the_home(home: &Home) -> File {
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.path().join("server.lock"))
        .expect("the home's lock file");
    lock.lock().expect("a server of the home holds its lock");
    lock
}

fn start_lock_is_free(home: &Home) -> bool {
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.path().join("start.lock"))
        .expect("the start lock");
    !matches!(lock.try_lock(), Err(TryLockError::WouldBlock))
}

fn database_of(home: &Home) -> PathBuf {
    home.path()
        .canonicalize()
        .expect("a real path")
        .join("workspace.db")
}

fn identity(home: &Home, interface: &str, control: u32, controls: &[u32]) -> Value {
    json!({
        "pid": FAKE_PID,
        "database": database_of(home),
        "interface": interface,
        "schema": 1,
        "generation": GENERATION,
        "control": control,
        "controls": controls,
        "started": "serve",
        "url": "http://127.0.0.1:9/cli",
    })
}

fn this_build(home: &Home) -> Value {
    identity(home, &quadratique_server::app::interface(), 1, &[1])
}

fn another_build(home: &Home) -> Value {
    identity(home, "fnv1a64:0000000000000000", 1, &[1])
}

/// A socket of this account with mode 0600 at the home's socket name.
fn fake_socket(home: &Home) -> UnixListener {
    let path = home.path().join("server.sock");
    let listener = UnixListener::bind(&path).expect("a fake server's socket");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("mode 0600");
    listener
}

fn http(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A fake server on the socket: it answers `GET /server` with `answer`, and a stop of `GENERATION` with `202`, after
/// which it stops listening and lets go of the home's lock, as a real server does once it has shut down. It records
/// the first line of each request.
struct Fake {
    requests: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn start(listener: UnixListener, answer: String, mut lock: Option<File>) -> Fake {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let (first, body) = read_request(&stream);
                recorded
                    .lock()
                    .expect("the request log")
                    .push(first.clone());
                let mut stream = stream;
                if first.starts_with("POST /server/stop ") {
                    let asked: Value = serde_json::from_str(&body).unwrap_or_default();
                    let answer = json!({ "generation": GENERATION }).to_string();
                    if asked["generation"] == GENERATION {
                        let _ = stream.write_all(http("202 Accepted", &answer).as_bytes());
                        drop(stream);
                        drop(listener);
                        drop(lock.take());
                        return;
                    }
                    let _ = stream.write_all(http("409 Conflict", &answer).as_bytes());
                } else {
                    let _ = stream.write_all(answer.as_bytes());
                }
            }
        });
        Fake { requests }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("the request log").clone()
    }
}

fn read_request(stream: &UnixStream) -> (String, String) {
    let mut reader = BufReader::new(stream);
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
    (
        first.trim().to_owned(),
        String::from_utf8_lossy(&body).into_owned(),
    )
}

/// Nothing was spawned and no start was reserved: no log, no start lock, no server process.
fn nothing_started(home: &Home) {
    assert!(
        !home.path().join("server.log").exists(),
        "a server was spawned"
    );
    assert!(
        !home.path().join("start.lock").exists(),
        "the start lock was taken"
    );
    assert!(home.servers().is_empty());
}

/// Waits until the server that `process` runs has published its runtime file, then returns its pid.
fn served_by(watch: &mut Watch, process: &mut Process) -> u32 {
    watch.await_steps(&[Step::Published("server.json")]);
    process.id()
}

fn stop_real(home: &Home, pid: u32) {
    let stopped = run(home, &["server", "stop"]).ok();
    assert_eq!(stopped["stopped"], true, "{stopped}");
    assert_eq!(stopped["pid"], pid, "{stopped}");
    await_exit(pid);
}

#[test]
fn row_no_socket_and_a_free_lock_status_is_not_running_stop_finds_nothing_ensure_and_take_over_serve()
 {
    let home = Home::new();
    assert_eq!(status(&home)["running"], false);
    let stopped = run(&home, &["server", "stop"]).ok();
    assert_eq!(stopped["stopped"], false, "{stopped}");
    assert!(
        !home.path().join("start.lock").exists(),
        "a stop of a free home reserved a start"
    );

    let mut watch = Watch::of(&home);
    let mut take_over = spawn(&home, &["serve", "--take-over"]);
    let pid = served_by(&mut watch, &mut take_over);
    let serving = status(&home);
    assert_eq!(
        (&serving["running"], &serving["pid"], &serving["started"]),
        (&json!(true), &json!(pid), &json!("serve")),
        "{serving}"
    );
    stop_real(&home, pid);

    run(&home, &["project", "list"]).ok();
    let on_demand = status(&home);
    assert_eq!(on_demand["started"], "on-demand", "{on_demand}");
}

#[test]
fn row_a_stale_socket_and_a_free_lock_is_not_running_and_the_next_server_replaces_the_file() {
    let home = Home::new();
    let socket = home.path().join("server.sock");
    let stale = fake_socket(&home);
    drop(stale);
    let refused = UnixStream::connect(&socket);
    assert_eq!(
        refused.as_ref().err().map(std::io::Error::kind),
        Some(std::io::ErrorKind::ConnectionRefused),
        "{refused:?}"
    );
    assert_eq!(status(&home)["running"], false);
    assert_eq!(run(&home, &["server", "stop"]).ok()["stopped"], false);

    run(&home, &["project", "list"]).ok();

    // A file system may give the new socket the inode number of the stale one, so the proof of the replacement is
    // that the name now reaches a listener.
    UnixStream::connect(&socket).expect("the new server's socket accepts");
    let serving = status(&home);
    assert_eq!(serving["running"], true, "{serving}");
    let pid = serving["pid"].as_u64().expect("a pid") as u32;
    stop_real(&home, pid);

    drop(fake_socket(&home));
    let mut watch = Watch::of(&home);
    let mut take_over = spawn(&home, &["serve", "--take-over"]);
    let pid = served_by(&mut watch, &mut take_over);
    assert_eq!(status(&home)["pid"], pid);
    stop_real(&home, pid);
}

/// The row "socket refuses or is missing, lock held, no runtime file of a build before the socket": a server that
/// has not bound its socket yet, or one that is stopping. A runtime file that a socket server wrote (it has `control`)
/// is no sign of a build before the socket.
#[test]
fn row_a_held_lock_without_a_socket_is_starting_and_each_command_waits_for_ready_or_for_the_lock_to_be_freed()
 {
    let home = Home::new();

    let held = hold_the_home(&home);
    let starting = status(&home);
    assert_eq!(
        (&starting["running"], &starting["starting"]),
        (&json!(false), &json!(true)),
        "{starting}"
    );
    let runtime_of_a_socket_server = json!({
        "pid": FAKE_PID,
        "address": "127.0.0.1:9",
        "url": "http://127.0.0.1:9/cli",
        "database": database_of(&home),
        "control": 1,
    });
    std::fs::write(home.runtime_path(), runtime_of_a_socket_server.to_string())
        .expect("a runtime file");
    let still_starting = status(&home);
    assert_eq!(still_starting["starting"], true, "{still_starting}");
    assert!(still_starting.get("legacy").is_none(), "{still_starting}");
    std::fs::remove_file(home.runtime_path()).expect("the runtime file goes");

    // ensure, then ready: it uses the server that answers, and starts nothing.
    let mut watch = Watch::of(&home);
    let ui = spawn(&home, &["ui"]);
    watch.await_steps(&[Step::Opened("server.lock")]);
    let fake = Fake::start(
        fake_socket(&home),
        http("200 OK", &this_build(&home).to_string()),
        None,
    );
    let answered = ui.finish().ok();
    assert_eq!(answered["url"], "http://127.0.0.1:9/", "{answered}");
    assert_eq!(fake.requests(), ["GET /server HTTP/1.1"]);
    nothing_started(&home);
    std::fs::remove_file(home.path().join("server.sock")).expect("the fake socket goes");

    // ensure, then freed: back to the first row under the start lock.
    watch.clear();
    let ui = spawn(&home, &["ui"]);
    watch.await_steps(&[Step::Opened("server.lock")]);
    drop(held);
    let started = ui.finish().ok();
    let serving = status(&home);
    assert_eq!(
        format!(
            "{}/",
            serving["url"]
                .as_str()
                .expect("a url")
                .trim_end_matches("/cli")
        ),
        started["url"].as_str().expect("a url"),
        "{serving}"
    );
    let pid = serving["pid"].as_u64().expect("a pid") as u32;
    stop_real(&home, pid);

    // stop, then freed: nothing to stop.
    let held = hold_the_home(&home);
    watch.clear();
    let stop = spawn(&home, &["server", "stop"]);
    watch.await_steps(&[Step::Opened("start.lock"), Step::Opened("server.lock")]);
    drop(held);
    assert_eq!(stop.finish().ok()["stopped"], false);

    // stop, then ready: it stops the generation that answers.
    let held = hold_the_home(&home);
    watch.clear();
    let stop = spawn(&home, &["server", "stop"]);
    watch.await_steps(&[Step::Opened("start.lock"), Step::Opened("server.lock")]);
    let fake = Fake::start(
        fake_socket(&home),
        http("200 OK", &this_build(&home).to_string()),
        Some(held),
    );
    let stopped = stop.finish().ok();
    assert_eq!(
        (&stopped["stopped"], &stopped["generation"], &stopped["pid"]),
        (&json!(true), &json!(GENERATION), &json!(FAKE_PID)),
        "{stopped}"
    );
    assert_eq!(
        fake.requests(),
        ["GET /server HTTP/1.1", "POST /server/stop HTTP/1.1"]
    );
    std::fs::remove_file(home.path().join("server.sock")).expect("the fake socket goes");

    // take-over: as for stop, then serve.
    let held = hold_the_home(&home);
    watch.clear();
    let mut take_over = spawn(&home, &["serve", "--take-over"]);
    watch.await_steps(&[Step::Opened("start.lock"), Step::Opened("server.lock")]);
    drop(held);
    let pid = served_by(&mut watch, &mut take_over);
    assert_eq!(status(&home)["pid"], pid);
    stop_real(&home, pid);
}

/// The socket accepts and never answers. Each command waits the probe limit, five seconds, once.
#[test]
fn row_a_stalled_socket_is_never_replaced_and_its_message_says_how_to_recover() {
    let home = Home::new();
    let _never_answers = fake_socket(&home);
    let _held = hold_the_home(&home);

    assert_eq!(status(&home)["stalled"], true);
    let ensured = run(&home, &["project", "list"]);
    let stopped = run(&home, &["server", "stop"]);
    let taken_over = run(&home, &["serve", "--take-over"]);

    for (refused, outcome) in [
        (&ensured, "Nothing was started."),
        (&stopped, "Nothing was stopped."),
        (&taken_over, "Nothing was started."),
    ] {
        assert_eq!(refused.code, UNAVAILABLE, "{refused:?}");
        let message = message(refused);
        assert!(
            message.contains("gave no full answer")
                && message.contains(outcome)
                && message.contains("Ctrl-C"),
            "{message}"
        );
    }
    assert!(!home.path().join("server.log").exists());
    assert!(home.servers().is_empty());
}

#[test]
fn row_a_malformed_identity_is_never_replaced_nor_stopped() {
    let home = Home::new();
    let _held = hold_the_home(&home);
    let fake = Fake::start(fake_socket(&home), http("200 OK", "not json"), None);

    assert_eq!(status(&home)["malformed"], true);
    let ensured = run(&home, &["project", "list"]);
    let stopped = run(&home, &["server", "stop"]);
    let taken_over = run(&home, &["serve", "--take-over"]);

    for refused in [&ensured, &stopped, &taken_over] {
        assert_eq!(refused.code, UNAVAILABLE, "{refused:?}");
        assert!(
            message(refused).contains("is not a Quadratique server"),
            "{refused:?}"
        );
    }
    assert!(message(&stopped).ends_with("`quadratique server status`."));
    assert!(
        fake.requests()
            .iter()
            .all(|request| request.starts_with("GET /server ")),
        "{:?}",
        fake.requests()
    );
    assert!(!home.path().join("server.log").exists());
    assert!(home.servers().is_empty());
}

#[test]
fn row_a_server_of_another_database_is_never_replaced_nor_stopped() {
    let home = Home::new();
    let _held = hold_the_home(&home);
    let mut elsewhere = this_build(&home);
    elsewhere["database"] = json!("/elsewhere/workspace.db");
    let fake = Fake::start(
        fake_socket(&home),
        http("200 OK", &elsewhere.to_string()),
        None,
    );

    assert_eq!(status(&home)["otherDatabase"], "/elsewhere/workspace.db");
    let ensured = run(&home, &["project", "list"]);
    let stopped = run(&home, &["server", "stop"]);
    let taken_over = run(&home, &["serve", "--take-over"]);

    for refused in [&ensured, &stopped, &taken_over] {
        assert_eq!(refused.code, UNAVAILABLE, "{refused:?}");
        assert!(
            message(refused).contains("serves another database, /elsewhere/workspace.db"),
            "{refused:?}"
        );
    }
    assert!(
        fake.requests()
            .iter()
            .all(|request| request.starts_with("GET /server ")),
        "{:?}",
        fake.requests()
    );
    assert!(!home.path().join("server.log").exists());
}

/// Same control version, another interface: commands refuse it and name `server stop`, which stops its generation;
/// a take-over stops it, then serves.
#[test]
fn row_a_server_of_another_build_is_refused_by_commands_and_stopped_by_its_generation() {
    let home = Home::new();
    let held = hold_the_home(&home);
    let fake = Fake::start(
        fake_socket(&home),
        http("200 OK", &another_build(&home).to_string()),
        Some(held),
    );

    assert_eq!(status(&home)["sameInterface"], false);
    let ensured = run(&home, &["project", "list"]);
    assert_eq!(ensured.code, UNAVAILABLE, "{ensured:?}");
    assert!(
        message(&ensured).contains("Run `quadratique server stop`"),
        "{ensured:?}"
    );
    nothing_started(&home);

    let stopped = run(&home, &["server", "stop"]).ok();
    assert_eq!(
        (&stopped["stopped"], &stopped["generation"]),
        (&json!(true), &json!(GENERATION)),
        "{stopped}"
    );
    assert_eq!(
        fake.requests().last().map(String::as_str),
        Some("POST /server/stop HTTP/1.1")
    );
    std::fs::remove_file(home.path().join("server.sock")).expect("the fake socket goes");

    let held = hold_the_home(&home);
    let fake = Fake::start(
        fake_socket(&home),
        http("200 OK", &another_build(&home).to_string()),
        Some(held),
    );
    let mut watch = Watch::of(&home);
    let mut take_over = spawn(&home, &["serve", "--take-over"]);
    let pid = served_by(&mut watch, &mut take_over);
    assert_eq!(
        fake.requests().last().map(String::as_str),
        Some("POST /server/stop HTTP/1.1")
    );
    let serving = status(&home);
    assert_eq!(
        (&serving["pid"], &serving["sameInterface"]),
        (&json!(pid), &json!(true)),
        "{serving}"
    );
    stop_real(&home, pid);
}

/// A control version this build does not know: commands say to upgrade; a stop happens only when the server still
/// lists this build's version; a take-over never replaces a newer server with an older one.
#[test]
fn row_a_newer_server_is_stopped_only_when_it_lists_this_control_version_and_never_taken_over() {
    let home = Home::new();
    let held = hold_the_home(&home);
    let interface = quadratique_server::app::interface();
    let unlisted = Fake::start(
        fake_socket(&home),
        http("200 OK", &identity(&home, &interface, 2, &[2]).to_string()),
        None,
    );

    assert_eq!(status(&home)["newer"], true);
    let ensured = run(&home, &["project", "list"]);
    let refused_stop = run(&home, &["server", "stop"]);
    for refused in [&ensured, &refused_stop] {
        assert_eq!(refused.code, UNAVAILABLE, "{refused:?}");
        assert!(
            message(refused).contains("Upgrade this command line"),
            "{refused:?}"
        );
    }
    assert!(
        unlisted
            .requests()
            .iter()
            .all(|request| request.starts_with("GET /server ")),
        "{:?}",
        unlisted.requests()
    );
    nothing_started(&home);
    std::fs::remove_file(home.path().join("server.sock")).expect("the fake socket goes");

    let listed = Fake::start(
        fake_socket(&home),
        http(
            "200 OK",
            &identity(&home, &interface, 2, &[1, 2]).to_string(),
        ),
        Some(held),
    );
    let taken_over = run(&home, &["serve", "--take-over"]);
    assert_eq!(taken_over.code, UNAVAILABLE, "{taken_over:?}");
    assert!(
        message(&taken_over).contains("Nothing was started."),
        "{taken_over:?}"
    );
    let stopped = run(&home, &["server", "stop"]).ok();
    assert_eq!(
        (&stopped["stopped"], &stopped["generation"]),
        (&json!(true), &json!(GENERATION)),
        "{stopped}"
    );
    assert_eq!(
        listed.requests().last().map(String::as_str),
        Some("POST /server/stop HTTP/1.1")
    );
    assert!(home.servers().is_empty());
}

/// The server of this build: commands use it, `server stop` stops its generation, and a take-over stops it, waits
/// for the home to be free while it keeps the start lock, then serves. No other starter gets in between.
#[test]
fn row_a_server_of_this_build_is_used_stopped_by_its_generation_and_handed_over_with_no_starter_in_between()
 {
    let home = Home::new();
    run(&home, &["project", "list"]).ok();
    let old = status(&home);
    let old_pid = old["pid"].as_u64().expect("a pid") as u32;
    run(&home, &["project", "list"]).ok();
    assert_eq!(home.servers(), vec![old_pid]);

    let mut watch = Watch::of(&home);
    let mut take_over = spawn(&home, &["serve", "--take-over"]);
    // The take-over opens the lock only once the old server has stopped listening, so it holds the start lock now.
    watch.await_steps(&[Step::Opened("start.lock"), Step::Opened("server.lock")]);
    assert!(
        !start_lock_is_free(&home),
        "a command could start a server while the take-over hands the home over"
    );
    let during = spawn(&home, &["ui"]);
    let new_pid = served_by(&mut watch, &mut take_over);

    let answered = during.finish().ok();
    await_exit(old_pid);
    let old_log =
        std::fs::read_to_string(home.path().join("server.log")).expect("the old server's log");
    assert!(
        !old_log.contains("could not stop listening"),
        "the stopped server could not close a listener: {old_log}"
    );
    let new = status(&home);
    assert_eq!(
        (&new["pid"], &new["started"]),
        (&json!(new_pid), &json!("serve")),
        "{new}"
    );
    assert_ne!(new["generation"], old["generation"]);
    assert_eq!(
        answered["url"],
        format!(
            "{}/",
            new["url"].as_str().expect("a url").trim_end_matches("/cli")
        ),
        "{answered}"
    );
    assert_eq!(home.servers(), vec![new_pid]);
    stop_real(&home, new_pid);
}
