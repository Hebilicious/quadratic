//! After the binary changed, makes the server of the caller's home run it, or says exactly what
//! state was left and what to run.
//!
//! What runs is read from the kernel and systemd now, not from the unit file on disk: the home's
//! server names its pid, `/proc/<pid>/exe` gives the device and inode of the file it runs, and
//! `systemctl --user show` whether `quadratique.service` runs that same process. Only a server of
//! this home that runs the replaced file (the device and inode that the install lock recorded) is
//! restarted through systemd or stopped. Whatever cannot be inspected is left as it is: an unknown
//! executable, a `systemctl` that fails, a platform without `/proc` or pidfds.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use quadratique_server::server::identity;
use serde_json::Value;

use crate::background::{self, Found, Home};

const SERVICE: &str = "quadratique.service";
const SYSTEMCTL_LIMIT: Duration = Duration::from_secs(120);
/// One deadline for the whole readiness step after a restart.
const READY_LIMIT: Duration = Duration::from_secs(60);
/// The unit's `TimeoutStopSec`.
const STOP_LIMIT: Duration = Duration::from_secs(70);
const POLL: Duration = Duration::from_millis(100);
/// The store schema of every build before servers reported theirs.
const SCHEMA_BEFORE_REPORTS: i64 = 1;

pub type FileId = (u64, u64);

/// What a process runs, compared with the installed binary and the file it replaced.
#[derive(Debug, PartialEq, Eq)]
pub enum Runs {
    ThisFile,
    Replaced,
    Other(PathBuf),
    Unknown,
}

pub fn file_id(path: &Path) -> Option<FileId> {
    std::fs::metadata(path)
        .ok()
        .map(|meta| (meta.dev(), meta.ino()))
}

/// The device and inode of the file a process runs, from `/proc` (Linux only).
fn running_file(pid: u32) -> Option<FileId> {
    if cfg!(target_os = "linux") {
        file_id(Path::new(&format!("/proc/{pid}/exe")))
    } else {
        None
    }
}

pub fn what_runs(pid: u32, binary: &Path, replaced: Option<FileId>) -> Runs {
    let Some(running) = running_file(pid) else {
        return Runs::Unknown;
    };
    if Some(running) == file_id(binary) {
        return Runs::ThisFile;
    }
    if Some(running) == replaced {
        return Runs::Replaced;
    }
    match std::fs::read_link(format!("/proc/{pid}/exe")) {
        Ok(path) => Runs::Other(path),
        Err(_) => Runs::Unknown,
    }
}

pub struct HomeServer {
    pub pid: u32,
    pub schema: i64,
}

/// The server that answers for the home now, with the store schema it opened. Starts nothing.
pub async fn home_server(home: &Home) -> Result<Option<HomeServer>, String> {
    let runtime = match background::find(home).await {
        Ok(Found::Ours(runtime)) | Ok(Found::OtherInterface { runtime, .. }) => runtime,
        Ok(Found::Nobody) => return Ok(None),
        Ok(Found::Refused { reason }) => return Err(reason),
        Err(error) => return Err(error.to_string()),
    };
    let schema = identity(runtime.address)
        .await
        .ok()
        .and_then(|identity| identity.schema)
        .unwrap_or(SCHEMA_BEFORE_REPORTS);
    Ok(Some(HomeServer {
        pid: runtime.pid,
        schema,
    }))
}

enum Inspected {
    NoService,
    Service { main_pid: u32, needs_reload: bool },
    Unknown(String),
}

