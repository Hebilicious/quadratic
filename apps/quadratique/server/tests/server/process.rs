use std::fs::OpenOptions;
use std::os::unix::fs::PermissionsExt;
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;

use quadratique_server::git::watcher;
#[cfg(target_os = "linux")]
use quadratique_server::serve;
use quadratique_server::server::{self, Paths, RuntimeFile, Waits};
use quadratique_server::web::WebApp;

use crate::support::{HANG_GUARD, web_address, within};

const LONGER_THAN_THE_TEST: Duration = Duration::from_secs(3600);
const _: () = assert!(LONGER_THAN_THE_TEST.as_secs() > HANG_GUARD.as_secs());

async fn start_waiting(paths: &Paths, waits: Waits) -> Result<server::Server, server::ServerError> {
    server::start_with(
        paths,
        server::loopback(),
        WebApp::from_env(),
        watcher::EVERY,
        waits,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_writes_no_runtime_file_and_its_web_app_listens_only_on_loopback() {
    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");

    let address = web_address(&running).await;
    let written = paths.runtime.exists();
    running.shutdown().await;

    assert!(address.ip().is_loopback(), "{address}");
    assert!(!written, "the server wrote server.json");
    for elsewhere in ["0.0.0.0:0", "[::1]:0"] {
        let refused = server::start(&paths, elsewhere.parse().unwrap()).await;
        assert!(
            matches!(refused, Err(server::ServerError::NotLoopback(_))),
            "{elsewhere}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_server_on_the_same_home_is_refused_until_the_first_shuts_down() {
    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let first = server::start(&paths, server::loopback())
        .await
        .expect("the first server starts");

    let waits = Waits {
        start: LONGER_THAN_THE_TEST,
        ..Waits::default()
    };
    let second = within(
        "the refusal of a second server, which must not wait for a live owner",
        start_waiting(&paths, waits),
    )
    .await;
    assert!(
        matches!(second, Err(server::ServerError::Owned(_))),
        "a second server started"
    );

    first.shutdown().await;
    let again = server::start(&paths, server::loopback())
        .await
        .expect("a server starts once the first is gone");
    again.shutdown().await;
}

#[tokio::test]
async fn a_server_waits_for_a_holder_of_the_lock_that_is_not_a_server_and_then_starts() {
    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let holder = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.lock())
        .expect("the lock file opens");
    holder.lock().expect("the holder takes the lock");

    let mut starting = pin!(server::start(&paths, server::loopback()));
    if let Poll::Ready(result) = futures::poll!(&mut starting) {
        panic!(
            "the start ended while the holder, which is no server, had the lock: {:?}",
            result.err()
        );
    }
    drop(holder);

    let started = within("the start after the holder let go", starting)
        .await
        .expect("the server starts once the holder lets go");
    started.shutdown().await;
}

#[tokio::test]
async fn no_identity_probe_outlasts_the_wait_of_a_starting_server() {
    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let holder = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.lock())
        .expect("the lock file opens");
    holder.lock().expect("the holder takes the lock");
    let silent = tokio::net::TcpListener::bind(server::loopback())
        .await
        .expect("a listener that never answers");
    let stale = RuntimeFile {
        pid: std::process::id(),
        address: silent.local_addr().expect("the listener's address"),
        url: String::new(),
        database: home.path().join("workspace.db"),
        control: None,
    };
    std::fs::write(&paths.runtime, serde_json::to_vec(&stale).unwrap()).unwrap();
    let accepting = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = silent.accept().await {
            held.push(stream);
        }
    });

    let waits = Waits {
        start: Duration::from_millis(200),
        probe: LONGER_THAN_THE_TEST,
        ..Waits::default()
    };
    let refused = within(
        "the start against a stale address that accepts and never answers",
        start_waiting(&paths, waits),
    )
    .await;
    assert!(
        matches!(refused, Err(server::ServerError::Owned(_))),
        "the start did not end in Owned: {:?}",
        refused.err()
    );
    accepting.abort();
    drop(holder);
}

/// Once the server's listener is closed, any socket of the machine may bind its port, so a connection to the port
/// cannot tell whether the server still listens: the test looks for the server's own socket in the kernel's table.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn a_server_dropped_without_a_shutdown_frees_its_home_and_its_port() {
    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
    let address = web_address(&running).await;
    let listener = kernel_sockets::listening_on(address).expect("the server's listening socket");
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(paths.lock())
        .expect("the home's lock file");
    assert!(lock.try_lock().is_err(), "the server holds its home");

    drop(running);

    within(
        "the home's lock",
        tokio::task::spawn_blocking(move || lock.lock().expect("the lock, once free")),
    )
    .await
    .expect("the waiting thread");
    assert!(
        !kernel_sockets::exists(listener),
        "the dropped server still listens on {address}"
    );
}

