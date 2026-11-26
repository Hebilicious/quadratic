//! The server's Unix socket, and the home policy as the server applies it before it opens its lock file.

use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use quadratique_server::home::{self, HomeError, SOCKET_FILE};
#[cfg(target_os = "linux")]
use quadratique_server::serve;
use quadratique_server::server::{self, CONTROL, Paths, ServerError, Started};

use crate::support::{stop_listening, within};

fn prepared_at(home: &Path) -> home::Prepared {
    home::prepare(&Paths::in_home(home).expect("an absolute home"))
        .expect("the home passes the policy")
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .expect("the entry")
        .permissions()
        .mode()
        & 0o7777
}

fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("a mode");
}

async fn refusal_before_the_lock(home: &Path) -> HomeError {
    let paths = Paths::in_home(home).expect("an absolute home");
    let refused = within("a refused start", server::start(&paths, server::loopback())).await;
    let lock = paths.lock();
    assert!(
        !lock.exists(),
        "{} exists after the refusal",
        lock.display()
    );
    match refused {
        Err(ServerError::Home(refusal)) => refusal,
        Ok(running) => {
            running.shutdown().await;
            panic!("the server started in {}", home.display());
        }
        Err(other) => panic!("not refused by the home policy: {other}"),
    }
}

#[tokio::test]
async fn each_refusal_of_the_home_policy_comes_before_the_server_opens_its_lock_file() {
    let folder = tempfile::tempdir().expect("a temporary folder");
    let base = folder.path().canonicalize().expect("a real path");

    let shared = base.join("shared");
    std::fs::create_dir(&shared).expect("a shared folder");
    set_mode(&shared, 0o777);
    let refusal = refusal_before_the_lock(&shared.join("home")).await;
    assert!(
        matches!(&refusal, HomeError::Replaceable { path, .. } if path == &shared),
        "{refusal}"
    );

    let real = base.join("real");
    std::fs::create_dir(&real).expect("a home");
    set_mode(&real, 0o700);
    symlink(&real, shared.join("link")).expect("a link in the shared folder");
    let refusal = refusal_before_the_lock(&shared.join("link")).await;
    assert!(
        matches!(&refusal, HomeError::ForeignLink { .. }),
        "{refusal}"
    );
    set_mode(&shared, 0o755);

    let grouped = base.join("grouped");
    std::fs::create_dir(&grouped).expect("a home");
    set_mode(&grouped, 0o770);
    let refusal = refusal_before_the_lock(&grouped).await;
    assert!(
        matches!(&refusal, HomeError::NotPrivate { .. }),
        "{refusal}"
    );

    #[cfg(target_os = "linux")]
    {
        let with_acl = base.join("with-acl");
        std::fs::create_dir(&with_acl).expect("a home");
        set_mode(&with_acl, 0o700);
        write_named_user_acl(&with_acl);
        let refusal = refusal_before_the_lock(&with_acl).await;
        assert!(
            matches!(&refusal, HomeError::SharedAccess { .. }),
            "{refusal}"
        );
    }

    let with_file = base.join("with-file");
    std::fs::create_dir(&with_file).expect("a home");
    set_mode(&with_file, 0o700);
    std::fs::write(with_file.join(SOCKET_FILE), "notes").expect("a regular file");
    let refusal = refusal_before_the_lock(&with_file).await;
    assert!(
        matches!(&refusal, HomeError::SocketEntry { what, .. } if what == "a regular file"),
        "{refusal}"
    );
    assert_eq!(
        std::fs::read_to_string(with_file.join(SOCKET_FILE)).expect("the file stays"),
        "notes"
    );
}

