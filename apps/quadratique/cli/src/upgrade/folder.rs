//! The install folder: who can change its entries, the lock that lets one upgrade at a time
//! replace its binary, and the private folder where the new binary waits until it is checked.
//!
//! The new binary is written, run and renamed by path, so every path it takes must be one that
//! only this account can change. The install folder and every folder above it must belong to this
//! account or to root. The install folder must not be writable by other accounts. A folder above
//! it may be writable by others only when it is sticky, as `/tmp` is: then they cannot rename or
//! remove what they do not own. A folder writable by its group counts as writable by others,
//! unless the group is this account's private group (its name is the account's name in
//! `/etc/group` and it lists no other member).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const STAGING_PREFIX: &str = ".quadratique-upgrade-";
const LOCK_NAME: &str = ".quadratique-upgrade.lock";
const GROUP_WRITE: u32 = 0o020;
const OTHER_WRITE: u32 = 0o002;
const STICKY: u32 = 0o1000;

pub fn check(folder: &Path) -> Result<(), String> {
    let me = rustix::process::geteuid().as_raw();
    let private_group = private_group(me);
    for (index, ancestor) in folder.ancestors().enumerate() {
        let meta = std::fs::metadata(ancestor)
            .map_err(|error| format!("{} cannot be read: {error}", ancestor.display()))?;
        if meta.uid() != me && meta.uid() != 0 {
            return Err(format!(
                "{} belongs to another account (uid {}), which could replace the binary",
                ancestor.display(),
                meta.uid()
            ));
        }
        let mode = meta.permissions().mode();
        let group_writes = mode & GROUP_WRITE != 0 && Some(meta.gid()) != private_group;
        let others_write = mode & OTHER_WRITE != 0 || group_writes;
        let sticky = mode & STICKY != 0;
        if others_write && (index == 0 || !sticky) {
            return Err(format!(
                "other accounts can write {} (mode {:o}), so they could replace the binary between its check and its install; remove their write access (chmod go-w)",
                ancestor.display(),
                mode & 0o7777
            ));
        }
    }
    Ok(())
}

/// The gid of this account's private group, when `/etc/passwd` and `/etc/group` show one.
fn private_group(uid: u32) -> Option<u32> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    let (name, gid) = passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.get(2)?.parse::<u32>().ok()? == uid)
            .then(|| Some((fields[0].to_owned(), fields.get(3)?.parse::<u32>().ok()?)))?
    })?;
    let group = std::fs::read_to_string("/etc/group").ok()?;
    group.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        let members = fields.get(3).copied().unwrap_or_default();
        (fields.first() == Some(&name.as_str())
            && fields.get(2)?.parse::<u32>().ok()? == gid
            && (members.is_empty() || members == name))
            .then_some(gid)
    })
}

/// One upgrade at a time per install folder. The lock file stays, empty, beside the binary.
pub struct Lock {
    _file: File,
}

impl Lock {
    pub fn take(folder: &Path) -> Result<Lock, String> {
        let path = folder.join(LOCK_NAME);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&path)
            .map_err(|error| format!("{} cannot be opened: {error}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Lock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(format!(
                "another quadratique upgrade is replacing the binary in {}; run quadratique upgrade again when it ends",
                folder.display()
            )),
            Err(std::fs::TryLockError::Error(error)) => {
                Err(format!("{} cannot be locked: {error}", path.display()))
            }
        }
    }
}

/// Removes the staging folders that an earlier upgrade left, when it was killed. Called with the
/// lock held, so no other upgrade is using one. A folder is taken for one of ours only when its
/// name has our prefix, it belongs to this account, its mode is 0700, and it holds nothing but a
/// file named `quadratique`.
pub fn remove_leftovers(folder: &Path, _lock: &Lock) -> Vec<PathBuf> {
    let me = rustix::process::geteuid().as_raw();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(STAGING_PREFIX) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let ours = meta.is_dir()
            && meta.uid() == me
            && meta.permissions().mode() & 0o7777 == 0o700
            && std::fs::read_dir(&path).is_ok_and(|inside| {
                inside
                    .filter_map(Result::ok)
                    .all(|entry| entry.file_name() == "quadratique")
            });
        if ours && std::fs::remove_dir_all(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

/// A folder only this account can enter, beside the binary so that the rename stays on one file
/// system, named at random. It is removed when dropped unless its binary was installed.
pub struct Staging {
    folder: PathBuf,
    pub binary: PathBuf,
}

impl Staging {
    pub fn create(install: &Path, bytes: &[u8]) -> Result<Staging, String> {
        let mut random = [0u8; 12];
        getrandom::fill(&mut random).map_err(|error| format!("no random name: {error}"))?;
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let folder = install.join(format!("{STAGING_PREFIX}{name}"));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&folder)
            .map_err(|error| format!("{} cannot be created: {error}", folder.display()))?;
        let staging = Staging {
            binary: folder.join("quadratique"),
            folder,
        };
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&staging.binary)
            .and_then(|mut file| {
                file.write_all(bytes)?;
                file.sync_all()
            })
            .map_err(|error| format!("writing {} failed: {error}", staging.binary.display()))?;
        Ok(staging)
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
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::{Lock, Staging, check, remove_leftovers};

    fn folder() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn a_folder_that_other_accounts_can_write_is_refused_and_a_sticky_one_above_it_is_not() {
        let root = folder();
        let install = root.path().join("bin");
        std::fs::create_dir(&install).unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o1777)).unwrap();
        std::fs::set_permissions(&install, std::fs::Permissions::from_mode(0o755)).unwrap();
        check(&install).unwrap();

        std::fs::set_permissions(&install, std::fs::Permissions::from_mode(0o757)).unwrap();
        assert!(check(&install).unwrap_err().contains("chmod go-w"));

        std::fs::set_permissions(&install, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            check(&install)
                .unwrap_err()
                .contains("other accounts can write")
        );
    }

    #[test]
    fn staging_is_private_and_a_dropped_staging_folder_is_removed_without_touching_others() {
        let install = folder();
        let decoy = install.path().join(".quadratique-upgrade-decoy");
        std::fs::create_dir(&decoy).unwrap();
        std::fs::write(decoy.join("notes.txt"), "a person's file").unwrap();
        let staging = Staging::create(install.path(), b"binary").unwrap();
        let mode = std::fs::metadata(staging.binary.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        drop(staging);
        let left: Vec<_> = std::fs::read_dir(install.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(left, vec![decoy.clone()]);

        let lock = Lock::take(install.path()).unwrap();
        let leftover = install.path().join(".quadratique-upgrade-0123");
        std::fs::create_dir(&leftover).unwrap();
        std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(leftover.join("quadratique"), "half written").unwrap();
        assert_eq!(remove_leftovers(install.path(), &lock), vec![leftover]);
        assert!(decoy.join("notes.txt").exists());
    }

    #[test]
    fn a_second_lock_on_the_same_folder_is_refused() {
        let install = folder();
        let _held = Lock::take(install.path()).unwrap();
        assert!(
            Lock::take(install.path())
                .err()
                .unwrap()
                .contains("another quadratique upgrade")
        );
    }
}
