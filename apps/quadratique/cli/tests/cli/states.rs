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
use std::sync::{Arc, Mutex, mpsc};

use serde_json::{Value, json};

use crate::support::{
    HANG_GUARD, Home, Process, Run, Step, Watch, await_exit, free, outside, stop_listening,
};

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

/// What a [`Fake`] does with a stop of its own generation.
enum OnStop {
    /// Answers `202`, stops listening and lets go of the home's lock, as a real server does once it has shut down.
    Release,
    /// Reads the whole request, stops listening, lets go of the lock, then closes the connection without an answer: a
    /// server that carried out the stop and lost its answer.
    LoseAnswer,
    /// Answers `202` and stops listening, then keeps the lock until the test sends on the channel.
    HoldUntil(mpsc::Receiver<()>),
    /// Answers with this status, as a build between the socket and the stop request does with `405`, and keeps serving.
    Answer(&'static str),
}

/// A fake server on the socket: it answers `GET /server` with `answer`, a stop of the generation in `answer` as
/// [`OnStop`] says, and a stop of another generation with `409`. It records the first line of each request, and says
/// on `posted` when a stop request has arrived.
struct Fake {
    requests: Arc<Mutex<Vec<String>>>,
    posted: mpsc::Receiver<()>,
}

impl Fake {
    fn start(listener: UnixListener, answer: String, lock: Option<File>) -> Fake {
        Fake::start_with(listener, answer, lock, OnStop::Release)
    }