/// A listener served by `serve_on`, with a second descriptor of it that the test keeps open, as a child process holds
/// one from its `fork` to its `exec` (other tests of this binary start children all the time). `try_clone` makes that
/// descriptor with `F_DUPFD_CLOEXEC`.
#[cfg(target_os = "linux")]
fn served_with_a_copy() -> (serve::Served, std::net::TcpListener) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
    let copy = listener
        .try_clone()
        .expect("a second descriptor of the listener");
    listener
        .set_nonblocking(true)
        .expect("a nonblocking listener");
    let listener = tokio::net::TcpListener::from_std(listener).expect("a tokio listener");
    let served = serve::serve_on(
        listener,
        Vec::new(),
        serve::TCP_CONNECTIONS,
        serve::REQUEST_LIMIT,
    )
    .expect("served");
    (served, copy)
}

/// Closing the server's own descriptor leaves the socket listening while another copy of it is open; the listener must
/// stop listening anyway, before the server lets go of its home.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_server_stops_listening_while_another_copy_of_its_listener_is_open() {
    let (served, copy) = served_with_a_copy();
    let address = served.address;
    let listener = kernel_sockets::listening_on(address).expect("the listening socket");

    drop(served);

    assert!(
        !kernel_sockets::exists(listener),
        "the dropped server still listens on {address} through another copy of its descriptor"
    );
    drop(copy);
}

/// A restart that asks for the port of the server it replaces, as a home that keeps its address does, binds it at once
/// even while a copy of the old listener's descriptor is open.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_binds_the_port_at_once_while_a_copy_of_the_old_listener_is_open() {
    let (served, copy) = served_with_a_copy();
    let address = served.address;

    drop(served);

    let socket = tokio::net::TcpSocket::new_v4().expect("a socket");
    socket
        .set_reuseaddr(true)
        .expect("SO_REUSEADDR, as the server sets it");
    let again = socket.bind(address).and_then(|()| socket.listen(1024));
    assert!(again.is_ok(), "the port of the dropped server: {again:?}");
    drop(copy);
}

/// The IPv4 TCP sockets of this network namespace, as `/proc/net/tcp` lists them.
#[cfg(target_os = "linux")]
mod kernel_sockets {
    use std::net::SocketAddr;

    const LISTEN: &str = "0A";

    struct Row {
        local: String,
        state: String,
        inode: u64,
    }

    fn rows() -> Vec<Row> {
        let table = std::fs::read_to_string("/proc/net/tcp").expect("the kernel's TCP table");
        table
            .lines()
            .skip(1)
            .filter_map(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                Some(Row {
                    local: (*fields.get(1)?).to_owned(),
                    state: (*fields.get(3)?).to_owned(),
                    inode: fields.get(9)?.parse().ok()?,
                })
            })
            .collect()
    }

    /// The table writes an IPv4 address as its 32 bits in the host's byte order, in hexadecimal.
    fn as_listed(address: SocketAddr) -> String {
        let SocketAddr::V4(address) = address else {
            panic!("{address} is not an IPv4 address");
        };
        let ip = u32::from_ne_bytes(address.ip().octets());
        format!("{ip:08X}:{:04X}", address.port())
    }

    /// The inode of the socket that listens on `address`.
    pub fn listening_on(address: SocketAddr) -> Option<u64> {
        let local = as_listed(address);
        rows()
            .into_iter()
            .find(|row| row.local == local && row.state == LISTEN)
            .map(|row| row.inode)
    }

    pub fn exists(inode: u64) -> bool {
        rows().iter().any(|row| row.inode == inode)
    }
}

/// The server never changes the mode of a home that exists: it refuses it, and names the command that makes it private.
#[tokio::test]
async fn the_server_refuses_a_data_folder_that_other_accounts_may_use_and_leaves_its_mode() {
    let parent = tempfile::tempdir().expect("a temporary folder");
    let home = parent.path().join("home");
    std::fs::create_dir(&home).expect("the home");
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).expect("mode 0755");
    let paths = Paths::in_home(&home).expect("an absolute home");

    let refused = server::start(&paths, server::loopback()).await.err();

    let mode = std::fs::metadata(&home)
        .expect("the home")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755, "mode {mode:o}");
    let Some(error @ server::ServerError::Home(_)) = refused else {
        panic!("the home was not refused: {refused:?}");
    };
    assert!(error.to_string().contains("chmod 700"), "{error}");
}