/// User 65534 may read and enter the folder; the mask of `---` leaves its mode at 0700. A Linux ACL.
#[cfg(target_os = "linux")]
fn write_named_user_acl(path: &Path) {
    const UNDEFINED: u32 = u32::MAX;
    let mut value = 2u32.to_le_bytes().to_vec();
    for (tag, perm, id) in [
        (0x01u16, 0o7u16, UNDEFINED),
        (0x02, 0o5, 65534),
        (0x04, 0, UNDEFINED),
        (0x10, 0, UNDEFINED),
        (0x20, 0, UNDEFINED),
    ] {
        value.extend(tag.to_le_bytes());
        value.extend(perm.to_le_bytes());
        value.extend(id.to_le_bytes());
    }
    rustix::fs::setxattr(
        path,
        "system.posix_acl_access",
        &value,
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap_or_else(|error| panic!("{} takes no ACL: {error}", path.display()));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_socket_answers_the_identity_with_the_generation_and_the_control_version() {
    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
    let socket = running.socket_path().to_path_buf();

    let identity = within(
        "the identity over the socket",
        server::identity_over_socket(&prepared_at(home.path())),
    )
    .await
    .expect("the server answers on its socket");

    let meta = std::fs::symlink_metadata(&socket).expect("the socket");
    assert!(std::os::unix::fs::FileTypeExt::is_socket(&meta.file_type()));
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(mode(home.path()), 0o700);
    assert_eq!(
        socket,
        home.path().canonicalize().unwrap().join(SOCKET_FILE)
    );
    assert_eq!(identity.pid, std::process::id());
    assert_eq!(identity.control, Some(CONTROL));
    assert_eq!(identity.started, Some(Started::Serve));
    assert_eq!(identity.url.as_deref(), Some(server::COMPATIBLE_URL));
    assert_eq!(
        identity.web, None,
        "a server that no browser used names a web app"
    );
    let generation = identity.generation.clone().expect("a generation");
    assert!(
        generation.len() == 32 && generation.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{generation}"
    );
    let executable = identity.executable.clone().expect("the executable");
    let this_file = std::env::current_exe().expect("the test binary");
    let this_meta = std::fs::metadata(&this_file).expect("its metadata");
    assert_eq!(executable.path, this_file);
    assert_eq!(
        (executable.device, executable.inode),
        (this_meta.dev(), this_meta.ino())
    );
    running.shutdown().await;
    assert!(!socket.exists(), "the socket outlived its server");
    let again = server::start(&paths, server::loopback())
        .await
        .expect("the next server starts");
    let next = server::identity_over_socket(&prepared_at(home.path()))
        .await
        .expect("the next server answers");
    assert_ne!(next.generation, Some(generation));
    again.shutdown().await;
}

#[tokio::test]
async fn a_stale_socket_of_this_account_is_replaced() {
    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let stale_path = home.path().join(SOCKET_FILE);
    stop_listening(std::os::unix::net::UnixListener::bind(&stale_path).expect("a stale socket"));
    let before = std::os::unix::net::UnixStream::connect(&stale_path);
    assert_eq!(
        before.as_ref().err().map(std::io::Error::kind),
        Some(std::io::ErrorKind::ConnectionRefused),
        "the stale socket accepts: {before:?}"
    );

    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts over a stale socket");

    let identity = server::identity_over_socket(&prepared_at(home.path()))
        .await
        .expect("the socket at the same name answers");
    assert_eq!(identity.pid, std::process::id());
    running.shutdown().await;
}

#[tokio::test]
async fn a_socket_whose_mode_is_not_0600_is_not_trusted() {
    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
    set_mode(running.socket_path(), 0o660);

    let probed = server::identity_over_socket(&prepared_at(home.path())).await;

    assert!(
        matches!(&probed, Err(server::Unanswered::NotReady(why)) if why.contains("mode 660")),
        "{probed:?}"
    );
    running.shutdown().await;
}

/// A hard link reaches the same socket as its name, and stays when the socket's name goes: through it, the test
/// connects after the drop, while it holds a second descriptor of the listener, as a child between its `fork` and its
/// `exec` holds one.
/// Linux's `shutdown` takes a listening socket out of the listening state for every descriptor of it.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_socket_refuses_connections_while_another_copy_of_it_is_open() {
    use std::os::fd::AsFd;

    let home = crate::support::private_home();
    let (listener, file) = home::bind_socket(&prepared_at(home.path()))
        .await
        .expect("the socket binds");
    let copy = listener
        .as_fd()
        .try_clone_to_owned()
        .expect("a second descriptor of the listener");
    let socket = file.path().to_path_buf();
    let served = serve::serve_socket(
        listener,
        file,
        Vec::new(),
        serve::SOCKET_REQUEST_LIMIT,
        &serve::Gate::default(),
    );
    let other_name = home.path().join("same-socket");
    std::fs::hard_link(&socket, &other_name).expect("a hard link to the socket");
    std::os::unix::net::UnixStream::connect(&other_name).expect("the link reaches the socket");

    drop(served);

    assert!(!socket.exists(), "the socket's name outlived its listener");
    let refused = std::os::unix::net::UnixStream::connect(&other_name);
    assert_eq!(
        refused.as_ref().err().map(std::io::Error::kind),
        Some(std::io::ErrorKind::ConnectionRefused),
        "{refused:?}"
    );
    drop(copy);
}

/// Once a listening Unix socket is shut down, Linux reports it readable and hung up for good, and `accept` answers
/// `EAGAIN`. Both ways the server stops shut the socket down just after they cancel its accept loop; a loop that is
/// already inside `accept` at that moment sees only the shut-down socket. This test makes that state on purpose: it
/// shuts the socket down through a second descriptor while the loop runs, on a runtime with one thread. Once the
/// runtime has seen the shutdown, its thread must still run another task.
#[test]
fn an_accept_loop_on_a_socket_that_was_shut_down_leaves_its_thread_to_other_tasks() {
    use std::os::fd::AsFd;

    let (ran, other_task_ran) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        runtime.block_on(async {
            let home = crate::support::private_home();
            let (listener, file) = home::bind_socket(&prepared_at(home.path()))
                .await
                .expect("the socket binds");
            let copy = listener
                .as_fd()
                .try_clone_to_owned()
                .expect("a second descriptor of the listener");
            let served = serve::serve_socket(
                listener,
                file,
                Vec::new(),
                serve::SOCKET_REQUEST_LIMIT,
                &serve::Gate::default(),
            );

            rustix::net::shutdown(&copy, rustix::net::Shutdown::Both)
                .expect("the socket stops listening");
            // tokio waits for the readiness of a stream, not of a listener: only that wait is asked of this copy.
            let copy = tokio::net::UnixStream::from_std(std::os::unix::net::UnixStream::from(copy))
                .expect("a watched copy");
            copy.readable()
                .await
                .expect("the shutdown, as the runtime sees it");
            tokio::task::yield_now().await;

            let _ = ran.send(());
            drop(served);
        });
    });

    assert!(
        other_task_ran
            .recv_timeout(crate::support::HANG_GUARD)
            .is_ok(),
        "the accept loop held the runtime's only thread after its socket was shut down"
    );
}

