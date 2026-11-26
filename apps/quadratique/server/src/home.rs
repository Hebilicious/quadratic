//! The data folder, which this code calls the home: the one policy that the server and the command line apply to it
//! before either opens a lock file, touches the store or starts a process, and the socket that the server binds in it.
//! docs/decisions/0004-a-socket-for-commands-and-a-token-for-the-browser.md, §1.
//!
//! The policy defends against other accounts of the machine, not against processes of this account, which already
//! hold its files. A folder that another account could rename, replace or enter would let that account put its own
//! socket, lock or store where this account's processes look for them.

use std::path::{Path, PathBuf};

use crate::credentials::CREDENTIALS_FILE;
use crate::server::{DATABASE_FILE, LOCK_FILE, Paths, RUNTIME_FILE};
use crate::store::{StoreError, refuse_git_work_tree};

pub const SOCKET_FILE: &str = "server.sock";
const START_LOCK_FILE: &str = "start.lock";
const LOG_FILE: &str = "server.log";
const KEPT_PORT_FILE: &str = "server.port";
const STRICT_FILE: &str = "strict";
const SERVED_PORTS_FILE: &str = "served-ports";

/// The longest socket path, in bytes, that `bind` and `connect` accept: the size of `sun_path` less its final NUL.
pub const SOCKET_PATH_LIMIT: usize = if cfg!(target_os = "macos") { 103 } else { 107 };

/// A home that [`prepare`] checked, named by its path with every link resolved. Only `prepare` makes one, and every
/// file of the home is named through it, so no later step can use a path that the policy did not examine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    folder: PathBuf,
}

impl Prepared {
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn database(&self) -> PathBuf {
        self.folder.join(DATABASE_FILE)
    }

    pub fn runtime(&self) -> PathBuf {
        self.folder.join(RUNTIME_FILE)
    }

    pub fn lock(&self) -> PathBuf {
        self.folder.join(LOCK_FILE)
    }

    pub fn start_lock(&self) -> PathBuf {
        self.folder.join(START_LOCK_FILE)
    }

    pub fn log(&self) -> PathBuf {
        self.folder.join(LOG_FILE)
    }

    pub fn socket(&self) -> PathBuf {
        self.folder.join(SOCKET_FILE)
    }

    pub fn credentials(&self) -> PathBuf {
        self.folder.join(CREDENTIALS_FILE)
    }

    /// The port the web app keeps across restarts. Unlike the runtime file, it stays after the server stops.
    pub fn kept_port(&self) -> PathBuf {
        self.folder.join(KEPT_PORT_FILE)
    }

    /// Present while the owner asked for the strict setting (`quadratique server strict on`).
    pub fn strict(&self) -> PathBuf {
        self.folder.join(STRICT_FILE)
    }

    /// The ports a strict home served, which a strict start does not take again.
    pub fn served_ports(&self) -> PathBuf {
        self.folder.join(SERVED_PORTS_FILE)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HomeError {
    #[error("{0}")]
    Store(#[from] StoreError),
    #[error("{} is a link that another account could change: {reason}", .link.display())]
    ForeignLink {
        link: PathBuf,
        reason: String,
        other_owner: Option<OtherOwner>,
    },
    #[error("another account can replace {}: {reason}", .path.display())]
    Replaceable {
        path: PathBuf,
        reason: String,
        other_owner: Option<OtherOwner>,
    },
    #[error("{} gives access to another account: {reason}", .path.display())]
    SharedAccess { path: PathBuf, reason: String },
    #[error("{} is not private: {reason}", .path.display())]
    NotPrivate {
        path: PathBuf,
        reason: String,
        other_owner: Option<OtherOwner>,
    },
    #[error("{} is not a folder", .0.display())]
    NotAFolder(PathBuf),
    #[error(
        "{} is missing, and the path goes back up from it with `..`: name the folder without `..`",
        .0.display()
    )]
    MissingThenParent(PathBuf),
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

    /// The entry whose owner made this refusal, when the kernel showed that owner as its overflow uid because this
    /// process's user namespace does not map the real one ([`owner_hidden`]). The refusal stands: this process cannot
    /// tell whether that owner is root or another account.
    pub fn hidden_owner(&self) -> Option<&Path> {
        let (HomeError::ForeignLink { other_owner, .. }
        | HomeError::Replaceable { other_owner, .. }
        | HomeError::NotPrivate { other_owner, .. }) = self
        else {
            return None;
        };
        let other = other_owner.as_ref()?;
        owner_hidden(other.uid).then_some(other.path.as_path())
    }
}

/// The entry that a refusal found owned by another account than this one or root, and that owner as `lstat` gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherOwner {
    pub path: PathBuf,
    pub uid: u32,
}