/// Waits until `ended` says every held connection was cut off, and gives the most TCP connections that `server` had
/// open at once meanwhile, each a descriptor of the server. Each look is a few milliseconds apart: a sample, not a
/// deadline.
#[cfg(target_os = "linux")]
async fn most_open_until(server: &server::Server, mut ended: impl FnMut() -> bool) -> usize {
    within("the cut-off of every held connection", async {
        let mut most = server.web_connections().await;
        while !ended() {
            most = most.max(server.web_connections().await);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        most
    })
    .await
}

/// Connections to the web app beyond its bound wait in the backlog and hold no descriptor of the server, and a request
/// that does not arrive whole within the limit is cut off, so every held connection ends; the socket answers meanwhile.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn tcp_connections_beyond_the_bound_hold_no_descriptor_and_partial_requests_are_cut_off() {
    use std::io::{Read, Write};

    let running = crate::support::start_with_request_limit(Duration::from_secs(1)).await;
    let address = web_address(&running.server).await;
    let held_count = 3 * serve::TCP_CONNECTIONS;
    let mut held: Vec<std::net::TcpStream> = (0..held_count)
        .map(|_| {
            let mut client = std::net::TcpStream::connect(address).expect("a connection");
            client
                .write_all(b"GET / HTTP/1.1\r\nHost: ")
                .expect("part of a request");
            client.set_nonblocking(true).expect("a nonblocking client");
            client
        })
        .collect();

    let most = most_open_until(&running.server, || {
        held.retain_mut(|client| {
            let mut bytes = [0u8; 64];
            match client.read(&mut bytes) {
                Err(error) => error.kind() == std::io::ErrorKind::WouldBlock,
                Ok(read) => read > 0,
            }
        });
        held.is_empty()
    })
    .await;
    let answered = server::identity_over_socket(
        &quadratique_server::home::prepare(&Paths::in_home(running.home()).expect("a home"))
            .expect("the home"),
    )
    .await;
    running.server.shutdown().await;

    assert!(answered.is_ok(), "{answered:?}");
    assert!(
        most <= serve::TCP_CONNECTIONS,
        "the server had {most} of {held_count} connections open at once, more than the bound of {}",
        serve::TCP_CONNECTIONS
    );
}

/// What the account `nobody` holds against the web app: as many connections as it can open, each with part of a
/// request, until the server cuts each one off. It says `held` once every connection is open.
#[cfg(target_os = "linux")]
const HOLD_AS_NOBODY: &str = r#"
import socket, sys
host, port, count = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
held = []
for _ in range(count):
    client = socket.create_connection((host, port), timeout=120)
    client.sendall(b"GET / HTTP/1.1\r\nHost: ")
    held.append(client)
print("held", flush=True)
for client in held:
    while client.recv(64):
        pass
"#;

/// Another account that holds three times the bound of TCP connections, each with part of a request, never takes a
/// descriptor beyond the bound, and the owner's socket keeps answering.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs `sudo -n` as the account nobody, and python3; CI runs it with `moon run quadratique-server:test-as-nobody`"]
async fn another_account_that_holds_tcp_connections_takes_no_descriptor_beyond_the_bound() {
    use tokio::io::AsyncBufReadExt;

    let running = crate::support::start_with_request_limit(Duration::from_secs(1)).await;
    let address = web_address(&running.server).await;
    let held_count = 3 * serve::TCP_CONNECTIONS;
    let mut holder = tokio::process::Command::new("sudo")
        .args(["-n", "-u", "nobody", "python3", "-c", HOLD_AS_NOBODY])
        .arg(address.ip().to_string())
        .arg(address.port().to_string())
        .arg(held_count.to_string())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("sudo runs");
    let mut said = String::new();
    within(
        "the other account's connections",
        tokio::io::BufReader::new(holder.stdout.take().expect("its output")).read_line(&mut said),
    )
    .await
    .expect("its output is read");
    assert_eq!(said.trim(), "held", "the other account held no connection");
    let home = quadratique_server::home::prepare(&Paths::in_home(running.home()).expect("a home"))
        .expect("the home");

    let answered = server::identity_over_socket(&home).await;
    let mut ended = false;
    let most = most_open_until(&running.server, || {
        ended = ended || holder.try_wait().is_ok_and(|status| status.is_some());
        ended
    })
    .await;
    running.server.shutdown().await;

    assert!(answered.is_ok(), "{answered:?}");
    assert!(
        most <= serve::TCP_CONNECTIONS,
        "the server had {most} of another account's {held_count} connections open at once, more than the bound of {}",
        serve::TCP_CONNECTIONS
    );
}