/// A process of this account that listens on the socket's name without the home's lock keeps its socket: the server
/// removes a socket only after a connection to it is refused.
#[tokio::test]
async fn a_socket_on_which_a_process_still_listens_is_not_removed() {
    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let socket = home.path().join(SOCKET_FILE);
    let listening = std::os::unix::net::UnixListener::bind(&socket).expect("a listening socket");

    let refused = server::start(&paths, server::loopback()).await;

    let Err(ServerError::Home(HomeError::SocketEntry { what, .. })) = &refused else {
        panic!(
            "the server did not refuse the socket in use: {:?}",
            refused.err()
        );
    };
    assert!(what.contains("a process of this account listens"), "{what}");
    std::os::unix::net::UnixStream::connect(&socket).expect("the socket still reaches its process");
    drop(listening);
}

/// An answer of one byte more than the limit is refused before it is parsed, even when it holds a valid identity.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_answer_longer_than_the_limit_is_refused_unparsed() {
    use std::io::{Read, Write};

    let home = crate::support::private_home();
    let socket = home.path().join(SOCKET_FILE);
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("a fake server");
    set_mode(&socket, 0o600);
    let identity = serde_json::json!({
        "pid": 1,
        "database": home.path().join("workspace.db"),
        "control": CONTROL,
    })
    .to_string();
    let head_and_body = |padding: usize| {
        let body = format!("{identity}{}", " ".repeat(padding));
        format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    };
    let mut padding = server::IDENTITY_LIMIT - head_and_body(0).len();
    while head_and_body(padding).len() > server::IDENTITY_LIMIT {
        padding -= 1;
    }
    let at_the_limit = head_and_body(padding);
    let over_the_limit = head_and_body(padding + 1);
    assert_eq!(
        (at_the_limit.len(), over_the_limit.len()),
        (server::IDENTITY_LIMIT, server::IDENTITY_LIMIT + 1)
    );
    let answers = vec![at_the_limit, over_the_limit];
    let serving = std::thread::spawn(move || {
        for answer in answers {
            let (mut stream, _) = listener.accept().expect("a connection");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(answer.as_bytes());
        }
    });
    let prepared = prepared_at(home.path());

    let at_the_limit = server::identity_over_socket(&prepared).await;
    let over_the_limit = server::identity_over_socket(&prepared).await;

    serving.join().expect("the fake server");
    assert!(at_the_limit.is_ok(), "{at_the_limit:?}");
    assert!(
        matches!(&over_the_limit, Err(server::Unanswered::Malformed(why)) if why.contains("longer than")),
        "{over_the_limit:?}"
    );
}

