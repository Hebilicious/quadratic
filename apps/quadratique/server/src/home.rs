//! The data folder, which this code calls the home: the one policy that the server and the command line apply to it
//! before either opens a lock file, touches the store or starts a process, and the socket that the server binds in it.
//! docs/decisions/0004-a-socket-for-commands-and-a-token-for-the-browser.md, §1.
//!
//! The policy defends against other accounts of the machine, not against processes of this account, which already
//! hold its files. A folder that another account could rename, replace or enter would let that account put its own
//! socket, lock or store where this account's processes look for them.

use std::path::PathBuf;

pub const SOCKET_FILE: &str = "server.sock";

/// The longest socket path, in bytes, that `bind` and `connect` accept: the size of `sun_path` less its final NUL.
pub const SOCKET_PATH_LIMIT: usize = if cfg!(target_os = "macos") { 103 } else { 107 };

#[derive(Debug, thiserror::Error)]
pub enum HomeError {
    #[error("{} is a link that another account could change: {reason}", .link.display())]
    ForeignLink { link: PathBuf, reason: String },
    #[error("another account can replace {}: {reason}", .path.display())]
    Replaceable { path: PathBuf, reason: String },
    #[error("{} gives access to another account: {reason}", .path.display())]
    SharedAccess { path: PathBuf, reason: String },
    #[error("{} is not private: {reason}", .path.display())]
    NotPrivate { path: PathBuf, reason: String },
    #[error("{} is not a folder", .0.display())]
    NotAFolder(PathBuf),
    #[error(
        "the server's socket would be {}, {length} bytes long, and this system allows at most {limit}: set QUADRATIQUE_HOME to a shorter folder",
        .path.display()
    )]
    SocketPathTooLong {
        path: PathBuf,
        length: usize,
        limit: usize,
    },
    #[error(
        "{} is {what}, where only this account's server socket may be: Quadratique does not remove it; remove it yourself if nothing needs it",
        .path.display()
    )]
    SocketEntry { path: PathBuf, what: String },
    #[error("{} cannot be examined: {error}", .path.display())]
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
}

impl HomeError {
    /// `EACCES`, `EPERM` or `EROFS`: something, often a sandbox, denies this process the folder, rather than the
    /// folder being unsafe.
    pub fn denied(&self) -> Option<&std::io::Error> {
        match self {
            HomeError::Io { error, .. }
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
                ) =>
            {
                Some(error)
            }
            _ => None,
        }
    }
}

