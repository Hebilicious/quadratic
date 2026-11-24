use std::fs::OpenOptions;
use std::os::unix::fs::PermissionsExt;
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;

use quadratique_server::git::watcher;
use quadratique_server::server::{self, Paths, RuntimeFile, Waits};
use quadratique_server::web::WebApp;

use crate::support::{HANG_GUARD, within};

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
async fn the_server_publishes_its_loopback_address_while_it_runs_and_removes_it_on_shutdown() {
    let home = tempfile::tempdir().expect("a temporary home");
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");

    let written: RuntimeFile =
        serde_json::from_slice(&std::fs::read(&paths.runtime).expect("the runtime file exists"))
            .expect("the runtime file is JSON");
    assert_eq!(written.pid, std::process::id());
    assert!(written.address.ip().is_loopback());
    assert_eq!(written.url, format!("http://{}/cli", written.address));
    assert_eq!(written.url, running.url());
    assert_eq!(
        written.database,
        home.path().canonicalize().unwrap().join("workspace.db")
    );

    running.shutdown().await;
    assert!(!paths.runtime.exists());
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
    let home = tempfile::tempdir().expect("a temporary home");
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
    let home = tempfile::tempdir().expect("a temporary home");
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
    let home = tempfile::tempdir().expect("a temporary home");
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

async fn written_runtime(paths: &Paths) -> Option<RuntimeFile> {
    let text = std::fs::read(&paths.runtime).ok()?;
    serde_json::from_slice(&text).ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_stops_never_removes_the_runtime_file_of_its_replacement() {
    use std::io::Write;

    let home = tempfile::tempdir().expect("a temporary home");
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let old = server::start(&paths, server::loopback())
        .await
        .expect("the old server starts");
    let old_runtime = std::fs::read(&paths.runtime).expect("the old server's runtime file");
    std::fs::remove_file(&paths.runtime).expect("the runtime file moves aside");
    let made = std::process::Command::new("mkfifo")
        .arg(&paths.runtime)
        .status()
        .expect("mkfifo runs");
    assert!(made.success());
    let stopping = tokio::spawn(old.shutdown());
    let fifo = paths.runtime.clone();
    let mut held_read = within(
        "the old server's read of its runtime file",
        tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new()
                .write(true)
                .open(fifo)
                .expect("the other end of the old server's read")
        }),
    )
    .await
    .expect("the opening thread");

    let no_wait = Waits {
        start: Duration::ZERO,
        ..Waits::default()
    };
    let during = start_waiting(&paths, no_wait).await;
    held_read
        .write_all(&old_runtime)
        .expect("the old server's read gets its own runtime");
    drop(held_read);
    within("the old server's shutdown", stopping)
        .await
        .expect("the shutdown's task");

    let replacement = match during {
        Ok(replacement) => replacement,
        Err(server::ServerError::Owned(_)) => server::start(&paths, server::loopback())
            .await
            .expect("the replacement starts once the old server is gone"),
        Err(error) => panic!("{error}"),
    };
    let named = written_runtime(&paths).await.map(|runtime| runtime.address);
    assert_eq!(named, Some(replacement.address()));
    replacement.shutdown().await;
}

/// Once the server's listener is closed, any socket of the machine may bind its port, so a connection to the port
/// cannot tell whether the server still listens: the test looks for the server's own socket in the kernel's table.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn a_server_dropped_without_a_shutdown_frees_its_home_and_its_port() {
    let home = tempfile::tempdir().expect("a temporary home");
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
    let address = running.address();
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

#[tokio::test]
async fn the_server_makes_its_data_folder_private_to_its_account() {
    let parent = tempfile::tempdir().expect("a temporary folder");
    let home = parent.path().join("home");
    std::fs::create_dir(&home).expect("the home");
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).expect("mode 0755");
    let paths = Paths::in_home(&home).expect("an absolute home");

    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");

    let mode = std::fs::metadata(&home)
        .expect("the home")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700, "mode {mode:o}");
    running.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sudo to act as the account nobody"]
async fn a_process_of_another_account_is_refused() {
    let home = tempfile::tempdir().expect("a temporary home");
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
    let url = format!("http://{}/server", running.address());

    let answer = tokio::process::Command::new("sudo")
        .args([
            "-n",
            "-u",
            "nobody",
            "curl",
            "-sS",
            "-w",
            "\n%{http_code}",
            &url,
        ])
        .output()
        .await
        .expect("sudo runs");

    running.shutdown().await;
    let stdout = String::from_utf8_lossy(&answer.stdout);
    println!("{stdout}");
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec![quadratique_server::serve::OWNER_ONLY, "403"],
        "{answer:?}"
    );
}