/// What `connect` gives the account nobody on `socket`: `connected`, or the name of the errno.
fn connect_as_nobody(socket: &Path) -> String {
    let script = "import errno, socket, sys\n\
                  s = socket.socket(socket.AF_UNIX)\n\
                  try:\n    s.connect(sys.argv[1]); print('connected')\n\
                  except OSError as e:\n    print(errno.errorcode.get(e.errno, e.errno))\n";
    let output = std::process::Command::new("sudo")
        .args(["-n", "-u", "nobody", "python3", "-c", script])
        .arg(socket)
        .output()
        .expect("sudo runs");
    assert!(output.status.success(), "{output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn as_root(args: &[&str], path: &Path) {
    let status = std::process::Command::new("sudo")
        .arg("-n")
        .args(args)
        .arg(path)
        .status()
        .expect("sudo runs");
    assert!(status.success(), "sudo {args:?} {}", path.display());
}

/// A folder under `/tmp` that every account may traverse, so that only the home's own modes stop the account nobody.
fn traversable_folder() -> (tempfile::TempDir, PathBuf) {
    let folder = tempfile::Builder::new()
        .prefix("q-")
        .tempdir_in("/tmp")
        .expect("a folder in /tmp");
    set_mode(folder.path(), 0o755);
    let path = folder.path().to_path_buf();
    (folder, path)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs `sudo -n` as root and as the account nobody, and /tmp, which nobody can traverse; CI runs it with `moon run quadratique-server:test-as-nobody`"]
async fn another_account_cannot_connect_to_the_socket_until_the_home_is_loosened() {
    let (_folder, parent) = traversable_folder();
    let home = parent.join("home");
    let paths = Paths::in_home(&home).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
    let socket = running.socket_path().to_path_buf();

    let as_made = connect_as_nobody(&socket);
    set_mode(&home, 0o755);
    let socket_private = connect_as_nobody(&socket);
    set_mode(&socket, 0o777);
    let both_loose = connect_as_nobody(&socket);
    set_mode(&home, 0o700);
    let folder_private = connect_as_nobody(&socket);

    running.shutdown().await;
    println!(
        "nobody connects to {}: home 0700 and socket 0600 (as the server makes them): {as_made}; home 0755 and socket 0600: {socket_private}; home 0755 and socket 0777: {both_loose}; home 0700 and socket 0777: {folder_private}",
        socket.display()
    );
    assert_eq!(
        (
            as_made.as_str(),
            socket_private.as_str(),
            both_loose.as_str(),
            folder_private.as_str()
        ),
        ("EACCES", "EACCES", "connected", "EACCES")
    );
}

#[tokio::test]
#[ignore = "needs `sudo -n` as root and as the account nobody, and /tmp, which nobody can traverse; CI runs it with `moon run quadratique-server:test-as-nobody`"]
async fn a_socket_or_a_link_of_another_account_is_refused_before_the_lock_as_another_account_owns_it()
 {
    let (_folder, parent) = traversable_folder();
    let home = parent.join("home");
    std::fs::create_dir(&home).expect("a home");
    set_mode(&home, 0o700);
    let socket = home.join(SOCKET_FILE);
    drop(std::os::unix::net::UnixListener::bind(&socket).expect("a socket"));
    as_root(&["chown", "nobody"], &socket);

    let refusal = refusal_before_the_lock(&home).await;
    println!("{refusal}");
    assert!(
        matches!(&refusal, HomeError::SocketEntry { what, .. } if what.starts_with("a socket of another account")),
        "{refusal}"
    );
    std::fs::remove_file(&socket).expect("the home's owner removes it");

    let link = parent.join("link");
    symlink(&home, &link).expect("a link to the home");
    as_root(&["chown", "-h", "nobody"], &link);

    let refusal = refusal_before_the_lock(&link).await;
    println!("{refusal}");
    assert!(
        matches!(&refusal, HomeError::ForeignLink { .. }),
        "{refusal}"
    );
}

/// A stop names the generation it means. Another generation gets `409` with the serving one, and the server keeps
/// serving on both listeners. Its own generation gets `202`: the client reads the whole answer, both listeners refuse
/// new connections from then on, and only then does the server ask to shut down.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_of_another_generation_gets_409_and_stops_nothing_and_a_stop_of_its_own_gets_202() {
    use futures::FutureExt;
    use server::StopAnswer;

    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
    let prepared = prepared_at(home.path());
    let socket = prepared.socket();
    let web = crate::support::web_address(&running).await;
    let generation = server::identity_over_socket(&prepared)
        .await
        .expect("the server answers")
        .generation
        .expect("a generation");
    let stale = if generation.starts_with('0') {
        "1"
    } else {
        "0"
    }
    .repeat(32);

    let refused = within(
        "a stop of another generation",
        server::stop_over_socket(&prepared, &stale, server::PROBE_LIMIT),
    )
    .await
    .expect("an answer");

    assert_eq!(
        refused,
        StopAnswer::OtherGeneration(Some(generation.clone()))
    );
    assert!(running.stop_requested().now_or_never().is_none());
    let still = server::identity_over_socket(&prepared)
        .await
        .expect("the socket still answers");
    assert_eq!(still.generation.as_deref(), Some(generation.as_str()));
    std::net::TcpStream::connect(web).expect("TCP still answers");

    let accepted = within(
        "a stop of this generation",
        server::stop_over_socket(&prepared, &generation, server::PROBE_LIMIT),
    )
    .await
    .expect("an answer");

    assert_eq!(accepted, StopAnswer::Accepted);
    within("the server's own stop", running.stop_requested()).await;
    // Only Linux takes a listening socket out of the listening state while the server still holds it; elsewhere a
    // connection made now waits in the backlog until the shutdown below drops the listeners.
    #[cfg(target_os = "linux")]
    {
        let socket_after = std::os::unix::net::UnixStream::connect(&socket);
        assert_eq!(
            socket_after.as_ref().err().map(std::io::Error::kind),
            Some(std::io::ErrorKind::ConnectionRefused),
            "{socket_after:?}"
        );
        let tcp_after = std::net::TcpStream::connect(web);
        assert_eq!(
            tcp_after.as_ref().err().map(std::io::Error::kind),
            Some(std::io::ErrorKind::ConnectionRefused),
            "{tcp_after:?}"
        );
    }
    within(
        "the shutdown after the stop",
        running.shutdown_draining(server::DRAIN),
    )
    .await;
    assert!(!socket.exists(), "the socket outlived its server");
}

/// A short limit for a client of the socket to send a whole request: the tests that show what it cuts off wait this
/// long, not the product's limit. It is far above the time a test's own requests take to arrive.
const SHORT_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);

