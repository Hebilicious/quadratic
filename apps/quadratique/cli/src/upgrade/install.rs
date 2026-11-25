//! Runs the staged binary with `--version` before it is installed. It runs with an empty
//! environment, from `/`, and only its first bytes of output are read: an unexpected binary sees
//! no home, no store and no repository, and cannot fill memory.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;

const VERSION_LIMIT: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: u64 = 4096;

pub async fn runs_as(binary: &Path, expected: &str) -> Result<(), String> {
    let mut command = tokio::process::Command::new(binary);
    command
        .arg("--version")
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| format!("the new binary does not start: {error}"))?;
    let mut stdout = child.stdout.take().expect("a piped standard output");
    let run = async {
        let mut printed = Vec::new();
        (&mut stdout)
            .take(OUTPUT_LIMIT + 1)
            .read_to_end(&mut printed)
            .await?;
        if printed.len() as u64 > OUTPUT_LIMIT {
            return Ok((None, printed));
        }
        Ok::<_, std::io::Error>((Some(child.wait().await?), printed))
    };
    let (status, printed) = match tokio::time::timeout(VERSION_LIMIT, run).await {
        Ok(Ok(done)) => done,
        Ok(Err(error)) => return Err(format!("the new binary's --version failed: {error}")),
        Err(_) => {
            return Err(format!(
                "the new binary did not answer --version within {} seconds",
                VERSION_LIMIT.as_secs()
            ));
        }
    };
    let Some(status) = status else {
        return Err(format!(
            "the new binary printed more than {OUTPUT_LIMIT} bytes for --version"
        ));
    };
    if !status.success() {
        return Err(format!("the new binary's --version failed ({status})"));
    }
    let printed = String::from_utf8_lossy(&printed).trim().to_owned();
    if printed != expected {
        return Err(format!(
            "the new binary says it is {printed:?}, not {expected}"
        ));
    }
    Ok(())
}
