//! The home policy on its own, with no server: these tests run on Linux and macOS. Each refusal leaves no file in the
//! home, so no lock file either; tests/server/socket.rs shows the same refusals through a server's start on Linux.
#![cfg(unix)]

use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use quadratique_server::home::{self, HomeError, SOCKET_FILE, SOCKET_PATH_LIMIT};
use quadratique_server::server::Paths;

/// The policy on the home at `path`: the resolved folder it gives, or its refusal.
fn prepare(path: &Path) -> Result<PathBuf, HomeError> {
    let paths = Paths::in_home(path).expect("an absolute home");
    home::prepare(&paths).map(|prepared| prepared.folder().to_path_buf())
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

/// A folder whose own path passes the policy, so that each test sees only the refusal it sets up.
fn safe_folder() -> (tempfile::TempDir, PathBuf) {
    let folder = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("a temporary folder");
    let resolved = prepare(folder.path()).expect("the temporary folder passes the policy");
    (folder, resolved)
}

fn empty(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

#[test]
fn a_missing_home_is_made_private_with_every_missing_folder_above_it() {
    let (_folder, base) = safe_folder();
    let wanted = base.join("data/quadratique");

    let made = prepare(&wanted).expect("the home is made");

    assert_eq!(made, wanted);
    assert_eq!(mode(&base.join("data")), 0o700);
    assert_eq!(mode(&wanted), 0o700);
    assert!(empty(&wanted));
}

#[test]
fn a_folder_above_the_home_that_others_may_write_is_refused_unless_it_is_sticky() {
    let (_folder, base) = safe_folder();
    let shared = base.join("shared");
    std::fs::create_dir(&shared).expect("a shared folder");
    set_mode(&shared, 0o777);

    let refused = prepare(&shared.join("home"));

    let Err(HomeError::Replaceable { path, .. }) = &refused else {
        panic!("not refused as replaceable: {refused:?}");
    };
    assert_eq!(path, &shared);
    let message = refused.unwrap_err().to_string();
    assert!(
        message.starts_with(&format!("another account can replace {}", shared.display())),
        "{message}"
    );
    assert!(!shared.join("home").exists(), "the home was made");

    set_mode(&shared, 0o1777);
    prepare(&shared.join("home")).expect("a sticky folder is safe above the home");
}

#[test]
fn a_link_is_followed_only_when_no_other_account_can_change_it() {
    let (_folder, base) = safe_folder();
    let real = base.join("real");
    std::fs::create_dir(&real).expect("the real home");
    set_mode(&real, 0o700);
    let safe_link = base.join("safe");
    symlink(&real, &safe_link).expect("a link in a safe folder");

    assert_eq!(prepare(&safe_link).expect("the link is followed"), real);

    let shared = base.join("shared");
    std::fs::create_dir(&shared).expect("a shared folder");
    let exposed_link = shared.join("home");
    symlink(&real, &exposed_link).expect("a link in a folder others may write");
    set_mode(&shared, 0o777);

    let refused = prepare(&exposed_link);

    let Err(HomeError::ForeignLink { link, .. }) = &refused else {
        panic!("not refused as a foreign link: {refused:?}");
    };
    assert_eq!(link, &exposed_link);
    assert!(
        refused
            .unwrap_err()
            .to_string()
            .contains("is a link that another account could change"),
    );
    set_mode(&shared, 0o755);
}

#[test]
fn a_home_that_a_group_or_other_accounts_may_use_is_refused_and_left_as_it_is() {
    let (_folder, base) = safe_folder();
    let shared = base.join("home");
    std::fs::create_dir(&shared).expect("a home");

    for loose in [0o770, 0o750, 0o705] {
        set_mode(&shared, loose);

        let refused = prepare(&shared);

        let Err(HomeError::NotPrivate { .. }) = &refused else {
            panic!("mode {loose:o} was not refused: {refused:?}");
        };
        let message = refused.unwrap_err().to_string();
        assert!(
            message.contains(&format!("chmod 700 {}", shared.display())),
            "{message}"
        );
        assert_eq!(mode(&shared), loose, "the policy changed the mode");
        assert!(empty(&shared));
    }
}

#[test]
fn anything_but_a_socket_of_this_account_at_the_socket_name_is_refused() {
    let (_folder, base) = safe_folder();
    let socket = base.join(SOCKET_FILE);

    std::fs::write(&socket, "notes").expect("a regular file");
    let refused = prepare(&base);
    assert!(
        matches!(&refused, Err(HomeError::SocketEntry { what, .. }) if what == "a regular file"),
        "{refused:?}"
    );
    std::fs::remove_file(&socket).expect("the file goes");

    std::fs::create_dir(&socket).expect("a folder");
    let refused = prepare(&base);
    assert!(
        matches!(&refused, Err(HomeError::SocketEntry { what, .. }) if what == "a folder"),
        "{refused:?}"
    );
    std::fs::remove_dir(&socket).expect("the folder goes");

    symlink("/dev/null", &socket).expect("a link");
    let refused = prepare(&base);
    assert!(
        matches!(&refused, Err(HomeError::SocketEntry { what, .. }) if what == "a link"),
        "{refused:?}"
    );
    std::fs::remove_file(&socket).expect("the link goes");

    stop_listening(std::os::unix::net::UnixListener::bind(&socket).expect("a socket"));
    prepare(&base).expect("a stale socket of this account is accepted");
}

/// Making a socket of another account needs root; tests/server/socket.rs does that where `sudo` works.
#[test]
fn a_socket_of_another_account_is_named_as_such() {
    let (_folder, base) = safe_folder();
    let socket = base.join(SOCKET_FILE);
    let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("a socket");
    let meta = std::fs::symlink_metadata(&socket).expect("the socket");
    let me = meta.uid();

    assert_eq!(home::foreign_entry(meta.file_type(), me, me), None);
    assert_eq!(
        home::foreign_entry(meta.file_type(), me + 1, me).as_deref(),
        Some(format!("a socket of another account (uid {})", me + 1).as_str())
    );
}

/// A short folder that this test controls, so that the limit is reached whatever the system's temporary folder is.
fn short_folder() -> (tempfile::TempDir, PathBuf) {
    let folder = tempfile::Builder::new()
        .prefix("q")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .expect("a folder in /tmp");
    let resolved = prepare(folder.path()).expect("the folder passes the policy");
    (folder, resolved)
}

/// The home whose socket path is `length` bytes long, in `base`.
fn home_with_socket_path_of(base: &Path, length: usize) -> PathBuf {
    let fixed = base.as_os_str().len() + 1 + 1 + SOCKET_FILE.len();
    assert!(
        length > fixed,
        "{} is too long for this test",
        base.display()
    );
    base.join("h".repeat(length - fixed))
}

/// `lstat` needs only to search the home; `bind` makes an entry in it. In a home of mode 0500 the policy passes and the
/// bind is what fails, and the refusal says so.
#[tokio::test]
async fn a_home_this_account_cannot_write_refuses_the_bind_and_names_it() {
    let (_folder, home) = safe_folder();
    let prepared = home::prepare(&Paths::in_home(&home).expect("a home")).expect("a home");
    set_mode(&home, 0o500);

    let refused = home::bind_socket(&prepared).await;

    set_mode(&home, 0o700);
    let Err(error @ HomeError::Bind { .. }) = &refused else {
        panic!("the bind was not refused as a bind: {refused:?}");
    };
    assert!(error.denied().is_some(), "{error}");
    assert!(
        error.to_string().starts_with(&format!(
            "the server cannot make its socket {}: Permission denied",
            prepared.socket().display()
        )),
        "{error}"
    );
}

#[tokio::test]
async fn the_socket_path_may_be_as_long_as_the_system_allows_and_not_one_byte_longer() {
    let (_folder, base) = short_folder();
    let at_the_limit = home_with_socket_path_of(&base, SOCKET_PATH_LIMIT);
    let over_the_limit = home_with_socket_path_of(&base, SOCKET_PATH_LIMIT + 1);

    let prepared = home::prepare(&Paths::in_home(&at_the_limit).expect("a home"))
        .expect("a socket path at the limit is accepted");
    assert_eq!(prepared.socket().as_os_str().len(), SOCKET_PATH_LIMIT);
    let (_listener, file) = home::bind_socket(&prepared)
        .await
        .expect("the server's bind works at the limit");
    std::os::unix::net::UnixStream::connect(file.path()).expect("a connect works at the limit");

    let refused = prepare(&over_the_limit);

    let Err(error @ HomeError::SocketPathTooLong { length, limit, .. }) = &refused else {
        panic!("one byte over the limit was not refused: {refused:?}");
    };
    assert_eq!(
        (*length, *limit),
        (SOCKET_PATH_LIMIT + 1, SOCKET_PATH_LIMIT)
    );
    let message = error.to_string();
    assert!(
        message.contains(&format!("{} bytes long", SOCKET_PATH_LIMIT + 1))
            && message.contains("set QUADRATIQUE_HOME to a shorter folder"),
        "{message}"
    );
    assert!(!over_the_limit.exists(), "the refused home was made");
    std::fs::create_dir(&over_the_limit).expect("the folder, made by hand");
    let over = over_the_limit.join(SOCKET_FILE);
    assert!(
        std::os::unix::net::UnixListener::bind(&over).is_err(),
        "the system binds one byte over the limit"
    );
}

#[test]
fn a_missing_folder_followed_by_dot_dot_is_refused_before_anything_is_made() {
    let (_folder, base) = safe_folder();
    let wanted = base.join("missing/../home");

    let refused = prepare(&wanted);

    assert!(
        matches!(&refused, Err(HomeError::MissingThenParent(path)) if path == &base.join("missing")),
        "{refused:?}"
    );
    assert!(empty(&base), "something was made in {}", base.display());
}

/// The ACL is written as the kernel stores it: `struct posix_acl_xattr_header`, then (tag, perm, id) entries. `false`
/// when the file system takes no extended attributes.
#[cfg(target_os = "linux")]
fn write_acl(path: &Path, name: &str, named_user_perm: u16, mask: u16) -> bool {
    const UNDEFINED: u32 = u32::MAX;
    let entries: [(u16, u16, u32); 5] = [
        (0x01, 0o7, UNDEFINED),
        (0x02, named_user_perm, 65534),
        (0x04, 0, UNDEFINED),
        (0x10, mask, UNDEFINED),
        (0x20, 0, UNDEFINED),
    ];
    let mut value = 2u32.to_le_bytes().to_vec();
    for (tag, perm, id) in entries {
        value.extend(tag.to_le_bytes());
        value.extend(perm.to_le_bytes());
        value.extend(id.to_le_bytes());
    }
    match rustix::fs::setxattr(path, name, &value, rustix::fs::XattrFlags::empty()) {
        Ok(()) => true,
        Err(rustix::io::Errno::NOTSUP) => {
            println!(
                "skipped: the file system of {} takes no extended attributes, so it holds no ACL to refuse",
                path.display()
            );
            false
        }
        Err(error) => panic!("{} takes no ACL: {error}", path.display()),
    }
}

#[cfg(target_os = "linux")]
const ACCESS: &str = "system.posix_acl_access";

#[cfg(target_os = "linux")]
#[test]
fn an_acl_that_gives_another_account_access_is_refused_whatever_the_mode_says() {
    let (_folder, base) = safe_folder();
    let leaf = base.join("home");
    std::fs::create_dir(&leaf).expect("a home");
    set_mode(&leaf, 0o700);
    if !write_acl(&leaf, ACCESS, 0o5, 0) {
        return;
    }
    assert_eq!(mode(&leaf), 0o700, "the mask leaves the mode private");

    let refused = prepare(&leaf);

    assert!(
        matches!(&refused, Err(HomeError::SharedAccess { path, .. }) if path == &leaf),
        "{refused:?}"
    );
    let message = refused.unwrap_err().to_string();
    assert!(
        message.contains("gives access to another account: its ACL lets user 65534 use it")
            && message.contains(&format!("setfacl -b {}", leaf.display())),
        "{message}"
    );

    let parent = base.join("parent");
    std::fs::create_dir(&parent).expect("a folder above the home");
    set_mode(&parent, 0o755);
    write_acl(&parent, ACCESS, 0o7, 0o5);
    assert_eq!(mode(&parent), 0o750, "the mask gives the group no write");

    let refused = prepare(&parent.join("home"));

    assert!(
        matches!(&refused, Err(HomeError::SharedAccess { path, .. }) if path == &parent),
        "{refused:?}"
    );
}

/// A default ACL gives its entries to everything made in the folder later, the store and the socket included.
#[cfg(target_os = "linux")]
#[test]
fn a_default_acl_that_gives_another_account_access_is_refused() {
    let (_folder, base) = safe_folder();
    let leaf = base.join("home");
    std::fs::create_dir(&leaf).expect("a home");
    set_mode(&leaf, 0o700);
    if !write_acl(&leaf, "system.posix_acl_default", 0o6, 0o6) {
        return;
    }
    assert_eq!(mode(&leaf), 0o700, "a default ACL leaves the mode as it is");

    let refused = prepare(&leaf);

    let message = refused.as_ref().map_err(ToString::to_string).unwrap_err();
    assert!(
        matches!(&refused, Err(HomeError::SharedAccess { path, .. }) if path == &leaf),
        "{refused:?}"
    );
    assert!(
        message.contains("user 65534, in its default ACL,")
            && message.contains(&format!("setfacl -k {}", leaf.display())),
        "{message}"
    );
}

#[cfg(target_os = "macos")]
fn add_acl(path: &Path, entry: &str) {
    let status = std::process::Command::new("/bin/chmod")
        .args(["+a", entry])
        .arg(path)
        .status()
        .expect("chmod runs");
    assert!(status.success(), "chmod +a {entry:?} failed");
}

#[cfg(target_os = "macos")]
#[test]
fn an_acl_entry_that_allows_access_is_refused_and_a_deny_entry_is_not() {
    let (_folder, base) = safe_folder();
    let leaf = base.join("home");
    std::fs::create_dir(&leaf).expect("a home");
    set_mode(&leaf, 0o700);
    add_acl(&leaf, "everyone deny delete");

    prepare(&leaf).expect("a deny entry gives no access");

    add_acl(&leaf, "everyone allow list");
    let refused = prepare(&leaf);
    assert!(
        matches!(&refused, Err(HomeError::SharedAccess { path, .. }) if path == &leaf),
        "{refused:?}"
    );

    let parent = base.join("parent");
    std::fs::create_dir(&parent).expect("a folder above the home");
    set_mode(&parent, 0o755);
    add_acl(&parent, "everyone allow add_subdirectory");
    let refused = prepare(&parent.join("home"));
    assert!(
        matches!(&refused, Err(HomeError::SharedAccess { path, .. }) if path == &parent),
        "{refused:?}"
    );
}

/// A macOS ACL entry can let an account remove what a folder holds whatever its sticky bit says, so the ACL of a
/// sticky folder above the home is read too.
#[cfg(target_os = "macos")]
#[test]
fn an_acl_entry_on_a_sticky_folder_above_the_home_is_refused() {
    let (_folder, base) = safe_folder();
    let sticky = base.join("sticky");
    std::fs::create_dir(&sticky).expect("a folder above the home");
    set_mode(&sticky, 0o1777);
    prepare(&sticky.join("home")).expect("a sticky folder without an ACL is safe");

    add_acl(&sticky, "everyone allow delete_child");
    let refused = prepare(&sticky.join("other"));

    assert!(
        matches!(&refused, Err(HomeError::SharedAccess { path, .. }) if path == &sticky),
        "{refused:?}"
    );
    let message = refused.unwrap_err().to_string();
    assert!(
        message.contains(&format!("chmod -N {}", sticky.display())),
        "{message}"
    );
}

/// Takes `listener` out of the listening state for every descriptor of it, then closes it: later connections are
/// refused. A plain drop closes one descriptor, while a child that another test spawns holds a copy of each descriptor
/// of this process between its `fork` and its `exec`, and so keeps the socket listening for that moment. This relies on
/// Linux's `shutdown`, as `serve.rs` does; elsewhere the listener is only dropped.
fn stop_listening(listener: std::os::unix::net::UnixListener) {
    #[cfg(target_os = "linux")]
    rustix::net::shutdown(&listener, rustix::net::Shutdown::Both)
        .expect("the socket stops listening");
    drop(listener);
}
