//! `server status` gives one answer for each state of the socket (docs/decisions/0004-…, §2), from fake servers on
//! the socket's name.

use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;

use serde_json::{Value, json};

use crate::support::{Home, free, outside};

fn status(home: &Home) -> Value {
    home.quadratique(outside().path())
        .args(["server", "status"])
        .run()
        .ok()
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

/// A socket of this account with mode 0600 at the home's socket name.
fn fake_socket(home: &Home) -> UnixListener {
    let path = home.path().join("server.sock");
    let listener = UnixListener::bind(&path).expect("a fake server's socket");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("mode 0600");
    listener
}

/// Answers each connection with `answer`, as many times as asked.
fn answering(listener: UnixListener, answer: String, times: usize) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for _ in 0..times {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(answer.as_bytes());
        }
    })
}

fn http(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn identity(home: &Home, database: &Path, control: u32) -> Value {
    let _ = home;
    json!({
        "pid": 4242,
        "database": database,
        "interface": "fnv1a64:0000000000000000",
        "schema": 1,
        "generation": "0123456789abcdef0123456789abcdef",
        "control": control,
        "started": "serve",
        "url": "http://127.0.0.1:9/cli",
    })
}

fn database_of(home: &Home) -> std::path::PathBuf {
    home.path()
        .canonicalize()
        .expect("a real path")
        .join("workspace.db")
}

#[test]
fn a_socket_that_accepts_and_never_answers_is_stalled() {
    let home = Home::new();
    let _listener = fake_socket(&home);

    let answer = status(&home);

    assert_eq!(answer["stalled"], true, "{answer}");
    assert!(answer.get("running").is_none(), "{answer}");
}

#[test]
fn an_answer_that_is_no_identity_is_malformed_and_never_running() {
    let home = Home::new();
    let listener = fake_socket(&home);
    let mut complete = identity(&home, &database_of(&home), 2);
    complete
        .as_object_mut()
        .expect("an object")
        .remove("generation");
    let serving = answering(listener, http("not json"), 1);
    let first = status(&home);
    serving.join().expect("the fake server");
    std::fs::remove_file(home.path().join("server.sock")).expect("the fake socket goes");
    let listener = fake_socket(&home);
    let serving = answering(listener, http(&complete.to_string()), 1);

    let without_generation = status(&home);

    serving.join().expect("the fake server");
    assert_eq!(first["malformed"], true, "{first}");
    assert_eq!(
        without_generation["malformed"], true,
        "{without_generation}"
    );
    assert!(
        without_generation.get("running").is_none(),
        "{without_generation}"
    );
}

#[test]
fn an_identity_of_another_database_or_of_a_newer_control_version_is_never_running() {
    let home = Home::new();
    let listener = fake_socket(&home);
    let elsewhere = Path::new("/elsewhere/workspace.db");
    let serving = answering(
        listener,
        http(&identity(&home, elsewhere, 2).to_string()),
        1,
    );
    let other = status(&home);
    serving.join().expect("the fake server");
    std::fs::remove_file(home.path().join("server.sock")).expect("the fake socket goes");
    let listener = fake_socket(&home);
    let serving = answering(
        listener,
        http(&identity(&home, &database_of(&home), 3).to_string()),
        1,
    );

    let newer = status(&home);

    serving.join().expect("the fake server");
    assert_eq!(other["otherDatabase"], "/elsewhere/workspace.db", "{other}");
    assert!(other.get("running").is_none(), "{other}");
    assert_eq!(
        (&newer["newer"], &newer["control"]),
        (&json!(true), &json!(3)),
        "{newer}"
    );
    assert!(newer.get("running").is_none(), "{newer}");
}

/// A server sets its socket's mode after `bind`, while its start holds `start.lock`.
#[test]
fn the_lock_tells_a_starting_server_from_a_stopped_home() {
    let home = Home::new();
    assert_eq!(status(&home)["running"], false);

    let held = hold_the_home(&home);
    let starting = status(&home);
    let start = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.path().join("start.lock"))
        .expect("the start lock");
    start
        .lock()
        .expect("a start under way holds the start lock");
    let path = home.path().join("server.sock");
    let socket = UnixListener::bind(&path).expect("a socket not yet private");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660)).expect("mode 0660");
    let not_ready_while_held = status(&home);
    free(start);
    free(held);
    let not_ready_while_free = status(&home);
    drop(socket);

    assert_eq!(
        (&starting["running"], &starting["starting"]),
        (&json!(false), &json!(true)),
        "{starting}"
    );
    assert_eq!(
        not_ready_while_held["starting"], true,
        "{not_ready_while_held}"
    );
    assert!(
        not_ready_while_held["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("mode 660")),
        "{not_ready_while_held}"
    );
    assert_eq!(
        not_ready_while_free["running"], false,
        "{not_ready_while_free}"
    );
    assert!(
        not_ready_while_free.get("starting").is_none(),
        "{not_ready_while_free}"
    );
}
