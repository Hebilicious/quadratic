//! Replaces the binary file, then restarts the server so that the new build answers.
//!
//! The new binary is written beside the old one under a temporary name, run with `--version`, and
//! only then renamed over the old one. Until that rename the old binary is in place, and on any
//! failure the temporary file is removed.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::background::{self, Home, Stopped};

const VERSION_LIMIT: Duration = Duration::from_secs(30);
const RESTART_LIMIT: Duration = Duration::from_secs(120);
const SERVICE: &str = "quadratique.service";

pub struct Staged {
    path: PathBuf,
    destination: PathBuf,
    installed: bool,
}

impl Staged {
    pub fn write(bytes: &[u8], destination: &Path) -> Result<Staged, String> {
        use std::os::unix::fs::OpenOptionsExt;

        let folder = destination.parent().unwrap_or(Path::new("/"));
        let path = folder.join(format!(".quadratique-upgrade-{}", std::process::id()));
        let staged = Staged {
            path,
            destination: destination.to_owned(),
            installed: false,
        };
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&staged.path)
            .and_then(|mut file| {
                file.write_all(bytes)?;
                file.sync_all()
            })
            .map_err(|error| format!("writing {} failed: {error}", staged.path.display()))?;
        Ok(staged)
    }

    /// Runs the new binary with `--version`; it must print `expected`.
    pub async fn runs_as(&self, expected: &str) -> Result<(), String> {
        let mut command = tokio::process::Command::new(&self.path);
        command
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = match tokio::time::timeout(VERSION_LIMIT, command.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => return Err(format!("the new binary does not start: {error}")),
            Err(_) => {
                return Err(format!(
                    "the new binary did not answer --version within {} seconds",
                    VERSION_LIMIT.as_secs()
                ));
            }
        };
        let printed = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !output.status.success() {
            return Err(format!(
                "the new binary's --version failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        if printed != expected {
            return Err(format!(
                "the new binary says it is {printed:?}, not {expected}"
            ));
        }
        Ok(())
    }

    pub fn install(mut self) -> Result<(), String> {
        std::fs::rename(&self.path, &self.destination).map_err(|error| {
            format!(
                "renaming {} over {} failed: {error}",
                self.path.display(),
                self.destination.display()
            )
        })?;
        self.installed = true;
        Ok(())
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.installed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// The binary that the user unit written by `install.sh` runs, when there is one.
pub fn service_binary() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let unit = Path::new(&home).join(".config/systemd/user").join(SERVICE);
    let text = std::fs::read_to_string(unit).ok()?;
    text.lines().find_map(|line| {
        let (path, _) = line.strip_prefix("ExecStart=\"")?.split_once('"')?;
        Path::new(path).canonicalize().ok()
    })
}

/// Restarts what runs the server, so that the new build answers. Returns what it did, or why it
/// could not and what to run.
pub async fn restart(home: &Home, binary: &Path) -> Result<String, String> {
    if service_binary().as_deref() == Some(binary) {
        let mut command = tokio::process::Command::new("systemctl");
        command
            .args(["--user", "restart", SERVICE])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let failed = |why: String| {
            format!("restarting {SERVICE} failed: {why}. Run: systemctl --user restart {SERVICE}")
        };
        return match tokio::time::timeout(RESTART_LIMIT, command.output()).await {
            Ok(Ok(output)) if output.status.success() => Ok(format!("restarted {SERVICE}")),
            Ok(Ok(output)) => Err(failed(format!(
                "{}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ))),
            Ok(Err(error)) => Err(failed(error.to_string())),
            Err(_) => Err(failed(format!(
                "no answer within {} seconds",
                RESTART_LIMIT.as_secs()
            ))),
        };
    }
    match background::stop(home).await {
        Ok(Stopped::Ended { pid }) => Ok(format!(
            "stopped the server of the earlier build (pid {pid}); the next command starts this one"
        )),
        Ok(Stopped::NotRunning) => Ok("no server ran; the next command starts this build".into()),
        Ok(Stopped::Closing { .. }) => Ok(
            "the server of the earlier build is stopping; the next command starts this one".into(),
        ),
        Err(error) => Err(format!(
            "stopping the server of the earlier build failed: {error}. Run: quadratique server stop"
        )),
    }
}