/// Sends `bytes` on a new connection to the socket of `running`, then reads until the server closes the connection:
/// what it answered before it cut the connection off. Without a cut, the hang guard fails the test.
async fn answered_before_the_cut(running: &crate::support::Running, bytes: &[u8]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::UnixStream::connect(running.server.socket_path())
        .await
        .expect("a connection");
    stream.write_all(bytes).await.expect("the bytes are sent");
    let mut answered = Vec::new();
    within(
        "the server's cut of the connection",
        stream.read_to_end(&mut answered),
    )
    .await
    .expect("the server closes the connection");
    String::from_utf8_lossy(&answered).into_owned()
}

/// A client that sends part of a control request, its head or its body, is cut off at the socket's request limit, and
/// the server stops nothing and goes on answering.
#[tokio::test(flavor = "multi_thread")]
async fn a_partial_control_request_is_cut_off_at_the_limit_and_stops_nothing() {
    use futures::FutureExt;

    let board = crate::support::start_with_socket_limit(SHORT_LIMIT).await;
    let prepared = prepared_at(board.home());
    let generation = server::identity_over_socket(&prepared)
        .await
        .expect("the server answers")
        .generation
        .expect("a generation");
    let partial_head = "POST /server/stop HTTP/1.1\r\nHost: quadratique.invalid\r\n".to_owned();
    let partial_body = format!(
        "POST /server/stop HTTP/1.1\r\nHost: quadratique.invalid\r\nContent-Length: 200\r\n\r\n{{\"generation\":\"{generation}\""
    );

    for partial in [partial_head, partial_body] {
        let answered = answered_before_the_cut(&board, partial.as_bytes()).await;
        assert_eq!(answered, "", "{partial:?}");
    }

    assert!(board.server.stop_requested().now_or_never().is_none());
    let still = server::identity_over_socket(&prepared)
        .await
        .expect("the socket still answers");
    assert_eq!(still.generation, Some(generation));
    within("the shutdown", board.server.shutdown()).await;
}

