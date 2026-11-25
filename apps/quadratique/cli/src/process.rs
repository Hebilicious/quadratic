//! What the command line does to the server's process: start it apart from the caller, and, on Linux, pin a process
//! with a pidfd to read what it runs and holds, and signal it. A pidfd is the only way this command line signals a
//! process: a signal through it cannot reach another process that took the same pid.

use std::process::Command;

/// The server leaves the caller's process group, so a Ctrl-C in the caller's terminal does not reach it.
#[cfg(unix)]
pub fn detach(command: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt;

    command.process_group(0)
}

#[cfg(windows)]
pub fn detach(command: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP)
}

/// Windows has no SIGTERM, and the server does not serve there yet: QUA-61 chooses how to stop it.
#[cfg(windows)]
pub fn ask_to_stop(_pid: u32) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "stopping the server is not supported on Windows yet (QUA-61)",
    ))
}

/// Without a safe way to ask Windows yet, every pid counts as alive, so a runtime file is never removed while its
/// server may run.
#[cfg(windows)]
pub fn alive(_pid: u32) -> bool {
    true
}

/// The device and inode of a file.
#[cfg(unix)]
pub type FileId = (u64, u64);

#[cfg(unix)]
pub fn file_id(path: &std::path::Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;

    std::fs::metadata(path)
        .ok()
        .map(|meta| (meta.dev(), meta.ino()))
}

/// The file a process runs, from `/proc` (Linux only). Without a pidfd this names whatever process has the pid now:
/// use it only to describe, never before a signal.
#[cfg(unix)]
pub fn running_file(pid: u32) -> Option<FileId> {
    if cfg!(target_os = "linux") {
        file_id(std::path::Path::new(&format!("/proc/{pid}/exe")))
    } else {
        None
    }
}

/// A process held by a pidfd. What `/proc/<pid>` shows while the process is alive is that process's own: its pid
/// cannot be reused before it exits. If it exits between a read and the signal, the signal fails and reaches no other
/// process.
#[cfg(target_os = "linux")]
pub struct Pinned {
    pid: u32,
    pidfd: std::os::fd::OwnedFd,
}

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
pub enum Signalled {
    Sent,
    /// The process had exited already.
    Gone,
}