/// A path as a person types it in a POSIX shell.
pub fn shell_word(path: &str) -> String {
    if path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    {
        path.to_owned()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

#[cfg(unix)]
pub use unix::*;

/// Windows has no modes to check, and the server refuses to start there (QUA-61): the home is only made.
#[cfg(not(unix))]
pub fn prepare(home: &std::path::Path) -> Result<PathBuf, HomeError> {
    let io = |error| HomeError::Io {
        path: home.to_path_buf(),
        error,
    };
    let home = std::path::absolute(home).map_err(io)?;
    std::fs::create_dir_all(&home).map_err(io)?;
    Ok(home)
}

#[cfg(unix)]
mod unix {
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::fs::Metadata;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
    use std::path::{Component, Path, PathBuf};

    use super::{HomeError, SOCKET_FILE, SOCKET_PATH_LIMIT, shell_word};

    const GROUP_OR_OTHER_WRITE: u32 = 0o022;
    const GROUP_OR_OTHER_ANY: u32 = 0o077;
    const STICKY: u32 = 0o1000;
    const SOCKET_MODE: u32 = 0o600;
    /// As many links as Linux follows in one path before `ELOOP`.
    const LINKS: usize = 40;
    const ROOT: u32 = 0;

    /// Why a folder's entries are not this account's alone.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Exposure {
        OtherOwner(u32),
        Writable(u32),
    }

    /// The rule for a folder whose entries no other account may change, shared by the home and the upgrade's install
    /// folder: the folder belongs to this account or to root, and no group or other account may write it. A folder
    /// above it may be writable by others only when it is sticky, as `/tmp` is, so that they cannot rename or remove
    /// what they do not own.
    pub fn exposure(owner: u32, mode: u32, me: u32, is_the_folder: bool) -> Option<Exposure> {
        if owner != me && owner != ROOT {
            return Some(Exposure::OtherOwner(owner));
        }
        let sticky = mode & STICKY != 0;
        (mode & GROUP_OR_OTHER_WRITE != 0 && (is_the_folder || !sticky))
            .then_some(Exposure::Writable(mode & 0o7777))
    }

    pub fn own_account() -> u32 {
        rustix::process::geteuid().as_raw()
    }

    /// Checks the home, makes it when it is missing, and gives the path that all later work uses: the home with every
    /// link resolved.
    ///
    /// 1. Every component is examined with `lstat`. A link is followed only when it and the folder that holds it
    ///    belong to this account or to root, and that folder passes rule 2.
    /// 2. Every folder above the home passes [`exposure`].
    /// 3. The home, and every folder above it that this account owns and that is not sticky, carry no extended ACL
    ///    entry that lets another user or group write it, or, for the home, use it at all.
    /// 4. A missing home, and any missing folder above it, is made with mode 0700 and checked again. A home whose
    ///    mode gives a group or other accounts any access is refused, never changed.
    /// 5. The socket's path fits in `sun_path`, and the socket's name holds nothing, or a socket of this account.
    pub fn prepare(home: &Path) -> Result<PathBuf, HomeError> {
        let me = own_account();
        let home = std::path::absolute(home).map_err(at(home))?;
        let leaf = match walk(&home, me)? {
            Walked::Whole(leaf) => leaf,
            Walked::Missing { existing, missing } => {
                check_ancestors(&existing, me)?;
                let wanted: PathBuf = missing
                    .iter()
                    .fold(existing.clone(), |path, name| path.join(name));
                check_socket_path(&wanted.join(SOCKET_FILE))?;
                make(&existing, &missing)?;
                match walk(&home, me)? {
                    Walked::Whole(leaf) => leaf,
                    Walked::Missing { existing, missing } => {
                        let path = existing.join(missing.first().cloned().unwrap_or_default());
                        return Err(HomeError::Io {
                            path,
                            error: std::io::ErrorKind::NotFound.into(),
                        });
                    }
                }
            }
        };
        if let Some(parent) = leaf.parent() {
            check_ancestors(parent, me)?;
        }
        check_leaf(&leaf, me)?;
        let socket = leaf.join(SOCKET_FILE);
        check_socket_path(&socket)?;
        check_socket_entry(&socket, me)?;
        Ok(leaf)
    }

    enum Walked {
        Whole(PathBuf),
        Missing {
            existing: PathBuf,
            missing: Vec<OsString>,
        },
    }

    enum Step {
        Name(OsString),
        Parent,
    }

    fn steps(path: &Path) -> impl Iterator<Item = Step> + '_ {
        path.components().filter_map(|component| match component {
            Component::Normal(name) => Some(Step::Name(name.to_os_string())),
            Component::ParentDir => Some(Step::Parent),
            Component::RootDir | Component::CurDir | Component::Prefix(_) => None,
        })
    }

    /// The path as the kernel would resolve it, one component at a time, following only the links that rule 1
    /// allows. `resolved` never holds a link, so its parent is its real parent.
    fn walk(home: &Path, me: u32) -> Result<Walked, HomeError> {
        let mut resolved = PathBuf::from("/");
        let mut pending: VecDeque<Step> = steps(home).collect();
        let mut links = 0;
        while let Some(step) = pending.pop_front() {
            let name = match step {
                Step::Parent => {
                    resolved.pop();
                    continue;
                }
                Step::Name(name) => name,
            };
            let candidate = resolved.join(&name);
            let meta = match std::fs::symlink_metadata(&candidate) {
                Ok(meta) => meta,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let missing = std::iter::once(name)
                        .chain(pending.into_iter().filter_map(|step| match step {
                            Step::Name(name) => Some(name),
                            Step::Parent => None,
                        }))
                        .collect();
                    return Ok(Walked::Missing {
                        existing: resolved,
                        missing,
                    });
                }
                Err(error) => return Err(at(&candidate)(error)),
            };
            if meta.file_type().is_symlink() {
                check_link(&candidate, &meta, &resolved, me)?;
                links += 1;
                if links > LINKS {
                    return Err(HomeError::Io {
                        path: home.to_path_buf(),
                        error: std::io::Error::other("too many links"),
                    });
                }
                let target = std::fs::read_link(&candidate).map_err(at(&candidate))?;
                if target.is_absolute() {
                    resolved = PathBuf::from("/");
                }
                for step in steps(&target).collect::<Vec<_>>().into_iter().rev() {
                    pending.push_front(step);
                }
                continue;
            }
            if !meta.is_dir() {
                return Err(HomeError::NotAFolder(candidate));
            }
            resolved = candidate;
        }
        Ok(Walked::Whole(resolved))
    }

    fn check_link(link: &Path, meta: &Metadata, holder: &Path, me: u32) -> Result<(), HomeError> {
        let foreign = |reason: String| HomeError::ForeignLink {
            link: link.to_path_buf(),
            reason,
        };
        if meta.uid() != me && meta.uid() != ROOT {
            return Err(foreign(format!(
                "it belongs to another account (uid {})",
                meta.uid()
            )));
        }
        check_ancestors(holder, me).map_err(|refusal| foreign(refusal.to_string()))
    }

    /// `folder` and every folder above it, as folders above the home.
    fn check_ancestors(folder: &Path, me: u32) -> Result<(), HomeError> {
        for ancestor in folder.ancestors() {
            let meta = std::fs::symlink_metadata(ancestor).map_err(at(ancestor))?;
            let replaceable = |reason: String| HomeError::Replaceable {
                path: ancestor.to_path_buf(),
                reason,
            };
            match exposure(meta.uid(), meta.mode(), me, false) {
                Some(Exposure::OtherOwner(uid)) => {
                    return Err(replaceable(format!(
                        "it belongs to another account (uid {uid})"
                    )));
                }
                Some(Exposure::Writable(mode)) => {
                    return Err(replaceable(format!(
                        "its mode is {mode:o}, which lets a group or other accounts rename what it holds, and it is not sticky; remove their write access (chmod go-w {})",
                        shell_word(&ancestor.display().to_string())
                    )));
                }
                None => {}
            }
            if meta.uid() == me
                && meta.mode() & STICKY == 0
                && let Some(entry) = acl::granted(ancestor, acl::Access::Write)?
            {
                return Err(HomeError::SharedAccess {
                    path: ancestor.to_path_buf(),
                    reason: format!("its ACL lets {entry} write it"),
                });
            }
        }
        Ok(())
    }

    fn check_leaf(leaf: &Path, me: u32) -> Result<(), HomeError> {
        let meta = std::fs::symlink_metadata(leaf).map_err(at(leaf))?;
        let not_private = |reason: String| HomeError::NotPrivate {
            path: leaf.to_path_buf(),
            reason,
        };
        if meta.uid() != me {
            return Err(not_private(format!(
                "it belongs to another account (uid {})",
                meta.uid()
            )));
        }
        if let Some(entry) = acl::granted(leaf, acl::Access::Any)? {
            return Err(HomeError::SharedAccess {
                path: leaf.to_path_buf(),
                reason: format!("its ACL lets {entry} use it"),
            });
        }
        let mode = meta.permissions().mode() & 0o777;
        if mode & GROUP_OR_OTHER_ANY != 0 {
            return Err(not_private(format!(
                "its mode is {mode:o}, which lets a group or other accounts use it; run chmod 700 {}",
                shell_word(&leaf.display().to_string())
            )));
        }
        Ok(())
    }

    /// Each missing folder is made private from the start, never made and then changed.
    fn make(existing: &Path, missing: &[OsString]) -> Result<(), HomeError> {
        let mut folder = existing.to_path_buf();
        for name in missing {
            folder.push(name);
            match std::fs::DirBuilder::new().mode(0o700).create(&folder) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(at(&folder)(error)),
            }
        }
        Ok(())
    }

    pub fn check_socket_path(socket: &Path) -> Result<(), HomeError> {
        let length = socket.as_os_str().as_bytes().len();
        if length > SOCKET_PATH_LIMIT {
            return Err(HomeError::SocketPathTooLong {
                path: socket.to_path_buf(),
                length,
                limit: SOCKET_PATH_LIMIT,
            });
        }
        Ok(())
    }

    /// What may stand at the socket's name: nothing, or a socket of this account, which a server that holds the
    /// home's lock replaces. Anything else is never removed.
    pub fn foreign_entry(file_type: std::fs::FileType, owner: u32, me: u32) -> Option<String> {
        if file_type.is_socket() && owner == me {
            return None;
        }
        let kind = if file_type.is_socket() {
            "a socket"
        } else if file_type.is_symlink() {
            "a link"
        } else if file_type.is_dir() {
            "a folder"
        } else if file_type.is_file() {
            "a regular file"
        } else {
            "a special file"
        };
        Some(if owner == me {
            kind.to_owned()
        } else {
            format!("{kind} of another account (uid {owner})")
        })
    }

    fn check_socket_entry(socket: &Path, me: u32) -> Result<(), HomeError> {
        match std::fs::symlink_metadata(socket) {
            Ok(meta) => match foreign_entry(meta.file_type(), meta.uid(), me) {
                Some(what) => Err(HomeError::SocketEntry {
                    path: socket.to_path_buf(),
                    what,
                }),
                None => Ok(()),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(at(socket)(error)),
        }
    }

    /// Why a client must not trust the entry at the socket's name yet.
    #[derive(Debug)]
    pub enum NotReady {
        Missing,
        Denied(std::io::Error),
        /// Another kind of entry, an entry of another account, or a socket whose mode is not yet 0600.
        Untrusted(String),
        Io(std::io::Error),
    }

    /// A client trusts the socket only when it is a socket of this account with mode 0600: the server sets that mode
    /// before it accepts a connection.
    pub fn socket_ready(socket: &Path) -> Result<(), NotReady> {
        let meta = match std::fs::symlink_metadata(socket) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(NotReady::Missing);
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(NotReady::Denied(error));
            }
            Err(error) => return Err(NotReady::Io(error)),
        };
        if let Some(what) = foreign_entry(meta.file_type(), meta.uid(), own_account()) {
            return Err(NotReady::Untrusted(format!(
                "{} is {what}",
                socket.display()
            )));
        }
        let mode = meta.permissions().mode() & 0o777;
        if mode != SOCKET_MODE {
            return Err(NotReady::Untrusted(format!(
                "{} has mode {mode:o}, not {SOCKET_MODE:o}",
                socket.display()
            )));
        }
        Ok(())
    }

    /// The socket's file while the server listens on it. Dropping it removes the file, only while it is still the
    /// socket this server bound.
    #[derive(Debug)]
    pub struct SocketFile {
        path: PathBuf,
        device: u64,
        inode: u64,
    }

    impl SocketFile {
        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for SocketFile {
        fn drop(&mut self) {
            let ours = std::fs::symlink_metadata(&self.path).is_ok_and(|meta| {
                meta.file_type().is_socket()
                    && meta.dev() == self.device
                    && meta.ino() == self.inode
            });
            if ours {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    /// Binds the socket in a home that `prepare` checked. Call it only while holding the home's lock: it removes a
    /// stale socket of this account, refuses any other entry, and returns once the socket has mode 0600, without
    /// changing the process's umask. The listener queues connections from `bind` on; the caller accepts them only
    /// after this returns.
    pub fn bind_socket(home: &Path) -> Result<(tokio::net::UnixListener, SocketFile), HomeError> {
        let me = own_account();
        let path = home.join(SOCKET_FILE);
        let io = |error| HomeError::Io {
            path: path.clone(),
            error,
        };
        check_socket_path(&path)?;
        match std::fs::symlink_metadata(&path) {
            Ok(meta) => match foreign_entry(meta.file_type(), meta.uid(), me) {
                Some(what) => {
                    return Err(HomeError::SocketEntry {
                        path: path.clone(),
                        what,
                    });
                }
                None => match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(io(error)),
                },
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(error)),
        }
        let listener = tokio::net::UnixListener::bind(&path).map_err(io)?;
        let bound = std::fs::symlink_metadata(&path).map_err(io)?;
        if let Some(what) = foreign_entry(bound.file_type(), bound.uid(), me) {
            return Err(HomeError::SocketEntry {
                path: path.clone(),
                what,
            });
        }
        let file = SocketFile {
            path: path.clone(),
            device: bound.dev(),
            inode: bound.ino(),
        };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(SOCKET_MODE))
            .map_err(io)?;
        let checked = std::fs::symlink_metadata(&path).map_err(io)?;
        let mode = checked.permissions().mode() & 0o777;
        if checked.dev() != file.device || checked.ino() != file.inode || mode != SOCKET_MODE {
            return Err(HomeError::SocketEntry {
                path: path.clone(),
                what: format!("a socket with mode {mode:o} after it was set to {SOCKET_MODE:o}"),
            });
        }
        Ok((listener, file))
    }

    fn at(path: &Path) -> impl Fn(std::io::Error) -> HomeError + '_ {
        move |error| HomeError::Io {
            path: path.to_path_buf(),
            error,
        }
    }

    /// Extended ACLs, read where a safe call reads them.
    #[cfg(target_os = "linux")]
    mod acl {
        use std::path::Path;

        use rustix::io::Errno;

        use super::{HomeError, own_account};

        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum Access {
            Write,
            Any,
        }

        /// `struct posix_acl_xattr_header` and its entries: version 2, then (tag, perm, id) in little-endian order.
        const VERSION: u32 = 2;
        const TAG_USER: u16 = 0x02;
        const TAG_GROUP: u16 = 0x08;
        const WRITE: u16 = 0o2;
        const NAMES: [&str; 2] = ["system.posix_acl_access", "system.posix_acl_default"];

        /// The first named user or group entry, in the folder's access or default ACL, that grants `access`. The
        /// entries count as written, without the mask, so an ACL that the mask disables is refused too.
        pub fn granted(path: &Path, access: Access) -> Result<Option<String>, HomeError> {
            for name in NAMES {
                let mut value = [0u8; 4096];
                let length = match rustix::fs::lgetxattr(path, name, &mut value[..]) {
                    Ok(length) => length,
                    Err(Errno::NODATA | Errno::NOTSUP) => continue,
                    Err(Errno::RANGE) => return Ok(Some(format!("a {name} too large to read"))),
                    Err(error) => {
                        return Err(HomeError::Io {
                            path: path.to_path_buf(),
                            error: error.into(),
                        });
                    }
                };
                if let Some(entry) = named_grant(&value[..length], access, own_account()) {
                    return Ok(Some(entry));
                }
            }
            Ok(None)
        }

        pub(super) fn named_grant(value: &[u8], access: Access, me: u32) -> Option<String> {
            let Some((header, entries)) = value.split_first_chunk::<4>() else {
                return Some("an ACL that cannot be read".to_owned());
            };
            if u32::from_le_bytes(*header) != VERSION || entries.len() % 8 != 0 {
                return Some("an ACL that cannot be read".to_owned());
            }
            entries.as_chunks::<8>().0.iter().find_map(|entry| {
                let tag = u16::from_le_bytes([entry[0], entry[1]]);
                let perm = u16::from_le_bytes([entry[2], entry[3]]);
                let id = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]);
                let grants = match access {
                    Access::Write => perm & WRITE != 0,
                    Access::Any => perm != 0,
                };
                match tag {
                    TAG_USER if id != me && grants => Some(format!("user {id}")),
                    TAG_GROUP if grants => Some(format!("group {id}")),
                    _ => None,
                }
            })
        }
    }

    /// macOS keeps ACLs out of reach of `getxattr`; exacl reads them through the system's ACL library. A deny entry
    /// grants nothing, so macOS's own `everyone deny delete` on `~/Library` passes. An allow entry counts for any
    /// account, this one included, since its name is not compared with this account's.
    #[cfg(target_os = "macos")]
    mod acl {
        use std::path::Path;

        use exacl::Perm;

        use super::HomeError;

        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum Access {
            Write,
            Any,
        }

        pub fn granted(path: &Path, access: Access) -> Result<Option<String>, HomeError> {
            let entries =
                exacl::getfacl(path, None::<exacl::AclOption>).map_err(|error| HomeError::Io {
                    path: path.to_path_buf(),
                    error,
                })?;
            let write = Perm::WRITE
                | Perm::APPEND
                | Perm::DELETE
                | Perm::DELETE_CHILD
                | Perm::WRITEATTR
                | Perm::WRITEEXTATTR
                | Perm::WRITESECURITY
                | Perm::CHOWN;
            Ok(entries
                .into_iter()
                .filter(|entry| entry.allow)
                .find(|entry| match access {
                    Access::Write => entry.perms.intersects(write),
                    Access::Any => !entry.perms.is_empty(),
                })
                .map(|entry| format!("{:?} {}", entry.kind, entry.name)))
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    mod acl {
        use std::path::Path;

        use super::HomeError;

        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum Access {
            Write,
            Any,
        }

        pub fn granted(_path: &Path, _access: Access) -> Result<Option<String>, HomeError> {
            Ok(None)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Exposure, exposure};

        const ME: u32 = 1000;

        #[test]
        fn a_folder_above_the_home_may_be_writable_by_others_only_when_it_is_sticky() {
            assert_eq!(exposure(0, 0o1777, ME, false), None);
            assert_eq!(
                exposure(0, 0o777, ME, false),
                Some(Exposure::Writable(0o777))
            );
            assert_eq!(
                exposure(ME, 0o1775, ME, true),
                Some(Exposure::Writable(0o1775))
            );
            assert_eq!(
                exposure(1001, 0o755, ME, false),
                Some(Exposure::OtherOwner(1001))
            );
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn a_named_entry_that_grants_access_is_found_and_the_own_entries_are_not() {
            use super::acl::{Access, named_grant};

            let acl = |entries: &[(u16, u16, u32)]| {
                let mut value = 2u32.to_le_bytes().to_vec();
                for (tag, perm, id) in entries {
                    value.extend(tag.to_le_bytes());
                    value.extend(perm.to_le_bytes());
                    value.extend(id.to_le_bytes());
                }
                value
            };
            let own = [
                (0x01, 7, u32::MAX),
                (0x04, 0, u32::MAX),
                (0x20, 0, u32::MAX),
            ];
            let reader = [&own[..], &[(0x02, 4, 65534), (0x10, 4, u32::MAX)]].concat();
            let writer = [&own[..], &[(0x08, 6, 100), (0x10, 6, u32::MAX)]].concat();
            let me = [&own[..], &[(0x02, 7, ME)]].concat();

            assert_eq!(named_grant(&acl(&own), Access::Any, ME), None);
            assert_eq!(named_grant(&acl(&reader), Access::Write, ME), None);
            assert_eq!(
                named_grant(&acl(&reader), Access::Any, ME).as_deref(),
                Some("user 65534")
            );
            assert_eq!(
                named_grant(&acl(&writer), Access::Write, ME).as_deref(),
                Some("group 100")
            );
            assert_eq!(named_grant(&acl(&me), Access::Any, ME), None);
            assert!(named_grant(&[1, 0, 0, 0], Access::Any, ME).is_some());
        }
    }
}