/// The limit covers the body of a request to the command line's routes as it covers a control request's: a body that
/// never arrives whole is cut off.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_request_whose_body_never_arrives_whole_is_cut_off_at_the_limit() {
    let board = crate::support::start_with_socket_limit(SHORT_LIMIT).await;

    let answered = answered_before_the_cut(
        &board,
        b"POST /cli/capabilities/issue.list HTTP/1.1\r\nHost: quadratique.invalid\r\nContent-Type: application/json\r\nContent-Length: 1000\r\n\r\n{",
    )
    .await;

    assert_eq!(answered, "");
    within("the shutdown", board.server.shutdown()).await;
}

/// A complete answer that neither streams nor waits, here a `404`, lifts nothing: the connection that stays idle after
/// it is cut off at the limit.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_connection_after_a_complete_answer_is_cut_off_at_the_limit() {
    let board = crate::support::start_with_socket_limit(SHORT_LIMIT).await;

    let answered = answered_before_the_cut(
        &board,
        b"GET /cli/nothing HTTP/1.1\r\nHost: quadratique.invalid\r\n\r\n",
    )
    .await;

    assert!(answered.starts_with("HTTP/1.1 404"), "{answered}");
    within("the shutdown", board.server.shutdown()).await;
}

/// After a complete answer, the next request on the same connection is held to the limit again: sent in part, it is
/// cut off, and gets no answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_partial_request_after_a_complete_answer_on_the_same_connection_is_cut_off_at_the_limit()
{
    let board = crate::support::start_with_socket_limit(SHORT_LIMIT).await;

    let answered = answered_before_the_cut(
        &board,
        b"GET /cli/nothing HTTP/1.1\r\nHost: quadratique.invalid\r\n\r\nGET /server HTTP/1.1\r\nHost: quadratique.invalid\r\n",
    )
    .await;

    assert!(answered.starts_with("HTTP/1.1 404"), "{answered}");
    assert_eq!(answered.matches("HTTP/1.1 ").count(), 1, "{answered}");
    within("the shutdown", board.server.shutdown()).await;
}

type Occurrences = futures::stream::BoxStream<
    'static,
    Result<acid_cat_core::Occurrence, acid_cat_core::CallFailure>,
>;