    fn start_with(
        listener: UnixListener,
        answer: String,
        mut lock: Option<File>,
        on_stop: OnStop,
    ) -> Fake {
        let generation = answer
            .split_once("\r\n\r\n")
            .and_then(|(_, body)| serde_json::from_str::<Value>(body).ok())
            .and_then(|identity| identity["generation"].as_str().map(str::to_owned))
            .unwrap_or_else(|| GENERATION.to_owned());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let (post, posted) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let (first, body) = read_request(&stream);
                recorded
                    .lock()
                    .expect("the request log")
                    .push(first.clone());
                let mut stream = stream;
                if first.starts_with("POST /server/pair ") {
                    // As a server answers `ui`: the address of its web app, and a link with a code.
                    let paired = json!({
                        "url": "http://127.0.0.1:9/",
                        "link": "http://127.0.0.1:9/#pair=code",
                        "file": null,
                    });
                    let _ = stream.write_all(http("200 OK", &paired.to_string()).as_bytes());
                    continue;
                }
                if !first.starts_with("POST /server/stop ") {
                    let _ = stream.write_all(answer.as_bytes());
                    continue;
                }
                let _ = post.send(());
                let asked: Value = serde_json::from_str(&body).unwrap_or_default();
                let named = json!({ "generation": generation }).to_string();
                if asked["generation"] != generation.as_str() {
                    let _ = stream.write_all(http("409 Conflict", &named).as_bytes());
                    continue;
                }
                if let OnStop::Answer(status) = on_stop {
                    let _ = stream.write_all(http(status, "").as_bytes());
                    continue;
                }
                if matches!(on_stop, OnStop::LoseAnswer) {
                    // The home is free before the client reads the end of its connection, so the client sees it free.
                    stop_listening(listener);
                    if let Some(lock) = lock.take() {
                        free(lock);
                    }
                    drop(stream);
                    return;
                }
                let _ = stream.write_all(http("202 Accepted", &named).as_bytes());
                drop(stream);
                stop_listening(listener);
                if let OnStop::HoldUntil(release) = on_stop {
                    let _ = release.recv();
                }
                if let Some(lock) = lock.take() {
                    free(lock);
                }
                return;
            }
        });
        Fake { requests, posted }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("the request log").clone()
    }

    /// Returns once a stop request has reached the fake.
    fn await_stop_request(&self) {
        self.posted
            .recv_timeout(HANG_GUARD)
            .expect("a stop request within the hang guard");
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
    stop_listening(fake_socket(&home));
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

    stop_listening(fake_socket(&home));
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
    // The identity, the pairing, and the identity again for whether the address is kept.
    assert_eq!(
        fake.requests(),
        [
            "GET /server HTTP/1.1",
            "POST /server/pair HTTP/1.1",
            "GET /server HTTP/1.1"
        ]
    );
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
/// for the home to be free, then serves with a new generation.
#[test]
fn row_a_server_of_this_build_is_used_stopped_by_its_generation_and_taken_over_by_a_new_generation()
{
    let home = Home::new();
    run(&home, &["project", "list"]).ok();
    let old = status(&home);
    let old_pid = old["pid"].as_u64().expect("a pid") as u32;
    run(&home, &["project", "list"]).ok();
    assert_eq!(home.servers(), vec![old_pid]);

    let mut watch = Watch::of(&home);
    let mut take_over = spawn(&home, &["serve", "--take-over"]);
    let new_pid = served_by(&mut watch, &mut take_over);

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
    assert_eq!(home.servers(), vec![new_pid]);
    stop_real(&home, new_pid);
}

/// The handoff of a take-over, held at a step the test controls: the old server has accepted the stop and still
/// holds the home. The take-over keeps `start.lock` through it, so a `serve` started then waits for the lock, and
/// once the take-over's server is ready, finds the home owned. A command started then uses the new server.
#[test]
fn a_take_over_hands_off_with_no_other_starter_in_between() {
    let home = Home::new();
    let held = hold_the_home(&home);
    let (release, released) = mpsc::channel();
    let old = Fake::start_with(
        fake_socket(&home),
        http("200 OK", &this_build(&home).to_string()),
        Some(held),
        OnStop::HoldUntil(released),
    );
    let mut watch = Watch::of(&home);
    let mut take_over = spawn(&home, &["serve", "--take-over"]);
    old.await_stop_request();
    assert!(
        !start_lock_is_free(&home),
        "a command could start a server while the take-over hands the home over"
    );

    watch.clear();
    let contender = spawn(&home, &["serve"]);
    watch.await_steps(&[Step::Opened("start.lock")]);
    let during = spawn(&home, &["ui"]);
    release.send(()).expect("the old server lets go");
    let new_pid = served_by(&mut watch, &mut take_over);

    let refused = contender.finish();
    assert_eq!(refused.code, UNAVAILABLE, "{refused:?}");
    assert!(
        message(&refused).contains("another server owns the database"),
        "{refused:?}"
    );
    let answered = during.finish().ok();
    let new = status(&home);
    assert_eq!(new["pid"], new_pid, "{new}");
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

/// A stop that saw generation A, then waited for `start.lock` while a handoff published B, stops nothing: it says
/// which generation it meant and which serves.
#[test]
fn a_stop_that_waited_for_the_start_lock_refuses_a_generation_it_did_not_see() {
    const SUCCESSOR: &str = "fedcba9876543210fedcba9876543210";
    let home = Home::new();
    let held = hold_the_home(&home);
    let handoff = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.path().join("start.lock"))
        .expect("the start lock");
    handoff.lock().expect("a handoff holds the start lock");
    let first = Fake::start(
        fake_socket(&home),
        http("200 OK", &this_build(&home).to_string()),
        None,
    );
    let mut watch = Watch::of(&home);
    let stop = spawn(&home, &["server", "stop"]);
    watch.await_steps(&[Step::Opened("start.lock")]);
    std::fs::remove_file(home.path().join("server.sock")).expect("the first socket goes");
    let mut successor = this_build(&home);
    successor["generation"] = json!(SUCCESSOR);
    let second = Fake::start(
        fake_socket(&home),
        http("200 OK", &successor.to_string()),
        Some(held),
    );
    drop(handoff);

    let refused = stop.finish();

    assert_eq!(refused.code, UNAVAILABLE, "{refused:?}");
    let message = message(&refused);
    assert!(
        message.contains(GENERATION)
            && message.contains(SUCCESSOR)
            && message.contains("Nothing was stopped."),
        "{message}"
    );
    assert_eq!(first.requests(), ["GET /server HTTP/1.1"]);
    assert!(
        second
            .requests()
            .iter()
            .all(|request| request.starts_with("GET /server ")),
        "{:?}",
        second.requests()
    );
}

/// A newer server is checked for its database and its generation before its control versions: another database is
/// refused even when it lists this control version, and an identity without a generation is malformed.
#[test]
fn a_newer_server_of_another_database_or_without_a_generation_is_never_stopped() {
    let home = Home::new();
    let _held = hold_the_home(&home);
    let interface = quadratique_server::app::interface();
    let mut elsewhere = identity(&home, &interface, 2, &[1, 2]);
    elsewhere["database"] = json!("/elsewhere/workspace.db");
    let other_database = Fake::start(
        fake_socket(&home),
        http("200 OK", &elsewhere.to_string()),
        None,
    );

    assert_eq!(status(&home)["otherDatabase"], "/elsewhere/workspace.db");
    let refused = run(&home, &["server", "stop"]);
    assert_eq!(refused.code, UNAVAILABLE, "{refused:?}");
    assert!(
        message(&refused).contains("serves another database"),
        "{refused:?}"
    );
    assert!(
        other_database
            .requests()
            .iter()
            .all(|request| request.starts_with("GET /server ")),
        "{:?}",
        other_database.requests()
    );
    std::fs::remove_file(home.path().join("server.sock")).expect("the fake socket goes");

    let mut nameless = identity(&home, &interface, 2, &[1, 2]);
    nameless
        .as_object_mut()
        .expect("an object")
        .remove("generation");
    let without_generation = Fake::start(
        fake_socket(&home),
        http("200 OK", &nameless.to_string()),
        None,
    );

    assert_eq!(status(&home)["malformed"], true);
    let refused = run(&home, &["server", "stop"]);
    assert_eq!(refused.code, UNAVAILABLE, "{refused:?}");
    assert!(message(&refused).contains("generation"), "{refused:?}");
    assert!(
        without_generation
            .requests()
            .iter()
            .all(|request| request.starts_with("GET /server ")),
        "{:?}",
        without_generation.requests()
    );
}

/// The server read the whole stop request and closed without an answer, then let go of the home: the stop is not
/// confirmed, which is neither "nothing was running" nor "could not connect".
#[test]
fn a_stop_whose_answer_is_lost_says_it_is_not_confirmed_and_what_the_lock_shows() {
    let home = Home::new();
    let held = hold_the_home(&home);
    let lost = Fake::start_with(
        fake_socket(&home),
        http("200 OK", &this_build(&home).to_string()),
        Some(held),
        OnStop::LoseAnswer,
    );

    let stopped = run(&home, &["server", "stop"]);

    assert_eq!(stopped.code, UNAVAILABLE, "{stopped:?}");
    let message = message(&stopped);
    assert!(
        message.contains(&format!(
            "the stop of generation {GENERATION} is not confirmed"
        )) && message.contains("the answer was lost")
            && message.contains("lock is free"),
        "{message}"
    );
    assert_eq!(
        lost.requests().last().map(String::as_str),
        Some("POST /server/stop HTTP/1.1")
    );
}

/// A server that answers the stop request with another status than `202` or `409` is named with how to end it by hand,
/// with the pid it gave, and nothing is stopped. Only a `405` gets the note about a build between the socket and the
/// stop request (pull requests 34 and 36 of main, never released).
#[test]
fn a_server_that_does_not_serve_the_stop_request_is_named_with_how_to_end_it() {
    for (status, historical) in [
        ("405 Method Not Allowed", true),
        ("503 Service Unavailable", false),
    ] {
        let home = Home::new();
        let held = hold_the_home(&home);
        let serving = Fake::start_with(
            fake_socket(&home),
            http("200 OK", &this_build(&home).to_string()),
            Some(held),
            OnStop::Answer(status),
        );

        let stopped = run(&home, &["server", "stop"]);

        assert_eq!(stopped.code, UNAVAILABLE, "{status}: {stopped:?}");
        let message = message(&stopped);
        let code = &status[..3];
        for expected in [
            &format!("the stop request got status {code}"),
            "Nothing was stopped.",
            "End it yourself, then run the command again",
            "Ctrl-C",
            &format!("ps -p {FAKE_PID} -o args"),
        ] {
            assert!(message.contains(expected), "{expected}: {message}");
        }
        assert_eq!(
            message.contains("between the socket and the stop request"),
            historical,
            "{message}"
        );
        assert_eq!(
            serving.requests().last().map(String::as_str),
            Some("POST /server/stop HTTP/1.1")
        );
    }
}

/// A lock that cannot be looked at is the row "reachable: false" with a reason that names it: status answers, and no
/// command starts or stops anything.
#[test]
fn a_lock_path_that_cannot_be_used_is_unreachable_and_named() {
    let home = Home::new();
    let lock = home.path().join("server.lock");
    std::fs::create_dir(&lock).expect("a folder where the lock should be");

    let status = status(&home);
    let ensured = run(&home, &["project", "list"]);
    let stopped = run(&home, &["server", "stop"]);

    assert_eq!(status["reachable"], false, "{status}");
    let reason = status["reason"].as_str().expect("a reason");
    let lock = lock.canonicalize().expect("the lock's path");
    assert!(reason.contains(&lock.display().to_string()), "{reason}");
    assert_eq!(ensured.code, UNAVAILABLE, "{ensured:?}");
    assert!(
        message(&ensured).contains("Nothing was started"),
        "{ensured:?}"
    );
    assert_eq!(stopped.code, UNAVAILABLE, "{stopped:?}");
    assert!(
        message(&stopped).contains("Nothing was stopped"),
        "{stopped:?}"
    );
    assert!(home.servers().is_empty());
}
