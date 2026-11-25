//! What the command line does to the server's process: start it apart from the caller, ask it to stop, and see
//! whether it still runs.

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

/// SIGTERM: the server closes its store, then exits.
#[cfg(unix)]
pub fn ask_to_stop(pid: u32) -> std::io::Result<()> {
    use rustix::process::{Signal, kill_process};

    let pid = pid_of(pid).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the server gave no valid pid",
        )
    })?;
    kill_process(pid, Signal::TERM).map_err(std::io::Error::from)
}

/// Windows has no SIGTERM, and the server does not serve there yet: QUA-34 chooses how to stop it.
#[cfg(windows)]
pub fn ask_to_stop(_pid: u32) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "stopping the server is not supported on Windows yet (QUA-34)",
    ))
}

#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    pid_of(pid).is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

/// Without a safe way to ask Windows yet, every pid counts as alive, so a runtime file is never removed while its
/// server may run.
#[cfg(windows)]
pub fn alive(_pid: u32) -> bool {
    true
}

#[cfg(unix)]
fn pid_of(pid: u32) -> Option<rustix::process::Pid> {
    rustix::process::Pid::from_raw(i32::try_from(pid).ok()?)
}
