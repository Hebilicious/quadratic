//! The server's Unix socket, and the home policy as the server applies it before it opens its lock file.

use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use quadratique_server::home::{HomeError, SOCKET_FILE};
use quadratique_server::server::{self, CONTROL, Paths, RuntimeFile, ServerError, Started};

use crate::support::within;

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
        server::identity_over_socket(home.path()),
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
    let next = server::identity_over_socket(home.path())
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
    let stale_inode = std::fs::symlink_metadata(&stale_path).unwrap().ino();

    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts over a stale socket");

    let bound = std::fs::symlink_metadata(&stale_path).expect("the new socket");
    assert_ne!(bound.ino(), stale_inode);
    server::identity_over_socket(home.path())
        .await
        .expect("the new socket answers");
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

    let probed = server::identity_over_socket(home.path()).await;

    assert!(
        matches!(&probed, Err(server::Unanswered::NotReady(why)) if why.contains("mode 660")),
        "{probed:?}"
    );
    running.shutdown().await;
}

/// A hard link reaches the same socket as its name, and stays when the server removes the name: through it, the test
/// connects after the drop, while it holds another copy of the listener's descriptor, as a child between its `fork`
/// and its `exec` does.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_server_refuses_connections_on_its_socket_while_another_copy_of_it_is_open() {
    let home = crate::support::private_home();
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
    let socket = running.socket_path().to_path_buf();
    let other_name = home.path().join("same-socket");
    std::fs::hard_link(&socket, &other_name).expect("a hard link to the socket");
    std::os::unix::net::UnixStream::connect(&other_name).expect("the link reaches the server");
    let copy = copy_of_listener(&socket);
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(paths.lock())
        .expect("the home's lock file");

    drop(running);

    within(
        "the home's lock",
        tokio::task::spawn_blocking(move || lock.lock().expect("the lock, once free")),
    )
    .await
    .expect("the waiting thread");
    assert!(!socket.exists(), "the socket's name outlived its server");
    let refused = std::os::unix::net::UnixStream::connect(&other_name);
    assert_eq!(
        refused.as_ref().err().map(std::io::Error::kind),
        Some(std::io::ErrorKind::ConnectionRefused),
        "{refused:?}"
    );
    drop(copy);
}

/// A new descriptor of this process's listening socket bound at `path`.
fn copy_of_listener(path: &Path) -> std::os::fd::OwnedFd {
    use rustix::process::{PidfdFlags, PidfdGetfdFlags, getpid, pidfd_getfd, pidfd_open};

    let inode = std::fs::read_to_string("/proc/net/unix")
        .expect("the kernel's Unix sockets")
        .lines()
        .find(|line| line.ends_with(&format!(" {}", path.display())))
        .and_then(|line| line.split_whitespace().nth(6)?.parse::<u64>().ok())
        .expect("the listening socket in /proc/net/unix");
    let socket = format!("socket:[{inode}]");
    let descriptor = std::fs::read_dir("/proc/self/fd")
        .expect("this process's descriptors")
        .flatten()
        .find(|entry| {
            std::fs::read_link(entry.path())
                .is_ok_and(|target| target.as_os_str() == socket.as_str())
        })
        .and_then(|entry| entry.file_name().to_str()?.parse().ok())
        .expect("a descriptor of the socket in this process");
    let this_process = pidfd_open(getpid(), PidfdFlags::empty()).expect("a pidfd of this process");
    pidfd_getfd(this_process, descriptor, PidfdGetfdFlags::empty())
        .expect("a copy of the descriptor")
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