#[cfg(target_os = "linux")]
impl Pinned {
    /// `None` when no process has the pid.
    pub fn open(pid: u32) -> Result<Option<Pinned>, String> {
        use rustix::io::Errno;
        use rustix::process::{Pid, PidfdFlags, pidfd_open};

        let target = i32::try_from(pid)
            .ok()
            .and_then(Pid::from_raw)
            .ok_or_else(|| format!("{pid} is not a process id"))?;
        match pidfd_open(target, PidfdFlags::empty()) {
            Ok(pidfd) => Ok(Some(Pinned { pid, pidfd })),
            Err(Errno::SRCH) => Ok(None),
            Err(error) => Err(format!("pid {pid} cannot be opened: {error}")),
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The device and inode of the file the process runs, even when that file was replaced since.
    pub fn runs(&self) -> Option<FileId> {
        running_file(self.pid)
    }

    /// The path the process was started from, as the kernel names it (with " (deleted)" once the file was replaced).
    pub fn runs_path(&self) -> Option<std::path::PathBuf> {
        std::fs::read_link(format!("/proc/{}/exe", self.pid)).ok()
    }

    /// Whether one of the process's open descriptors is `file` and holds an exclusive `flock` on it. The kernel lists
    /// the locks of an open file in `/proc/<pid>/fdinfo/<fd>` (Linux 4.13 and later); where it lists none, there is no
    /// proof. A descriptor of the same file opened apart from the one that took the lock shows no lock.
    pub fn holds_lock(&self, file: FileId) -> bool {
        let Ok(descriptors) = std::fs::read_dir(format!("/proc/{}/fd", self.pid)) else {
            return false;
        };
        descriptors.filter_map(Result::ok).any(|descriptor| {
            file_id(&descriptor.path()) == Some(file)
                && std::fs::read_to_string(format!(
                    "/proc/{}/fdinfo/{}",
                    self.pid,
                    descriptor.file_name().to_string_lossy()
                ))
                .is_ok_and(|fdinfo| holds_exclusive_flock(&fdinfo, file.1))
        })
    }

    /// Whether `/proc` numbers processes as this process's own PID namespace does, and gives the pinned process the
    /// number it was opened by: then `/proc/<pid>` is the pinned process for as long as it is alive. A `/proc` mounted
    /// from another namespace names this process by another number, or not at all.
    pub fn numbered_alike_in_proc(&self) -> bool {
        use std::os::fd::AsRawFd;

        let ours = std::fs::read_link("/proc/self")
            .is_ok_and(|path| path.as_os_str() == std::process::id().to_string().as_str());
        let pinned =
            std::fs::read_to_string(format!("/proc/self/fdinfo/{}", self.pidfd.as_raw_fd()))
                .is_ok_and(|fdinfo| {
                    fdinfo
                        .lines()
                        .find_map(|line| line.strip_prefix("Pid:"))
                        .is_some_and(|number| number.trim() == self.pid.to_string())
                });
        ours && pinned
    }

    /// Whether the process has not exited. Checked after reading `/proc/<pid>`: while it is alive, no other process
    /// can have had its number during the reads.
    pub fn alive(&self) -> bool {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};

        let now = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let mut watched = [PollFd::new(&self.pidfd, PollFlags::IN)];
        poll(&mut watched, Some(&now)).is_ok_and(|ready| ready == 0)
    }

    /// SIGTERM: a server closes its store, then exits.
    pub fn terminate(&self) -> Result<Signalled, String> {
        use rustix::io::Errno;
        use rustix::process::{Signal, pidfd_send_signal};

        match pidfd_send_signal(&self.pidfd, Signal::TERM) {
            Ok(()) => Ok(Signalled::Sent),
            Err(Errno::SRCH) => Ok(Signalled::Gone),
            Err(error) => Err(format!("pid {} cannot be signalled: {error}", self.pid)),
        }
    }
}

/// A `lock:` line of an fdinfo file, as `/proc/locks` writes it: `lock:\t1: FLOCK  ADVISORY  WRITE <pid>
/// <major>:<minor>:<inode> 0 EOF`. Only the inode is compared: on btrfs the device there is the file system's, not the
/// subvolume's that `stat` gives.
#[cfg(target_os = "linux")]
fn holds_exclusive_flock(fdinfo: &str, inode: u64) -> bool {
    fdinfo
        .lines()
        .filter_map(|line| line.strip_prefix("lock:"))
        .any(|lock| {
            let words: Vec<&str> = lock.split_whitespace().collect();
            words.get(1) == Some(&"FLOCK")
                && words.get(3) == Some(&"WRITE")
                && words
                    .get(5)
                    .and_then(|file| file.rsplit(':').next())
                    .is_some_and(|number| number == inode.to_string())
        })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::path::Path;
    use std::process::{Child, Command, Stdio};

    use super::{Pinned, Signalled, file_id, holds_exclusive_flock};

    /// A shell that runs `script` on `file` and says once it is ready.
    fn shell(script: &str, file: &Path) -> Child {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(format!("{script}; echo ready; read -r _"))
            .arg(file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("a process");
        let mut said = String::new();
        BufReader::new(child.stdout.take().expect("its output"))
            .read_line(&mut said)
            .expect("the process says it is ready");
        child
    }

    #[test]
    fn a_pinned_process_shows_what_it_runs_and_the_lock_it_holds_and_a_signal_ends_it() {
        let directory = tempfile::tempdir().expect("a folder");
        let lock = directory.path().join("server.lock");
        std::fs::write(&lock, "").expect("a file");
        let lock_id = file_id(&lock).expect("the file's identity");
        let mut holder = shell("exec 9>>\"$0\"; flock -x 9", &lock);
        let mut opener = shell("exec 9<\"$0\"", &lock);
        let holding = Pinned::open(holder.id())
            .expect("a pidfd")
            .expect("a process");
        let only_open = Pinned::open(opener.id())
            .expect("a pidfd")
            .expect("a process");

        assert!(holding.holds_lock(lock_id), "the holder of the lock");
        assert!(
            !only_open.holds_lock(lock_id),
            "a descriptor of the same file that holds no lock"
        );
        assert!(holding.numbered_alike_in_proc());
        assert!(holding.alive());
        assert_eq!(holding.runs(), file_id(Path::new("/bin/sh")));
        assert_eq!(holding.terminate(), Ok(Signalled::Sent));
        assert!(!holder.wait().expect("the process ends").success());
        assert!(!holding.alive());
        assert_eq!(holding.terminate(), Ok(Signalled::Gone));
        let _ = opener.kill();
        let _ = opener.wait();
    }

    #[test]
    fn only_an_exclusive_flock_on_the_same_inode_counts() {
        let line = |kind: &str, mode: &str, inode: u64| {
            format!(
                "pos:\t0\nflags:\t0102001\nlock:\t1: {kind}  ADVISORY  {mode} 42 00:1e:{inode} 0 EOF\n"
            )
        };

        assert!(holds_exclusive_flock(&line("FLOCK", "WRITE", 7), 7));
        assert!(!holds_exclusive_flock(&line("FLOCK", "READ", 7), 7));
        assert!(!holds_exclusive_flock(&line("POSIX", "WRITE", 7), 7));
        assert!(!holds_exclusive_flock(&line("FLOCK", "WRITE", 77), 7));
        assert!(!holds_exclusive_flock("pos:\t0\nflags:\t0100000\n", 7));
    }
}
