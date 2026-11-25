//! Where the server listens when no port is given: on the port it kept by default, on a fresh one in strict mode.
//!
//! A start that blocks on `server.port` blocks the worker thread it runs on, so the read test starts each server on a
//! task of its own, which the test's task can outlive.

use std::net::{Ipv4Addr, SocketAddr};

use quadratique_server::home::{self, Prepared};
use quadratique_server::serve::Connection;
use quadratique_server::server::{self, Paths, RuntimeFile};
use tempfile::TempDir;
use tokio::net::TcpSocket;

use crate::support::{HANG_GUARD, private_home, within};

struct Home {
    paths: Paths,
    prepared: Prepared,
    _folder: TempDir,
}

impl Home {
    fn new() -> Home {
        let folder = private_home();
        let paths = Paths::in_home(folder.path()).expect("an absolute home");
        let prepared = home::prepare(&paths).expect("a private home");
        Home {
            paths,
            prepared,
            _folder: folder,
        }
    }

    fn strict() -> Home {
        let home = Home::new();
        server::set_strict(&home.prepared, true, None).expect("the setting is written");
        home
    }

    async fn start(&self) -> server::Server {
        within("the server's start", server::start_in_home(&self.paths))
            .await
            .expect("the server starts")
    }

    fn kept(&self) -> Option<String> {
        std::fs::read_to_string(self.prepared.kept_port()).ok()
    }

    fn write_kept(&self, text: &str) {
        std::fs::write(self.prepared.kept_port(), text).expect("server.port is written");
    }

    fn served(&self) -> Vec<u16> {
        std::fs::read_to_string(self.prepared.served_ports())
            .unwrap_or_default()
            .lines()
            .map(|line| line.parse().expect("a port per line"))
            .collect()
    }

    fn published(&self) -> Option<SocketAddr> {
        let text = std::fs::read(self.prepared.runtime()).ok()?;
        serde_json::from_slice::<RuntimeFile>(&text)
            .ok()
            .map(|runtime| runtime.address)
    }
}

/// A port that the system never gives to a socket asking for any port while this guard lives, but that a server asked
/// for it by number can still bind and listen on: the guard is bound with `SO_REUSEADDR` and does not listen, and Linux
/// shares a port between such sockets, while its automatic choice skips a port in use (`ip_autobind_reuse` is off).
struct HeldPort {
    port: u16,
    _socket: TcpSocket,
}

fn held_port() -> HeldPort {
    let socket = TcpSocket::new_v4().expect("a socket");
    socket.set_reuseaddr(true).expect("SO_REUSEADDR");
    socket
        .bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .expect("a loopback port");
    let port = socket.local_addr().expect("its address").port();
    HeldPort {
        port,
        _socket: socket,
    }
}