/// The churn opens thousands of loopback connections, and the kernel may drop the first SYN of a connection, for
/// example while its connection-tracking table is full; the connection then opens on a later retransmission. The test
/// asks the admission of each connection directly, so a slow handshake is not taken for a refusal.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_is_admitted_while_other_sockets_open_and_close() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use quadratique_server::serve::Connection;

    let home = tempfile::tempdir().expect("a temporary home");
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
    let other = std::net::TcpListener::bind("127.0.0.1:0").expect("another listener");
    let other_address = other.local_addr().expect("its address");
    let stop = Arc::new(AtomicBool::new(false));
    let accepting = std::thread::spawn(move || {
        for stream in other.incoming() {
            if stream.is_err() {
                return;
            }
        }
    });
    let churning: Vec<_> = (0..4)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let opened: Vec<_> = (0..50)
                        .filter_map(|_| std::net::TcpStream::connect(other_address).ok())
                        .collect();
                    drop(opened);
                }
            })
        })
        .collect();

    let refused = within(
        "2000 connections of the owner, each one admitted or refused",
        async {
            let mut refused = Vec::new();
            for probe in 0..2000 {
                let client = tokio::net::TcpStream::connect(running.address())
                    .await
                    .expect("a connection to the server");
                let own = Connection {
                    local: running.address(),
                    peer: client.local_addr().expect("the client's address"),
                };
                if !running.admits(own) {
                    refused.push(probe);
                }
            }
            refused
        },
    )
    .await;

    stop.store(true, Ordering::Relaxed);
    for thread in churning {
        thread.join().expect("a churning thread");
    }
    drop(accepting);
    running.shutdown().await;
    assert!(
        refused.is_empty(),
        "{} of 2000 connections of the owner were refused: {:?}",
        refused.len(),
        &refused[..refused.len().min(5)]
    );
}

#[tokio::test]
async fn a_refused_connection_reads_as_the_owner_only_refusal() {
    use quadratique_server::serve::{self, Connection};
    use quadratique_server::server::Unanswered;

    let refuse_all: serve::Admit = std::sync::Arc::new(|_: Connection| false);
    let served = serve::serve(server::loopback(), Vec::new(), refuse_all)
        .await
        .expect("a listener");

    let probe = server::identity(served.address).await;

    served.close().await;
    assert!(matches!(probe, Err(Unanswered::OwnerOnly)), "{probe:?}");
}

#[tokio::test]
async fn a_started_server_admits_its_own_account_and_refuses_a_peer_with_no_connection() {
    use quadratique_server::serve::Connection;

    let home = tempfile::tempdir().expect("a temporary home");
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
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

    let admitted = (running.admits(own), running.admits(nobody_connected));

    drop(client);
    running.shutdown().await;
    assert_eq!(admitted, (true, false));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_beyond_the_draining_bound_closes_the_connection_at_once() {
    use std::io::{Read, Write};

    use quadratique_server::serve::{self, Connection, Refusals};

    let refusals = Refusals {
        draining: 1,
        drain_bytes: u64::MAX,
        drain_for: LONGER_THAN_THE_TEST,
    };
    let refuse_all: serve::Admit = std::sync::Arc::new(|_: Connection| false);
    let served = serve::serve_with(server::loopback(), Vec::new(), refuse_all, refusals)
        .await
        .expect("a listener");
    let address = served.address;
    let refused = |mut client: std::net::TcpStream| {
        client
            .write_all(b"GET /server HTTP/1.1\r\nHost: quadratique\r\n\r\n")
            .expect("the request is sent");
        let mut answer = [0u8; 16];
        let _ = client.read(&mut answer);
        client
    };

    let drained = tokio::task::spawn_blocking(move || {
        let mut drained = refused(std::net::TcpStream::connect(address).expect("a connection"));
        let mut beyond = refused(std::net::TcpStream::connect(address).expect("a connection"));
        let closed_at_once = within_blocking(|| beyond.write_all(b"x").is_err());
        let still_drained = drained.write_all(b"x").is_ok();
        (closed_at_once, still_drained)
    })
    .await
    .expect("the clients");

    served.close().await;
    assert_eq!(drained, (true, true));
}

fn within_blocking(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + HANG_GUARD;
    while std::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
    }
    false
}
