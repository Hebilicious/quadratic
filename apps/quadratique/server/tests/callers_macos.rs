//! The caller check on macOS against the real kernel's table of TCP connections, with a process of the owner and a
//! process of another account that each hold a connection open with `nc`. The tests that act as the account
//! `nobody` need `sudo -n` and are ignored by `test`; `moon run quadratique-server:test-callers-macos` runs them, and
//! so does CI on macOS.
#![cfg(target_os = "macos")]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use quadratique_server::peer_macos::{self, PeerSocket};
use quadratique_server::serve::Connection;
use quadratique_server::server::{self, Paths};
use quadratique_server::tcp_table::Found;
use rustix::event::{PollFd, PollFlags, Timespec, poll};

/// Only stops a hang: nc connects at once.
const HANG_GUARD: Duration = Duration::from_secs(120);
const LOOK_EVERY: Timespec = Timespec {
    tv_sec: 0,
    tv_nsec: 50_000_000,
};

/// A process that holds one connection to `listener` open until it is dropped: `nc` keeps it while its standard input
/// stays open.
struct Holder {
    child: Child,
    _stdin: ChildStdin,
    connection: Connection,
    _accepted: TcpStream,
}

impl Holder {
    /// Waits for nc's connection and fails, instead of waiting for ever, when nc or sudo exits without connecting.
    fn start(mut command: Command, listener: &TcpListener) -> Holder {
        let address = listener.local_addr().expect("the listener's address");
        let mut child = command
            .args(["nc", "127.0.0.1", &address.port().to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("nc starts");
        let stdin = child.stdin.take().expect("nc's standard input");
        let deadline = Instant::now() + HANG_GUARD;
        loop {
            if let Some(status) = child.try_wait().expect("nc can be waited on") {
                panic!("nc exited without connecting: {status}");
            }
            assert!(
                Instant::now() < deadline,
                "nc did not connect within the hang guard"
            );
            let mut ready = [PollFd::new(listener, PollFlags::IN)];
            if poll(&mut ready, Some(&LOOK_EVERY)).expect("poll on the listener") == 1 {
                break;
            }
        }
        let (accepted, peer) = listener.accept().expect("nc's connection is accepted");
        let local = accepted.local_addr().expect("the accepted side's address");
        Holder {
            child,
            _stdin: stdin,
            connection: Connection { local, peer },
            _accepted: accepted,
        }
    }

    fn peer_socket(&self) -> PeerSocket {
        let (SocketAddr::V4(server), SocketAddr::V4(peer)) =
            (self.connection.local, self.connection.peer)
        else {
            panic!("an IPv4 connection");
        };
        PeerSocket { peer, server }
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn listener() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").expect("a loopback port")
}

fn uid_of(account: &str) -> u32 {
    let output = Command::new("id")
        .args(["-u", account])
        .output()
        .expect("id runs");
    assert!(output.status.success(), "{output:?}");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("a uid")
}

#[test]
fn a_connection_of_another_process_of_the_owner_is_admitted() {
    let listener = listener();
    let holder = Holder::start(Command::new("/usr/bin/env"), &listener);

    let admitted = peer_macos::owner_only(peer_macos::own_account())(holder.connection);
    let found = peer_macos::find(holder.peer_socket()).expect("the table can be read");

    println!(
        "the owner (uid {}) connects with nc from {}: admitted {admitted}; the table finds {found:?}",
        peer_macos::own_account(),
        holder.connection.peer
    );
    assert!(admitted);
    assert_eq!(
        found,
        Found::Caller {
            uid: peer_macos::own_account()
        }
    );
}

#[test]
#[ignore = "needs sudo to act as the account nobody"]
fn a_connection_of_another_account_is_refused_and_the_table_names_that_account() {
    let listener = listener();
    let mut as_nobody = Command::new("sudo");
    as_nobody.args(["-n", "-u", "nobody"]);
    let holder = Holder::start(as_nobody, &listener);
    let nobody = uid_of("nobody");

    let admitted = peer_macos::owner_only(peer_macos::own_account())(holder.connection);
    let found = peer_macos::find(holder.peer_socket()).expect("the table can be read");

    println!(
        "the owner is uid {}; nobody (uid {nobody}) connects with nc from {}: admitted {admitted}; the table finds {found:?}",
        peer_macos::own_account(),
        holder.connection.peer,
    );
    assert!(!admitted, "a connection of nobody was admitted");
    assert_eq!(found, Found::Caller { uid: nobody });
}

const LOOKUPS: usize = 200;

fn summary(mut times: Vec<Duration>) -> String {
    times.sort();
    let at = |share: f64| times[((times.len() - 1) as f64 * share) as usize];
    format!(
        "median {:?}, p95 {:?}, max {:?}",
        at(0.5),
        at(0.95),
        at(1.0)
    )
}

/// Prints what the check costs on this machine; asserts nothing about time.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measure, read from its output"]
async fn the_cost_of_the_caller_check() {
    let listener = listener();
    let holder = Holder::start(Command::new("/usr/bin/env"), &listener);
    let (lookups, connections) =
        peer_macos::time_lookups(holder.peer_socket(), LOOKUPS).expect("the table can be read");
    println!(
        "{LOOKUPS} lookups, each a read of the whole table of {connections} TCP connections over IPv4: {}",
        summary(lookups)
    );

    let home = tempfile::tempdir().expect("a temporary home");
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
    let address = running.address();
    let (fresh, kept) = tokio::task::spawn_blocking(move || {
        let request = |stream: &mut TcpStream, close: bool| {
            let connection = if close { "close" } else { "keep-alive" };
            write!(
                stream,
                "GET {} HTTP/1.1\r\nHost: {address}\r\nConnection: {connection}\r\n\r\n",
                server::IDENTITY
            )
            .expect("the request is sent");
        };
        let fresh: Vec<Duration> = (0..LOOKUPS)
            .map(|_| {
                let started = Instant::now();
                let mut stream = TcpStream::connect(address).expect("a connection");
                request(&mut stream, true);
                let mut answer = String::new();
                stream.read_to_string(&mut answer).expect("the answer");
                assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
                started.elapsed()
            })
            .collect();
        let mut stream = TcpStream::connect(address).expect("a connection");
        let kept: Vec<Duration> = (0..LOOKUPS)
            .map(|_| {
                let started = Instant::now();
                request(&mut stream, false);
                read_one_answer(&mut stream);
                started.elapsed()
            })
            .collect();
        (fresh, kept)
    })
    .await
    .expect("the requests");
    println!(
        "{LOOKUPS} requests to the server, each on a new connection: {}",
        summary(fresh)
    );
    println!(
        "{LOOKUPS} requests to the server on one kept-alive connection: {}",
        summary(kept)
    );
    running.shutdown().await;
    drop(holder);
}

/// Reads one HTTP/1.1 answer whose body has a `content-length`.
fn read_one_answer(stream: &mut TcpStream) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).expect("the answer's head");
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 200"), "{head}");
    let length: usize = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .map(|value| value.trim().parse().expect("a length"))
        .expect("a content-length");
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).expect("the answer's body");
}
