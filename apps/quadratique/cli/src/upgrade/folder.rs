//! The install folder: who can change its entries, the lock that lets one upgrade at a time
//! replace its binary, and the private folder where the new binary waits until it is checked.
//!
//! The upgrade defends against other accounts and the network, not against a process of the same
//! account, which already holds the account's files. So the paths it writes, runs and renames
//! must be paths that no other account can change: the install folder and every folder above it
//! belong to this account or to root; no other account and no group may write the install folder;
//! a folder above it may be writable by others or by a group only when it is sticky, as `/tmp`
//! is, so that they cannot rename or remove what they do not own.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use quadratique_server::home::{self, Exposure};

const STAGING_PREFIX: &str = ".quadratique-upgrade-";
/// The staging folder's name is the prefix and this many lowercase hex digits.
const STAGING_DIGITS: usize = 24;
const LOCK_NAME: &str = ".quadratique-upgrade.lock";

/// The home's rule for a folder whose entries no other account may change (`quadratique_server::home::exposure`),
/// applied to the install folder.
pub fn check(folder: &Path) -> Result<(), String> {
    let me = home::own_account();
    for (index, ancestor) in folder.ancestors().enumerate() {
        let meta = std::fs::metadata(ancestor)
            .map_err(|error| format!("{} cannot be read: {error}", ancestor.display()))?;
        match home::exposure(meta.uid(), meta.permissions().mode(), me, index == 0) {
            Some(Exposure::OtherOwner(uid)) => {
                return Err(format!(
                    "{} belongs to another account (uid {uid}), which could replace the binary",
                    ancestor.display(),
                ));
            }
            Some(Exposure::Writable(mode)) => {
                return Err(format!(
                    "other accounts can write {} (mode {mode:o}: group or others may write it), so they could replace the binary between its check and its install; remove their write access (chmod go-w {})",
                    ancestor.display(),
                    crate::init::shell_word(&ancestor.display().to_string())
                ));
            }
            None => {}
        }
    }
    Ok(())
}

/// One upgrade at a time per install folder. The lock file stays beside the binary. While a run
/// replaces the binary, it holds the device and inode of the file it replaces, so that a later run
/// can recognise a server left on that file when this one was stopped before its restart.
pub struct Lock {
    file: File,
}

impl Lock {
    pub fn take(folder: &Path) -> Result<Lock, String> {
        let path = folder.join(LOCK_NAME);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&path)
            .map_err(|error| format!("{} cannot be opened: {error}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Lock { file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(format!(
                "another quadratique upgrade is working on the binary in {}; run quadratique upgrade again when it ends",
                folder.display()
            )),
            Err(std::fs::TryLockError::Error(error)) => {
                Err(format!("{} cannot be locked: {error}", path.display()))
            }
        }
    }

    pub fn record_replaced(&mut self, file: (u64, u64)) -> Result<(), String> {
        let written = self
            .file
            .set_len(0)
            .and_then(|()| self.file.rewind())
            .and_then(|()| writeln!(self.file, "replaced {} {}", file.0, file.1))
            .and_then(|()| self.file.sync_all());
        written.map_err(|error| format!("the lock file cannot be written: {error}"))
    }

    /// The device and inode of the file that the last run replaced, when it recorded one.
    pub fn replaced(&mut self) -> Option<(u64, u64)> {
        self.file.rewind().ok()?;
        replaced_in(&self.file)
    }
}

/// The file that the last upgrade in `folder` replaced, as its lock file records it, read without taking the lock:
/// the device and inode of a binary that this install ran before.
#[cfg(target_os = "linux")]
pub fn recorded_replaced(folder: &Path) -> Option<(u64, u64)> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(folder.join(LOCK_NAME))
        .ok()?;
    replaced_in(&file)
}

fn replaced_in(file: &File) -> Option<(u64, u64)> {
    let mut text = String::new();
    file.take(256).read_to_string(&mut text).ok()?;
    let mut words = text.split_whitespace();
    (words.next()? == "replaced")
        .then(|| Some((words.next()?.parse().ok()?, words.next()?.parse().ok()?)))?
}

/// What `remove_leftovers` did with the staging-like folders beside the binary.
#[derive(Default)]
pub struct Leftovers {
    pub removed: Vec<PathBuf>,
    pub left: Vec<PathBuf>,
}