fn admits_only_its_owner(running: &server::Server) -> bool {
    let client = std::net::TcpStream::connect(running.address()).expect("a connection");
    let own = Connection {
        local: running.address(),
        peer: client.local_addr().expect("the client's address"),
    };
    let unconnected = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let nobody_connected = Connection {
        local: running.address(),
        peer: unconnected.local_addr().expect("its address"),
    };
    running.admits(own) && !running.admits(nobody_connected)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_takes_the_kept_port_even_while_a_socket_that_does_not_listen_holds_it() {
    let home = Home::new();
    let held = held_port();
    home.write_kept(&format!("{}\n", held.port));

    let running = home.start().await;

    let port = running.address().port();
    running.shutdown().await;
    assert_eq!(port, held.port, "the tests below rely on this");
}

#[tokio::test(flavor = "multi_thread")]
async fn by_default_the_server_restarts_on_its_address_and_still_admits_only_its_owner() {
    let home = Home::new();
    let first = home.start().await;
    let address = first.address();
    first.shutdown().await;
    assert_eq!(home.kept(), Some(format!("{}\n", address.port())));

    let restarted = home.start().await;

    let again = (restarted.address(), admits_only_its_owner(&restarted));
    restarted.shutdown().await;
    assert_eq!(again, (address, true));
}

#[tokio::test(flavor = "multi_thread")]
async fn when_the_kept_port_is_taken_the_server_listens_on_another_publishes_it_and_keeps_it() {
    let home = Home::new();
    let first = home.start().await;
    let address = first.address();
    first.shutdown().await;
    let squatter = std::net::TcpListener::bind(address).expect("the kept port, now free");

    let restarted = home.start().await;

    let moved = restarted.address();
    let published = home.published();
    let admits = admits_only_its_owner(&restarted);
    restarted.shutdown().await;
    drop(squatter);
    assert_ne!(moved, address);
    assert_eq!(published, Some(moved));
    assert_eq!(home.kept(), Some(format!("{}\n", moved.port())));
    assert!(admits, "the server on the new port admits only its owner");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_given_port_even_zero_never_reads_or_writes_the_kept_port() {
    let home = Home::new();
    let held = held_port();
    home.write_kept(&format!("{}\n", held.port));

    let any = server::start(&home.paths, server::loopback())
        .await
        .expect("the server starts on a port the system chooses");
    let chosen = any.address().port();
    any.shutdown().await;
    let given = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free port")
        .port();
    let named = server::start(&home.paths, SocketAddr::from((Ipv4Addr::LOCALHOST, given)))
        .await
        .expect("the server starts on the given port");
    let on = named.address().port();
    named.shutdown().await;

    assert_ne!(chosen, held.port, "--port 0 read server.port");
    assert_eq!(on, given);
    assert_eq!(home.kept(), Some(format!("{}\n", held.port)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_strict_home_neither_reads_nor_writes_a_kept_port_and_lists_the_ports_it_served() {
    let home = Home::strict();
    let held = held_port();
    home.write_kept(&format!("{}\n", held.port));

    let first = home.start().await;
    let one = first.address().port();
    first.shutdown().await;
    let second = home.start().await;
    let two = second.address().port();
    second.shutdown().await;

    assert_ne!(one, held.port, "a strict home read server.port");
    assert_ne!(one, two, "a strict home served a port twice");
    assert_eq!(home.kept(), Some(format!("{}\n", held.port)));
    assert_eq!(home.served(), vec![one, two]);
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_strict_on_forgets_the_kept_port_and_lists_it_as_served() {
    let home = Home::new();
    home.write_kept("4100\n");

    server::set_strict(&home.prepared, true, None).expect("strict on");

    assert!(server::strict(&home.prepared));
    assert_eq!(home.kept(), None);
    assert_eq!(home.served(), vec![4100]);
    server::set_strict(&home.prepared, false, Some(4200)).expect("strict off");
    assert!(!server::strict(&home.prepared));
    assert_eq!(home.kept(), Some("4200\n".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_kept_port_is_read_only_from_a_small_regular_file() {
    let home = Home::new();
    let held = held_port();
    let elsewhere = tempfile::tempdir().expect("a folder outside the home");
    let target = elsewhere.path().join("port");
    std::fs::write(&target, format!("{}\n", held.port)).expect("a port file elsewhere");
    let padded = format!("{}{}\n", " ".repeat(1 << 20), held.port);

    let mut ports = Vec::new();
    for kind in ["link", "long", "fifo"] {
        let _ = std::fs::remove_file(home.prepared.kept_port());
        match kind {
            "link" => {
                std::os::unix::fs::symlink(&target, home.prepared.kept_port()).expect("a link");
            }
            "long" => home.write_kept(&padded),
            _ => {
                let made = std::process::Command::new("mkfifo")
                    .arg(home.prepared.kept_port())
                    .status()
                    .expect("mkfifo runs");
                assert!(made.success(), "mkfifo fails");
            }
        }
        let paths = home.paths.clone();
        let mut starting = tokio::spawn(async move { server::start_in_home(&paths).await });
        let started = match tokio::time::timeout(HANG_GUARD, &mut starting).await {
            Ok(started) => started
                .expect("the starting task")
                .expect("the server starts"),
            Err(_) => {
                // A writer that opens and closes the FIFO ends the blocked read; the server that then starts is shut
                // down, so that the failure ends the test instead of hanging it.
                drop(
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(home.prepared.kept_port()),
                );
                if let Ok(Ok(late)) = within("the start once the FIFO is closed", starting).await {
                    late.shutdown().await;
                }
                panic!(
                    "the start waited on server.port as a {kind} for the hang guard of {HANG_GUARD:?}"
                );
            }
        };
        ports.push((kind, started.address().port()));
        started.shutdown().await;
    }

    for (kind, port) in ports {
        assert_ne!(port, held.port, "the server read a kept port from a {kind}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_kept_port_that_cannot_be_written_leaves_the_server_serving_and_no_partial_file() {
    let home = Home::new();
    std::fs::create_dir(home.prepared.kept_port()).expect("a folder where server.port goes");

    let running = home.start().await;

    let answered = server::identity(running.address()).await;
    running.shutdown().await;
    assert!(answered.is_ok(), "{answered:?}");
    let partial: Vec<String> = std::fs::read_dir(home.prepared.folder())
        .expect("the data folder")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("server.port."))
        .collect();
    assert_eq!(partial, Vec::<String>::new());
}