const OVERFLOW_UID: &str = "/proc/sys/kernel/overflowuid";
const UID_MAP: &str = "/proc/self/uid_map";

/// Whether `owner`, as `lstat` gave it, is not an account at all but the kernel's overflow uid, which stands for any
/// owner that this process's user namespace does not map (`user_namespaces(7)`). Read from the kernel, never guessed:
/// see [`stands_for_an_unmapped_owner`]. Outside Linux there is no such namespace, and it is never true.
pub fn owner_hidden(owner: u32) -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let (Ok(overflow), Ok(uid_map)) = (
        std::fs::read_to_string(OVERFLOW_UID),
        std::fs::read_to_string(UID_MAP),
    ) else {
        return false;
    };
    overflow
        .trim()
        .parse()
        .is_ok_and(|overflow| stands_for_an_unmapped_owner(owner, overflow, &uid_map))
}

/// `owner` is the overflow uid, and the overflow uid is no uid of this namespace: no line of its `uid_map` (inside,
/// outside, count) covers it. The kernel shows a file as owned by a uid of the namespace only when the map gives that
/// uid, so such an owner can only be the overflow uid standing in for an unmapped one. Outside a user namespace the map
/// is the identity map `0 0 4294967295`, which covers every uid: an entry that the account nobody (uid 65534) really
/// owns stays that account's. A map that does not read as such lines hides nothing.
fn stands_for_an_unmapped_owner(owner: u32, overflow: u32, uid_map: &str) -> bool {
    if owner != overflow {
        return false;
    }
    let mut covered = false;
    for line in uid_map.lines() {
        let fields: Option<Vec<u64>> = line
            .split_whitespace()
            .map(|field| field.parse().ok())
            .collect();
        let Some([inside, _outside, count]) = fields.as_deref() else {
            return false;
        };
        covered |= (*inside..inside.saturating_add(*count)).contains(&u64::from(overflow));
    }
    !covered
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

/// A new file that only this account can read, made with `create_new`, so that nothing already at the path (a link
/// included) is ever written through.
pub fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// The one way a file of the home is replaced: `contents` go to a partial file `<name>.<pid>.<random>`, unique to this
/// call and made by [`create_private`], so no two writes, in one process or two, share one and nothing already at that
/// name (a link included) is used; it is synced, then renamed over `path`. Only this call's own partial file is removed
/// when a step fails.
pub fn write_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut unique = [0u8; 8];
    getrandom::fill(&mut unique).map_err(std::io::Error::other)?;
    let unique: String = unique.iter().map(|byte| format!("{byte:02x}")).collect();
    let partial = path.with_file_name(format!("{name}.{}.{unique}", std::process::id()));
    let named = |what: &str, error: std::io::Error| {
        std::io::Error::new(
            error.kind(),
            format!("{what} {}: {error}", partial.display()),
        )
    };
    let mut made = false;
    let written = create_private(&partial)
        .and_then(|mut file| {
            made = true;
            file.write_all(contents)?;
            file.sync_all()
        })
        .map_err(|error| named("writing", error))
        .and_then(|()| std::fs::rename(&partial, path).map_err(|error| named("renaming", error)));
    if written.is_err() && made {
        let _ = std::fs::remove_file(&partial);
    }
    written
}

/// Applies the policy to the home that `paths` names, makes it when it is missing, and returns the only handle on its
/// files.
pub fn prepare(paths: &Paths) -> Result<Prepared, HomeError> {
    let given = paths.database.parent().unwrap_or(Path::new("."));
    refuse_git_work_tree(given)?;
    Ok(Prepared {
        folder: prepare_folder(given)?,
    })
}

#[cfg(unix)]
pub use unix::*;

