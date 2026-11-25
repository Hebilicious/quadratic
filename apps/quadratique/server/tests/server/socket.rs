//! The server's Unix socket, and the home policy as the server applies it before it opens its lock file.

use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use quadratique_server::home::{self, HomeError, SOCKET_FILE};
use quadratique_server::serve;
use quadratique_server::server::{self, CONTROL, Paths, RuntimeFile, ServerError, Started};

use crate::support::within;

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

    let with_acl = base.join("with-acl");
    std::fs::create_dir(&with_acl).expect("a home");
    set_mode(&with_acl, 0o700);
    write_named_user_acl(&with_acl);
    let refusal = refusal_before_the_lock(&with_acl).await;
    assert!(
        matches!(&refusal, HomeError::SharedAccess { .. }),
        "{refusal}"
    );

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

/// User 65534 may read and enter the folder; the mask of `---` leaves its mode at 0700.
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
    let runtime: RuntimeFile =
        serde_json::from_slice(&std::fs::read(&paths.runtime).expect("the runtime file"))
            .expect("JSON");

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
    assert_eq!(identity.url.as_deref(), Some(runtime.url.as_str()));
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
    let over_tcp = server::identity(running.address())
        .await
        .expect("the identity over TCP");
    assert_eq!(over_tcp, identity);

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
    let stale = std::os::unix::net::UnixListener::bind(&stale_path).expect("a stale socket");
    drop(stale);
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
    let served = serve::serve_socket(listener, file, Vec::new());
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

/// The runtime file is written through a FIFO whose pipe this test fills first, so the server's write blocks in the
/// middle of publishing its runtime file. The test knows it has reached that point when a descriptor of this process
/// other than its own two names the FIFO. Meanwhile the socket queues connections and answers none.
#[tokio::test(flavor = "multi_thread")]
async fn the_socket_answers_only_once_the_runtime_file_is_written() {
    use std::io::{Read, Write};
    use std::os::unix::fs::OpenOptionsExt;

    let nonblocking = rustix::fs::OFlags::NONBLOCK.bits() as i32;
    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let partial = home
        .path()
        .join(format!("server.json.{}", std::process::id()));
    let made = std::process::Command::new("mkfifo")
        .arg(&partial)
        .status()
        .expect("mkfifo runs");
    assert!(made.success());
    let mut reader = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nonblocking)
        .open(&partial)
        .expect("the FIFO's reading end");
    let mut filler = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(nonblocking)
        .open(&partial)
        .expect("a writing end of the test's own");
    let chunk = [b' '; 4096];
    let mut filled = 0;
    loop {
        match filler.write(&chunk) {
            Ok(written) => filled += written,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("filling the pipe: {error}"),
        }
    }
    drop(filler);
    let prepared = prepared_at(home.path());
    // From a thread outside the runtime's workers, the tasks that the start spawns go to the shared queue: other
    // workers run them while this thread blocks in the write, as they would in a server that is not under test.
    let starting = {
        let paths = paths.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            runtime.block_on(server::start(&paths, server::loopback()))
        })
    };
    within("the server's write into the full pipe", async {
        while descriptors_of(&partial) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;

    let while_publishing =
        server::identity_over_socket_within(&prepared, std::time::Duration::from_millis(300)).await;

    let mut written = Vec::new();
    within("the runtime file's text", async {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => written.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    tokio::task::yield_now().await;
                }
                Err(error) => panic!("reading the pipe: {error}"),
            }
        }
    })
    .await;
    let running = within("the start", starting)
        .await
        .expect("the start's task")
        .expect("the server starts");
    std::fs::remove_file(&paths.runtime).expect("the FIFO, renamed to the runtime file, goes");
    std::fs::write(&paths.runtime, &written[filled..])
        .expect("the runtime file, as a regular file");
    let after = server::identity_over_socket(&prepared).await;
    running.shutdown().await;
    assert!(
        matches!(while_publishing, Err(server::Unanswered::Stalled)),
        "the socket answered before the runtime file was written: {while_publishing:?}"
    );
    assert!(after.is_ok(), "{after:?}");
}

/// How many descriptors of this process name `path`.
fn descriptors_of(path: &Path) -> usize {
    std::fs::read_dir("/proc/self/fd")
        .expect("this process's descriptors")
        .flatten()
        .filter(|entry| std::fs::read_link(entry.path()).is_ok_and(|target| target == path))
        .count()
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
