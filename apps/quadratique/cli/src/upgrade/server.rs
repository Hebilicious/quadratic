//! After the binary changed, makes the server of the caller's home run it, or says exactly what
//! state was left and what to run.
//!
//! The server is told by what systemd and the kernel say now, not by the unit file on disk: the
//! home's runtime file and its server name the process that serves this home, `/proc/<pid>/exe`
//! the file it runs, and `systemctl --user show` whether `quadratique.service` runs that same
//! process. Only a server of this home that runs an earlier build of the installed file is
//! restarted (through systemd) or stopped (without systemd).

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::background::{self, Found, Home, Stopped};

const SERVICE: &str = "quadratique.service";
const SYSTEMCTL_LIMIT: Duration = Duration::from_secs(120);
const READY_LIMIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(100);

/// What a process runs, compared with the installed binary.
#[derive(Debug, PartialEq, Eq)]
pub enum Runs {
    ThisFile,
    /// The file that was at the binary's path before it was replaced.
    EarlierBuild,
    Other(PathBuf),
    Unknown,
}

pub fn what_runs(pid: u32, binary: &Path) -> Runs {
    let exe = PathBuf::from(format!("/proc/{pid}/exe"));
    let (Ok(running), Ok(installed)) = (std::fs::metadata(&exe), std::fs::metadata(binary)) else {
        return Runs::Unknown;
    };
    if (running.dev(), running.ino()) == (installed.dev(), installed.ino()) {
        return Runs::ThisFile;
    }
    let Ok(link) = std::fs::read_link(&exe) else {
        return Runs::Unknown;
    };
    let shown = link.to_string_lossy();
    let path = shown.strip_suffix(" (deleted)").unwrap_or(&shown);
    if Path::new(path) == binary {
        Runs::EarlierBuild
    } else {
        Runs::Other(PathBuf::from(path))
    }
}

/// The process that serves the home now, when one answers. Starts nothing.
pub async fn home_server(home: &Home) -> Option<u32> {
    match background::find(home).await {
        Ok(Found::Ours(runtime)) | Ok(Found::OtherInterface { runtime, .. }) => Some(runtime.pid),
        _ => None,
    }
}

struct Service {
    main_pid: u32,
    needs_reload: bool,
}

async fn systemctl(args: &[&str]) -> Result<String, String> {
    let mut command = tokio::process::Command::new("systemctl");
    command
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    match tokio::time::timeout(SYSTEMCTL_LIMIT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(Ok(output)) => Err(format!(
            "{}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(format!(
            "no answer within {} seconds",
            SYSTEMCTL_LIMIT.as_secs()
        )),
    }
}

/// The loaded `quadratique.service` and its main process, when systemd runs one.
async fn service() -> Option<Service> {
    let shown = systemctl(&[
        "show",
        SERVICE,
        "--property=LoadState,ActiveState,MainPID,NeedDaemonReload",
    ])
    .await
    .ok()?;
    let value = |name: &str| {
        shown
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .unwrap_or_default()
            .to_owned()
    };
    if value("LoadState") != "loaded" || value("ActiveState") != "active" {
        return None;
    }
    Some(Service {
        main_pid: value("MainPID").parse().ok().filter(|pid| *pid != 0)?,
        needs_reload: value("NeedDaemonReload") == "yes",
    })
}

/// What was done, or the state that was left with what to run. `Err` only when something the
/// person must fix was found.
pub async fn bring_up(home: &Home, binary: &Path, version: &str) -> Result<String, String> {
    let service = service().await;
    let Some(pid) = home_server(home).await else {
        let mut said =
            "no server runs for this home; the next quadratique command starts this build"
                .to_owned();
        if let Some(service) = &service
            && what_runs(service.main_pid, binary) == Runs::EarlierBuild
        {
            said.push_str(&format!(
                ". {SERVICE} runs the earlier build for another home: systemctl --user restart {SERVICE}"
            ));
        }
        return Ok(said);
    };
    match what_runs(pid, binary) {
        Runs::ThisFile => return Ok(format!("{version} serves this home (pid {pid})")),
        Runs::Other(path) => {
            return Ok(format!(
                "the server of this home (pid {pid}) runs {}, not the replaced binary: it was left running",
                path.display()
            ));
        }
        Runs::EarlierBuild | Runs::Unknown => {}
    }
    match service {
        Some(service) if service.main_pid == pid => {
            if service.needs_reload {
                return Err(format!(
                    "{SERVICE} serves this home with the earlier build, and its unit changed on disk since systemd loaded it; nothing was restarted. Run: systemctl --user daemon-reload && systemctl --user restart {SERVICE}"
                ));
            }
            systemctl(&["restart", SERVICE]).await.map_err(|error| {
                format!(
                    "restarting {SERVICE} failed: {error}. The earlier build may still serve this home. Run: systemctl --user restart {SERVICE}"
                )
            })?;
            ready(home, binary, version).await
        }
        _ => match background::stop(home).await {
            Ok(Stopped::Ended { pid }) => Ok(format!(
                "stopped the server of the earlier build (pid {pid}); the next quadratique command starts this build"
            )),
            Ok(Stopped::NotRunning) => Ok(
                "no server runs for this home; the next quadratique command starts this build".to_owned(),
            ),
            Ok(Stopped::Closing { .. }) => Ok(
                "the server of the earlier build is stopping; the next quadratique command starts this build".to_owned(),
            ),
            Err(error) => Err(format!(
                "stopping the server of the earlier build failed: {error}. Run: quadratique server stop"
            )),
        },
    }
}

/// Waits, as `install.sh` does, until the service's main process runs the installed file and is
/// the server that answers for this home.
async fn ready(home: &Home, binary: &Path, version: &str) -> Result<String, String> {
    let deadline = Instant::now() + READY_LIMIT;
    loop {
        if let Some(service) = service().await
            && what_runs(service.main_pid, binary) == Runs::ThisFile
            && home_server(home).await == Some(service.main_pid)
        {
            return Ok(format!(
                "restarted {SERVICE}; {version} serves this home (pid {})",
                service.main_pid
            ));
        }
        if Instant::now() > deadline {
            return Err(format!(
                "{SERVICE} was restarted, but no server of {} answered for this home within {} seconds. Run: systemctl --user status {SERVICE}",
                binary.display(),
                READY_LIMIT.as_secs()
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The binary that the service's main process runs, when the service runs a copy of it.
pub async fn service_binary() -> Option<PathBuf> {
    let service = service().await?;
    let link = std::fs::read_link(format!("/proc/{}/exe", service.main_pid)).ok()?;
    let shown = link.to_string_lossy();
    Some(PathBuf::from(
        shown.strip_suffix(" (deleted)").unwrap_or(&shown),
    ))
}
