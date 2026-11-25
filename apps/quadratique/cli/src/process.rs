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

    /// Whether one of the process's open descriptors is `file`.
    pub fn holds(&self, file: FileId) -> bool {
        let Ok(descriptors) = std::fs::read_dir(format!("/proc/{}/fd", self.pid)) else {
            return false;
        };
        descriptors
            .filter_map(Result::ok)
            .any(|descriptor| file_id(&descriptor.path()) == Some(file))
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

    /// Whether the process exits within `limit`.
    pub async fn exits_within(self, limit: std::time::Duration) -> Result<bool, String> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};

        tokio::task::spawn_blocking(move || {
            let limit = Timespec {
                tv_sec: i64::try_from(limit.as_secs()).unwrap_or(i64::MAX),
                tv_nsec: i64::from(limit.subsec_nanos()),
            };
            let mut watched = [PollFd::new(&self.pidfd, PollFlags::IN)];
            poll(&mut watched, Some(&limit))
        })
        .await
        .map_err(|error| error.to_string())?
        .map(|ready| ready == 1)
        .map_err(|error| error.to_string())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    use super::{Pinned, Signalled, file_id};

    #[test]
    fn a_pinned_process_shows_what_it_runs_and_holds_and_a_signal_ends_it() {
        let directory = tempfile::tempdir().expect("a folder");
        let held = directory.path().join("held");
        std::fs::write(&held, "").expect("a file");
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("exec 9<\"$0\"; echo open; read -r _")
            .arg(&held)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("a process");
        let mut said = String::new();
        BufReader::new(child.stdout.take().expect("its output"))
            .read_line(&mut said)
            .expect("the process says it holds the file");
        let pinned = Pinned::open(child.id())
            .expect("a pidfd")
            .expect("the process");

        assert!(pinned.holds(file_id(&held).expect("the file's identity")));
        assert!(!pinned.holds(file_id(directory.path()).expect("an identity")));
        assert_eq!(pinned.runs(), file_id(std::path::Path::new("/bin/sh")));
        assert_eq!(pinned.terminate(), Ok(Signalled::Sent));
        let status = child.wait().expect("the process ends");
        assert!(!status.success());
        assert_eq!(pinned.terminate(), Ok(Signalled::Gone));
    }
}