async fn run_systemctl(args: &[&str], limit: Duration) -> Result<String, Option<String>> {
    let mut command = tokio::process::Command::new("systemctl");
    command
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    match tokio::time::timeout(limit, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(Ok(output)) => Err(Some(format!(
            "systemctl {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Err(None),
        Ok(Err(error)) => Err(Some(format!("systemctl does not run: {error}"))),
        Err(_) => Err(Some(format!(
            "systemctl {} gave no answer within {} seconds",
            args.join(" "),
            limit.as_secs()
        ))),
    }
}

/// The loaded `quadratique.service` and its main process. Without `systemctl` there is no
/// service; a `systemctl` that fails could not be inspected.
async fn inspect_service(limit: Duration) -> Inspected {
    let shown = match run_systemctl(
        &[
            "show",
            SERVICE,
            "--property=LoadState,ActiveState,MainPID,NeedDaemonReload",
        ],
        limit,
    )
    .await
    {
        Ok(shown) => shown,
        Err(None) => return Inspected::NoService,
        Err(Some(problem)) => return Inspected::Unknown(problem),
    };
    let value = |name: &str| {
        shown
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .unwrap_or_default()
            .to_owned()
    };
    let main_pid: u32 = value("MainPID").parse().unwrap_or(0);
    if value("LoadState") != "loaded" || value("ActiveState") != "active" || main_pid == 0 {
        return Inspected::NoService;
    }
    Inspected::Service {
        main_pid,
        needs_reload: value("NeedDaemonReload") == "yes",
    }
}

/// What was done, or the state that was left with what to run. `Err` when the person must act.
pub async fn bring_up(
    home: &Home,
    binary: &Path,
    version: &str,
    replaced: Option<FileId>,
) -> Result<String, String> {
    let next = "the next quadratique command starts this build";
    let Some(server) = home_server(home).await? else {
        return Ok(format!("no server runs for this home; {next}"));
    };
    let pid = server.pid;
    match what_runs(pid, binary, replaced) {
        Runs::ThisFile => return Ok(format!("{version} serves this home (pid {pid})")),
        Runs::Other(path) => {
            return Ok(format!(
                "the server of this home (pid {pid}) runs {}, not the replaced binary: it was left running",
                path.display()
            ));
        }
        Runs::Unknown => {
            return Err(format!(
                "what the server of this home (pid {pid}) runs cannot be told on this system, so it was left running. Run: quadratique server stop; {next}"
            ));
        }
        Runs::Replaced => {}
    }
    match inspect_service(SYSTEMCTL_LIMIT).await {
        Inspected::Unknown(problem) => Err(format!(
            "{SERVICE} could not be inspected ({problem}), so the server of the earlier build (pid {pid}) was left running. Run: systemctl --user restart {SERVICE} if it runs this home's server, else quadratique server stop"
        )),
        Inspected::Service {
            main_pid,
            needs_reload,
        } if main_pid == pid => {
            if needs_reload {
                return Err(format!(
                    "{SERVICE} serves this home with the earlier build, and its unit changed on disk since systemd loaded it; nothing was restarted. Run: systemctl --user daemon-reload && systemctl --user restart {SERVICE}"
                ));
            }
            let deadline = Instant::now() + READY_LIMIT;
            run_systemctl(&["restart", SERVICE], READY_LIMIT)
                .await
                .map_err(|problem| {
                    format!(
                        "restarting {SERVICE} failed: {}. The earlier build may still serve this home. Run: systemctl --user restart {SERVICE}",
                        problem.unwrap_or_else(|| "systemctl is missing".to_owned())
                    )
                })?;
            ready(binary, version, deadline).await
        }
        Inspected::NoService | Inspected::Service { .. } => stop_replaced(pid, replaced)
            .await
            .map(|()| format!("stopped the server of the earlier build (pid {pid}); {next}")),
    }
}

/// Stops `pid` only while it still runs the replaced file: the check and the signal go through
/// one pidfd, so the signal cannot reach another process that took the pid in between.
#[cfg(target_os = "linux")]
async fn stop_replaced(pid: u32, replaced: Option<FileId>) -> Result<(), String> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use rustix::io::Errno;
    use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

    let leave = |why: String| format!("{why}; nothing was stopped. Run: quadratique server stop");
    let target = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| leave(format!("{pid} is not a process id")))?;
    let pidfd = match pidfd_open(target, PidfdFlags::empty()) {
        Ok(pidfd) => pidfd,
        Err(Errno::SRCH) => return Ok(()),
        Err(error) => return Err(leave(format!("pid {pid} cannot be opened: {error}"))),
    };
    if replaced.is_none() || running_file(pid) != replaced {
        return Err(leave(format!(
            "pid {pid} no longer runs the replaced binary"
        )));
    }
    match pidfd_send_signal(&pidfd, Signal::TERM) {
        Ok(()) => {}
        Err(Errno::SRCH) => return Ok(()),
        Err(error) => return Err(leave(format!("pid {pid} cannot be signalled: {error}"))),
    }
    tokio::task::spawn_blocking(move || {
        let limit = Timespec {
            tv_sec: STOP_LIMIT.as_secs() as i64,
            tv_nsec: 0,
        };
        let mut watched = [PollFd::new(&pidfd, PollFlags::IN)];
        poll(&mut watched, Some(&limit))
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|polled| polled.map_err(|error| error.to_string()))
    .and_then(|ready| {
        (ready == 1).then_some(()).ok_or_else(|| {
            format!(
                "the server of the earlier build (pid {pid}) was asked to stop and still runs after {} seconds. Run: quadratique server stop",
                STOP_LIMIT.as_secs()
            )
        })
    })
}

#[cfg(not(target_os = "linux"))]
async fn stop_replaced(pid: u32, _replaced: Option<FileId>) -> Result<(), String> {
    Err(format!(
        "the server of the earlier build (pid {pid}) cannot be verified on this system, so it was left running. Run: quadratique server stop"
    ))
}

/// Waits until the service's main process runs the installed file and the new build answers
/// for this home as that process with its own interface, the check `install.sh` makes. Every
/// call within shares one deadline.
async fn ready(binary: &Path, version: &str, deadline: Instant) -> Result<String, String> {
    let mut seen = "nothing was seen yet".to_owned();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!(
                "{SERVICE} was restarted, but the new build did not answer for this home as its main process within {} seconds ({seen}). Run: systemctl --user status {SERVICE}",
                READY_LIMIT.as_secs()
            ));
        }
        match inspect_service(left).await {
            Inspected::Service { main_pid, .. } => {
                if what_runs(main_pid, binary, None) != Runs::ThisFile {
                    seen = format!(
                        "the main process {main_pid} does not run {} yet",
                        binary.display()
                    );
                } else {
                    match status_of_new_build(binary, deadline).await {
                        Ok(status)
                            if status["running"] == true
                                && status["sameInterface"] == true
                                && status["pid"] == main_pid =>
                        {
                            return Ok(format!(
                                "restarted {SERVICE}; {version} serves this home (pid {main_pid})"
                            ));
                        }
                        Ok(status) => seen = format!("server status answered {status}"),
                        Err(problem) => seen = problem,
                    }
                }
            }
            Inspected::NoService => seen = format!("{SERVICE} is not active"),
            Inspected::Unknown(problem) => seen = problem,
        }
        tokio::time::sleep(POLL.min(deadline.saturating_duration_since(Instant::now()))).await;
    }
}

/// `server status` as the new build answers it.
async fn status_of_new_build(binary: &Path, deadline: Instant) -> Result<Value, String> {
    let mut command = tokio::process::Command::new(binary);
    command
        .args(["server", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let left = deadline.saturating_duration_since(Instant::now());
    match tokio::time::timeout(left, command.output()).await {
        Ok(Ok(output)) => serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("server status is not JSON: {error}")),
        Ok(Err(error)) => Err(format!("server status does not run: {error}")),
        Err(_) => Err("server status gave no answer before the deadline".to_owned()),
    }
}

/// The binary that the service's main process runs, when the service runs one.
pub async fn service_binary() -> Option<PathBuf> {
    let Inspected::Service { main_pid, .. } = inspect_service(SYSTEMCTL_LIMIT).await else {
        return None;
    };
    std::fs::read_link(format!("/proc/{main_pid}/exe")).ok()
}