/// The next occurrence that names `issue`; panics when the stream ends first.
async fn occurrence_naming(occurrences: &mut Occurrences, issue: &str) -> serde_json::Value {
    use futures::StreamExt;

    loop {
        match within("an occurrence", occurrences.next()).await {
            Some(Ok(occurrence)) if occurrence.data.to_string().contains(issue) => {
                return occurrence.data;
            }
            Some(Ok(_)) => {}
            ended => panic!("the event stream ended before an occurrence named {issue}: {ended:?}"),
        }
    }
}

/// The test helpers, as the command line, call the command line's routes and follow their event stream on the socket
/// only: a server that no browser used listens on no TCP port, and they work.
#[tokio::test(flavor = "multi_thread")]
async fn calls_and_their_event_stream_answer_on_the_socket_of_a_server_that_listens_on_no_tcp_port()
{
    let board = crate::support::start().await;

    let mut live = within(
        "the subscription",
        board.client(&board.agent("watcher")).subscribe(
            "issue.changed",
            &serde_json::json!({}),
            None,
            None,
        ),
    )
    .await
    .expect("the subscription starts on the socket");
    board.project("DEM").await;
    let issue = board.issue("Write the parser").await;

    let occurrence = occurrence_naming(&mut live, &issue).await;
    assert_eq!(occurrence["operation"], "issue.create", "{occurrence}");
    assert_eq!(board.server.web(), None);
    within("the shutdown", board.server.shutdown()).await;
}

/// An answer that streams has no limit once its request is complete: a partial request opened after the stream is cut
/// off at the limit, and the stream still delivers the next occurrence.
#[tokio::test(flavor = "multi_thread")]
async fn an_event_stream_on_the_socket_outlives_the_limit_of_a_request() {
    let board = crate::support::start_with_socket_limit(SHORT_LIMIT).await;
    board.project("DEM").await;
    let mut live = within(
        "the subscription",
        board.client(&board.agent("watcher")).subscribe(
            "issue.changed",
            &serde_json::json!({}),
            None,
            None,
        ),
    )
    .await
    .expect("the subscription starts on the socket");
    let answered = answered_before_the_cut(
        &board,
        b"GET /server HTTP/1.1\r\nHost: quadratique.invalid\r\n",
    )
    .await;
    assert_eq!(answered, "");

    let issue = board.issue("Write the parser").await;

    let occurrence = occurrence_naming(&mut live, &issue).await;
    assert_eq!(occurrence["operation"], "issue.create", "{occurrence}");
    within("the shutdown", board.server.shutdown()).await;
}

/// The command lines of releases 0.2 and 0.3 take an identity as a server only when it has these fields, the `url` as
/// a loopback address of the command line's routes (`background.rs` `on_socket_within` and `runtime_of` at v0.2.0 and
/// v0.3.0). Their `quadratique upgrade` waits for such an identity from the new build, with its own pid and
/// executable, after it restarts the unit (record 0004, §6). Anything else is `Malformed` to them, and the upgrade
/// fails.
#[tokio::test(flavor = "multi_thread")]
async fn the_identity_is_one_that_the_command_lines_of_releases_0_2_and_0_3_take_as_a_server() {
    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
    crate::support::web_address(&running).await;

    let identity = server::identity_over_socket(&prepared_at(home.path()))
        .await
        .expect("the identity on the socket");
    running.shutdown().await;

    let generation = identity.generation.clone().unwrap_or_default();
    assert!(
        generation.len() == 32 && generation.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{identity:?}"
    );
    assert_eq!(
        identity.database,
        home.path().canonicalize().unwrap().join("workspace.db")
    );
    assert_eq!(identity.control, Some(1));
    assert!(identity.started.is_some(), "{identity:?}");
    assert_eq!(identity.pid, std::process::id());
    assert!(identity.executable.is_some(), "{identity:?}");
    let url = identity.url.clone().unwrap_or_default();
    let (authority, path) = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split_once('/'))
        .unwrap_or_else(|| panic!("{url} is not http://<address>/<path>"));
    let address: std::net::SocketAddr = authority
        .parse()
        .unwrap_or_else(|_| panic!("{authority} is not an address"));
    assert!(address.ip().is_loopback(), "{url}");
    assert_eq!(path, "cli", "{url}");
}