fn generated_name(name: &str) -> bool {
    name.strip_prefix(STAGING_PREFIX).is_some_and(|digits| {
        digits.len() == STAGING_DIGITS
            && digits
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Removes the staging folders that an earlier upgrade left when it was killed. Called with the
/// lock held, so no other upgrade uses one. Only a folder that this code could have made is
/// removed: a name of the generated form, belonging to this account, mode 0700, holding nothing
/// or one regular file named `quadratique`. That file is unlinked and the empty folder removed;
/// nothing is removed recursively. Any other entry with the prefix is left, and named.
pub fn remove_leftovers(folder: &Path, _lock: &Lock) -> Leftovers {
    let me = rustix::process::geteuid().as_raw();
    let mut leftovers = Leftovers::default();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return leftovers;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(STAGING_PREFIX) {
            continue;
        }
        let path = entry.path();
        if generated_name(&name) && remove_staging(&path, me) {
            leftovers.removed.push(path);
        } else {
            leftovers.left.push(path);
        }
    }
    leftovers.removed.sort();
    leftovers.left.sort();
    leftovers
}

fn remove_staging(path: &Path, me: u32) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_dir() || meta.uid() != me || meta.permissions().mode() & 0o7777 != 0o700 {
        return false;
    }
    let Ok(inside) = std::fs::read_dir(path) else {
        return false;
    };
    let inside: Vec<_> = inside.filter_map(Result::ok).collect();
    match inside.as_slice() {
        [] => {}
        [only] if only.file_name() == "quadratique" => {
            let binary = only.path();
            let regular = std::fs::symlink_metadata(&binary).is_ok_and(|meta| meta.is_file());
            if !regular || std::fs::remove_file(&binary).is_err() {
                return false;
            }
        }
        _ => return false,
    }
    std::fs::remove_dir(path).is_ok()
}

/// A folder only this account can enter, beside the binary so that the rename stays on one file
/// system, named at random. It is removed when dropped unless its binary was installed.
pub struct Staging {
    folder: PathBuf,
    pub binary: PathBuf,
    /// The written file, open read-only, so that `--version` runs the very file that was
    /// written and not a path resolved again.
    verified: File,
}

impl Staging {
    pub fn create(install: &Path, bytes: &[u8]) -> Result<Staging, String> {
        let mut random = [0u8; STAGING_DIGITS / 2];
        getrandom::fill(&mut random).map_err(|error| format!("no random name: {error}"))?;
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let folder = install.join(format!("{STAGING_PREFIX}{name}"));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&folder)
            .map_err(|error| format!("{} cannot be created: {error}", folder.display()))?;
        let binary = folder.join("quadratique");
        let cleanup = |problem: String| {
            let _ = std::fs::remove_file(&binary);
            let _ = std::fs::remove_dir(&folder);
            problem
        };
        let written = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&binary)
            .and_then(|mut file| {
                file.write_all(bytes)?;
                file.sync_all()?;
                file.metadata()
            })
            .map_err(|error| cleanup(format!("writing {} failed: {error}", binary.display())))?;
        let verified = File::open(&binary)
            .map_err(|error| cleanup(format!("{} cannot be read: {error}", binary.display())))?;
        let opened = verified
            .metadata()
            .map_err(|error| cleanup(format!("{} cannot be read: {error}", binary.display())))?;
        if (opened.dev(), opened.ino()) != (written.dev(), written.ino()) {
            return Err(cleanup(format!(
                "{} changed after it was written",
                binary.display()
            )));
        }
        Ok(Staging {
            folder,
            binary,
            verified,
        })
    }

    /// The path that runs the written file: on Linux, the open descriptor's `/proc/self/fd/<n>`,
    /// kept open across `exec` so that a script's interpreter can read it too.
    pub fn runnable(&self) -> Result<PathBuf, String> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsFd;
            use std::os::fd::AsRawFd;

            rustix::io::fcntl_setfd(self.verified.as_fd(), rustix::io::FdFlags::empty())
                .map_err(|error| format!("the new binary cannot be run: {error}"))?;
            Ok(PathBuf::from(format!(
                "/proc/self/fd/{}",
                self.verified.as_raw_fd()
            )))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = &self.verified;
            Ok(self.binary.clone())
        }
    }

    /// Renames the binary over `destination`, which keeps the mode 0755 of an installed binary.
    pub fn install(self, destination: &Path) -> Result<(), String> {
        std::fs::set_permissions(&self.binary, std::fs::Permissions::from_mode(0o755))
            .and_then(|()| std::fs::rename(&self.binary, destination))
            .map_err(|error| {
                format!(
                    "renaming {} over {} failed: {error}",
                    self.binary.display(),
                    destination.display()
                )
            })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.binary);
        let _ = std::fs::remove_dir(&self.folder);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[cfg(target_os = "linux")]
    use super::{LOCK_NAME, recorded_replaced};
    use super::{Lock, Staging, check, remove_leftovers};

    fn folder() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn mode(path: &std::path::Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_folder_that_a_group_or_other_accounts_can_write_is_refused_and_a_sticky_one_above_it_is_not()
     {
        let root = folder();
        let install = root.path().join("bin");
        std::fs::create_dir(&install).unwrap();
        mode(root.path(), 0o1777);
        mode(&install, 0o755);
        check(&install).unwrap();

        for refused in [0o757, 0o775] {
            mode(&install, refused);
            assert!(
                check(&install).unwrap_err().contains("chmod go-w"),
                "{refused:o}"
            );
        }
        mode(&install, 0o755);
        mode(root.path(), 0o775);
        assert!(
            check(&install)
                .unwrap_err()
                .contains("other accounts can write")
        );
    }

    #[test]
    fn only_a_staging_folder_this_code_could_make_is_removed_and_never_recursively() {
        let install = folder();
        let lock = Lock::take(install.path()).unwrap();
        let make = |name: &str| {
            let path = install.path().join(name);
            std::fs::create_dir(&path).unwrap();
            mode(&path, 0o700);
            path
        };
        let leftover = make(".quadratique-upgrade-0123456789abcdef01234567");
        std::fs::write(leftover.join("quadratique"), "half written").unwrap();
        let empty = make(".quadratique-upgrade-89abcdef0123456789abcdef");
        let not_generated = make(".quadratique-upgrade-decoy");
        std::fs::write(not_generated.join("quadratique"), "a person's file").unwrap();
        let nested = make(".quadratique-upgrade-abcdefabcdefabcdefabcdef");
        std::fs::create_dir(nested.join("quadratique")).unwrap();
        std::fs::write(nested.join("quadratique/notes"), "a person's file").unwrap();

        let leftovers = remove_leftovers(install.path(), &lock);

        let mut removed = vec![leftover, empty];
        removed.sort();
        assert_eq!(leftovers.removed, removed);
        let mut left = vec![not_generated.clone(), nested.clone()];
        left.sort();
        assert_eq!(leftovers.left, left);
        assert!(not_generated.join("quadratique").exists());
        assert!(nested.join("quadratique/notes").exists());
    }

    #[test]
    fn staging_is_private_and_a_dropped_staging_folder_is_removed() {
        let install = folder();
        let staging = Staging::create(install.path(), b"binary").unwrap();
        let folder = staging.binary.parent().unwrap().to_owned();
        assert_eq!(
            std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
            0o700
        );
        drop(staging);
        assert!(!folder.exists());
    }

    #[test]
    fn a_second_lock_on_the_same_folder_is_refused_and_the_lock_keeps_the_replaced_file() {
        let install = folder();
        let mut held = Lock::take(install.path()).unwrap();
        assert!(
            Lock::take(install.path())
                .err()
                .unwrap()
                .contains("another quadratique upgrade")
        );
        assert_eq!(held.replaced(), None);
        held.record_replaced((7, 42)).unwrap();
        drop(held);
        assert_eq!(
            Lock::take(install.path()).unwrap().replaced(),
            Some((7, 42))
        );
    }

    #[cfg(target_os = "linux")]
    /// A command that is not an upgrade reads the replaced file without the lock, so that it never waits for an
    /// upgrade and never creates the lock file.
    #[test]
    fn the_replaced_file_is_read_without_the_lock_and_a_folder_without_one_records_none() {
        let install = folder();
        assert_eq!(recorded_replaced(install.path()), None);
        assert!(!install.path().join(LOCK_NAME).exists());

        let mut held = Lock::take(install.path()).unwrap();
        held.record_replaced((7, 42)).unwrap();

        assert_eq!(recorded_replaced(install.path()), Some((7, 42)));
    }
}