/// Windows has no modes to check, and the server refuses to start there (QUA-61): the home is only made.
#[cfg(not(unix))]
fn prepare_folder(home: &Path) -> Result<PathBuf, HomeError> {
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
    use std::time::Duration;

    use super::{HomeError, OtherOwner, Prepared, SOCKET_PATH_LIMIT, shell_word};

    const GROUP_OR_OTHER_WRITE: u32 = 0o022;
    const GROUP_OR_OTHER_ANY: u32 = 0o077;
    const STICKY: u32 = 0o1000;
    const SOCKET_MODE: u32 = 0o600;
    /// As many links as Linux follows in one path before `ELOOP`.
    const LINKS: usize = 40;
    /// How many times the walk starts again when another process makes a folder that this one was about to make.
    const RACES: usize = 8;
    const ROOT: u32 = 0;
    /// How long the server waits for a socket left at its name to accept or refuse a connection.
    const STALE_PROBE: Duration = Duration::from_secs(1);
    /// A macOS ACL entry can let an account remove what a folder holds whatever its owner and its sticky bit say
    /// (`DELETE_CHILD` is checked before the sticky rule), so every folder above the home has its ACL read there. On
    /// Linux an ACL adds only to what the owner granted: a root or sticky folder that passes the mode rule is safe.
    const ACL_ON_EVERY_ANCESTOR: bool = cfg!(target_os = "macos");
    const ELSEWHERE: &str = "or set QUADRATIQUE_HOME to a folder that only you and root can change, such as one in your home folder";

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

    /// Checks the home, makes what is missing of it, and gives the path that all later work uses: the home with every
    /// link resolved.
    ///
    /// 1. Every component is examined with `lstat`. A link is followed only when it and the folder that holds it
    ///    belong to this account or to root, and that folder passes rules 2 and 3.
    /// 2. Every folder above the home passes [`exposure`].
    /// 3. No extended ACL entry lets another user or group use the home, or write a folder above it: every folder on
    ///    macOS, the non-sticky folders of this account on Linux.
    /// 4. A missing folder is made with mode 0700 in a folder that passed rules 2 and 3, and checked before the walk
    ///    enters it. When another process made it first, the walk starts again and checks it as any other. A home
    ///    whose mode gives a group or other accounts any access is refused, never changed.
    /// 5. The socket's path fits in `sun_path`, and the socket's name holds nothing, or a socket of this account.
    pub(super) fn prepare_folder(home: &Path) -> Result<PathBuf, HomeError> {
        let me = own_account();
        let home = std::path::absolute(home).map_err(at(home))?;
        let mut races = 0;
        let leaf = loop {
            match walk(&home, me)? {
                Walked::Leaf(leaf) => break leaf,
                Walked::Raced(path) => {
                    races += 1;
                    if races > RACES {
                        return Err(HomeError::Io {
                            path,
                            error: std::io::Error::other(
                                "another process keeps making this folder while Quadratique makes it",
                            ),
                        });
                    }
                }
            }
        };
        if let Some(parent) = leaf.parent() {
            check_ancestors(parent, me)?;
        }
        check_leaf(&leaf, me)?;
        let socket = leaf.join(super::SOCKET_FILE);
        check_socket_path(&socket)?;
        check_socket_entry(&socket, me)?;
        Ok(leaf)
    }

    enum Walked {
        Leaf(PathBuf),
        /// Another process made this folder between its `lstat` and its `mkdir`.
        Raced(PathBuf),
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
    /// allows and making each missing folder before it enters it. `resolved` never holds a link, so its parent is the
    /// parent that the kernel finds for `..`; later work uses only the resolved path.
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
                    if pending.iter().any(|step| matches!(step, Step::Parent)) {
                        return Err(HomeError::MissingThenParent(candidate));
                    }
                    let wanted = pending
                        .iter()
                        .fold(candidate.clone(), |path, step| match step {
                            Step::Name(name) => path.join(name),
                            Step::Parent => path,
                        });
                    check_socket_path(&wanted.join(super::SOCKET_FILE))?;
                    check_ancestors(&resolved, me)?;
                    match make_one(&candidate, me)? {
                        Made::Made => {
                            resolved = candidate;
                            continue;
                        }
                        Made::Exists => return Ok(Walked::Raced(candidate)),
                    }
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
        Ok(Walked::Leaf(resolved))
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum Made {
        Made,
        /// Something stands at the name already: the caller examines it again, as any entry it did not make.
        Exists,
    }

    /// Makes one folder, private from the start, and checks that what stands at the name is that folder.
    pub fn make_one(folder: &Path, me: u32) -> Result<Made, HomeError> {
        match std::fs::DirBuilder::new().mode(0o700).create(folder) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Ok(Made::Exists);
            }
            Err(error) => return Err(at(folder)(error)),
        }
        let meta = std::fs::symlink_metadata(folder).map_err(at(folder))?;
        if !meta.is_dir() || meta.uid() != me || meta.mode() & GROUP_OR_OTHER_ANY != 0 {
            return Err(HomeError::NotPrivate {
                path: folder.to_path_buf(),
                other_owner: None,
                reason: format!(
                    "the folder that Quadratique made there is not a private folder of this account (mode {:o}, uid {})",
                    meta.mode() & 0o7777,
                    meta.uid()
                ),
            });
        }
        Ok(Made::Made)
    }

    fn check_link(link: &Path, meta: &Metadata, holder: &Path, me: u32) -> Result<(), HomeError> {
        let foreign = |reason: String, other_owner: Option<OtherOwner>| HomeError::ForeignLink {
            link: link.to_path_buf(),
            reason: format!("{reason}; name the folder without this link, {ELSEWHERE}"),
            other_owner,
        };
        if meta.uid() != me && meta.uid() != ROOT {
            return Err(foreign(
                format!("it belongs to another account (uid {})", meta.uid()),
                Some(OtherOwner {
                    path: link.to_path_buf(),
                    uid: meta.uid(),
                }),
            ));
        }
        check_ancestors(holder, me).map_err(|refusal| {
            let other_owner = match &refusal {
                HomeError::Replaceable { other_owner, .. } => other_owner.clone(),
                _ => None,
            };
            foreign(refusal.to_string(), other_owner)
        })
    }

    /// `folder` and every folder above it, as folders above the home.
    fn check_ancestors(folder: &Path, me: u32) -> Result<(), HomeError> {
        for ancestor in folder.ancestors() {
            let meta = std::fs::symlink_metadata(ancestor).map_err(at(ancestor))?;
            let shown = shell_word(&ancestor.display().to_string());
            let replaceable =
                |reason: String, other_owner: Option<OtherOwner>| HomeError::Replaceable {
                    path: ancestor.to_path_buf(),
                    reason,
                    other_owner,
                };
            match exposure(meta.uid(), meta.mode(), me, false) {
                Some(Exposure::OtherOwner(uid)) => {
                    return Err(replaceable(
                        format!(
                            "it belongs to another account (uid {uid}); ask its owner to give it to you or to root, {ELSEWHERE}"
                        ),
                        Some(OtherOwner {
                            path: ancestor.to_path_buf(),
                            uid,
                        }),
                    ));
                }
                Some(Exposure::Writable(mode)) => {
                    return Err(replaceable(
                        format!(
                            "its mode is {mode:o}, which lets a group or other accounts rename what it holds, and it is not sticky; remove their write access (chmod go-w {shown}), {ELSEWHERE}"
                        ),
                        None,
                    ));
                }
                None => {}
            }
            let read_acl = ACL_ON_EVERY_ANCESTOR || (meta.uid() == me && meta.mode() & STICKY == 0);
            if read_acl && let Some(grant) = acl::granted(ancestor, acl::Access::Write)? {
                return Err(HomeError::SharedAccess {
                    path: ancestor.to_path_buf(),
                    reason: format!(
                        "its ACL lets {} write it or remove what it holds; remove that entry ({}), {ELSEWHERE}",
                        grant.who, grant.remedy
                    ),
                });
            }
        }
        Ok(())
    }

    fn check_leaf(leaf: &Path, me: u32) -> Result<(), HomeError> {
        let meta = std::fs::symlink_metadata(leaf).map_err(at(leaf))?;
        let not_private = |reason: String, other_owner: Option<OtherOwner>| HomeError::NotPrivate {
            path: leaf.to_path_buf(),
            reason,
            other_owner,
        };
        if meta.uid() != me {
            return Err(not_private(
                format!(
                    "it belongs to another account (uid {}); set QUADRATIQUE_HOME to a folder of your own",
                    meta.uid()
                ),
                Some(OtherOwner {
                    path: leaf.to_path_buf(),
                    uid: meta.uid(),
                }),
            ));
        }
        if let Some(grant) = acl::granted(leaf, acl::Access::Any)? {
            return Err(HomeError::SharedAccess {
                path: leaf.to_path_buf(),
                reason: format!(
                    "its ACL lets {} use it; remove that entry ({})",
                    grant.who, grant.remedy
                ),
            });
        }
        let mode = meta.permissions().mode() & 0o777;
        if mode & GROUP_OR_OTHER_ANY != 0 {
            return Err(not_private(
                format!(
                    "its mode is {mode:o}, which lets a group or other accounts use it; run chmod 700 {}",
                    shell_word(&leaf.display().to_string())
                ),
                None,
            ));
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
    /// home's lock replaces once a connection to it is refused. Anything else is never removed.
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

    /// What a client finds at the socket's name before it connects.
    #[derive(Debug)]
    pub enum Entry {
        /// A socket of this account with mode 0600: the server sets that mode before it accepts a connection.
        Ready,
        /// Nothing: `lstat` gives `ENOENT` in a home that the policy checked.
        Missing,
        /// `EACCES` or `EPERM`: a sandbox, or another account's folder.
        Denied(std::io::Error),
        /// Another kind of entry, an entry of another account, or a socket whose mode is not yet 0600.
        Untrusted(String),
        /// Any other failure of `lstat`: `ENOTDIR`, `ELOOP`, `ENAMETOOLONG`, …
        PathError(std::io::Error),
    }

    pub fn socket_entry(socket: &Path) -> Entry {
        let meta = match std::fs::symlink_metadata(socket) {
            Ok(meta) => meta,
            Err(error) => {
                return match error.kind() {
                    std::io::ErrorKind::NotFound => Entry::Missing,
                    std::io::ErrorKind::PermissionDenied => Entry::Denied(error),
                    _ => Entry::PathError(error),
                };
            }
        };
        if let Some(what) = foreign_entry(meta.file_type(), meta.uid(), own_account()) {
            return Entry::Untrusted(format!("{} is {what}", socket.display()));
        }
        let mode = meta.permissions().mode() & 0o777;
        if mode != SOCKET_MODE {
            return Entry::Untrusted(format!(
                "{} has mode {mode:o}, not {SOCKET_MODE:o}",
                socket.display()
            ));
        }
        Entry::Ready
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

    /// Binds the socket of a prepared home. Call it only while holding the home's lock. A socket of this account at
    /// the name is removed only once a connection to it is refused: a process that listens there without the lock
    /// keeps it, and the server refuses to start. Any other entry is refused. Returns once the socket has mode 0600,
    /// without changing the process's umask. The listener queues connections from `bind` on; the caller accepts them
    /// only when it is ready.
    pub async fn bind_socket(
        home: &Prepared,
    ) -> Result<(tokio::net::UnixListener, SocketFile), HomeError> {
        let me = own_account();
        let path = home.socket();
        let io = |error| HomeError::Io {
            path: path.clone(),
            error,
        };
        let entry = |what: String| HomeError::SocketEntry {
            path: path.clone(),
            what,
        };
        check_socket_path(&path)?;
        match std::fs::symlink_metadata(&path) {
            Ok(meta) => {
                if let Some(what) = foreign_entry(meta.file_type(), meta.uid(), me) {
                    return Err(entry(what));
                }
                let probe =
                    tokio::time::timeout(STALE_PROBE, tokio::net::UnixStream::connect(&path)).await;
                match probe {
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                        match std::fs::remove_file(&path) {
                            Ok(()) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => return Err(io(error)),
                        }
                    }
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Ok(Ok(_)) => {
                        return Err(entry(
                            "a socket on which a process of this account listens without holding the home's lock"
                                .to_owned(),
                        ));
                    }
                    Ok(Err(error)) => {
                        return Err(entry(format!(
                            "a socket that may still be in use: connecting to it failed with {error}"
                        )));
                    }
                    Err(_) => {
                        return Err(entry(format!(
                            "a socket that may still be in use: it neither accepted nor refused a connection within {STALE_PROBE:?}"
                        )));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(error)),
        }
        let listener = tokio::net::UnixListener::bind(&path).map_err(io)?;
        let bound = std::fs::symlink_metadata(&path).map_err(io)?;
        if let Some(what) = foreign_entry(bound.file_type(), bound.uid(), me) {
            return Err(entry(what));
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
            return Err(entry(format!(
                "a socket with mode {mode:o} after it was set to {SOCKET_MODE:o}"
            )));
        }
        Ok((listener, file))
    }

    fn at(path: &Path) -> impl Fn(std::io::Error) -> HomeError + '_ {
        move |error| HomeError::Io {
            path: path.to_path_buf(),
            error,
        }
    }

    /// An ACL entry that gives an account other than this one access, and the command that removes it.
    pub struct Grant {
        pub who: String,
        pub remedy: String,
    }

    /// Extended ACLs, read where a safe call reads them.
    #[cfg(target_os = "linux")]
    mod acl {
        use std::path::Path;

        use rustix::io::Errno;

        use super::{Grant, HomeError, own_account, shell_word};

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
        /// Each ACL with the `setfacl` option that removes it: `-b` the extended entries, `-k` the default ACL.
        const NAMES: [(&str, &str); 2] = [
            ("system.posix_acl_access", "-b"),
            ("system.posix_acl_default", "-k"),
        ];

        /// The first named user or group entry, in the folder's access or default ACL, that grants `access`. The
        /// entries count as written, without the mask, so an ACL that the mask disables is refused too.
        pub fn granted(path: &Path, access: Access) -> Result<Option<Grant>, HomeError> {
            for (name, option) in NAMES {
                let mut value = [0u8; 4096];
                let remedy = || {
                    format!(
                        "setfacl {option} {}",
                        shell_word(&path.display().to_string())
                    )
                };
                let length = match rustix::fs::lgetxattr(path, name, &mut value[..]) {
                    Ok(length) => length,
                    Err(Errno::NODATA | Errno::NOTSUP) => continue,
                    Err(Errno::RANGE) => {
                        return Ok(Some(Grant {
                            who: format!("the entries of a {name} too large to read"),
                            remedy: remedy(),
                        }));
                    }
                    Err(error) => {
                        return Err(HomeError::Io {
                            path: path.to_path_buf(),
                            error: error.into(),
                        });
                    }
                };
                if let Some(who) = named_grant(&value[..length], access, own_account()) {
                    let who = if option == "-k" {
                        format!("{who}, in its default ACL,")
                    } else {
                        who
                    };
                    return Ok(Some(Grant {
                        who,
                        remedy: remedy(),
                    }));
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

        use super::{Grant, HomeError, shell_word};

        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum Access {
            Write,
            Any,
        }

        pub fn granted(path: &Path, access: Access) -> Result<Option<Grant>, HomeError> {
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
                .map(|entry| Grant {
                    who: format!("{:?} {}", entry.kind, entry.name),
                    remedy: format!("chmod -N {}", shell_word(&path.display().to_string())),
                }))
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    mod acl {
        use std::path::Path;

        use super::{Grant, HomeError};

        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum Access {
            Write,
            Any,
        }

        pub fn granted(_path: &Path, _access: Access) -> Result<Option<Grant>, HomeError> {
            Ok(None)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Exposure, Made, exposure, make_one, own_account};

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

        /// Another process may make the name between the walk's `lstat` and its `mkdir`, as a link to anywhere: the
        /// walk must examine that entry again, never enter it as the folder it meant to make.
        #[test]
        fn a_folder_that_another_process_made_first_is_not_taken_for_the_one_this_walk_made() {
            let folder = tempfile::tempdir().expect("a temporary folder");
            let name = folder.path().join("home");
            std::os::unix::fs::symlink("/", &name).expect("a link made first");

            assert_eq!(make_one(&name, own_account()).ok(), Some(Made::Exists));

            let fresh = folder.path().join("fresh");
            assert_eq!(make_one(&fresh, own_account()).ok(), Some(Made::Made));
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

#[cfg(test)]
mod hidden_owner_tests {
    use super::stands_for_an_unmapped_owner;

    const OVERFLOW: u32 = 65534;
    const IDENTITY: &str = "         0          0 4294967295\n";
    const ONLY_THIS_ACCOUNT: &str = "      1000       1000          1\n";

    #[test]
    fn outside_a_user_namespace_the_account_nobody_is_an_owner_like_any_other() {
        assert!(!stands_for_an_unmapped_owner(OVERFLOW, OVERFLOW, IDENTITY));
    }

    #[test]
    fn in_a_namespace_that_maps_only_this_account_the_overflow_uid_hides_the_real_owner() {
        assert!(stands_for_an_unmapped_owner(
            OVERFLOW,
            OVERFLOW,
            ONLY_THIS_ACCOUNT
        ));
        assert!(!stands_for_an_unmapped_owner(
            1001,
            OVERFLOW,
            ONLY_THIS_ACCOUNT
        ));
    }

    #[test]
    fn a_namespace_that_maps_the_overflow_uid_or_a_map_that_does_not_read_hides_nothing() {
        assert!(!stands_for_an_unmapped_owner(
            OVERFLOW,
            OVERFLOW,
            "0 100000 65536\n"
        ));
        assert!(!stands_for_an_unmapped_owner(
            OVERFLOW,
            OVERFLOW,
            "1000 1000\n"
        ));
    }
}
