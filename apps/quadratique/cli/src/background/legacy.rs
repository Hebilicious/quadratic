//! A server of release 0.1, or of any build before the socket (docs/decisions/0004-…, §2 and §3): the home's lock is
//! held, no socket answers, and a runtime file without a control version names a TCP address. Finding it is cheap and
//! stays. This command line never uses it, and stops it only with proof: on Linux, a pidfd pins the process that the
//! runtime file names, and `/proc` shows, while it is pinned, that the process runs a binary this install ran before
//! (the installed binary, or the file that `quadratique upgrade` replaced) and holds an open descriptor of this home's
//! `server.lock`. Then SIGTERM goes through the pidfd. Nothing the server answers over TCP counts: any process can
//! listen on a port.
//!
//! Without that proof nothing is signalled, and the person reads what to stop. Release 0.1 was published for Linux
//! only, and its code refuses no system, so a server built from its source can run on macOS: there this command line
//! reads no `/proc`, and always gives the instructions. Delete the proven stop no earlier than two minor releases after
//! the cutover (QUA-58), and only once the instructions cover every case.

use serde_json::{Value, json};

use super::{
    Attempt, Deadline, Home, LifeError, Observed, Stopped, legacy_instructions, observe_by,
};

/// What the command line can say about the process that the runtime file names.
pub enum Evidence {
    /// Made only on Linux; elsewhere `Proof` has no value.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Proven(Proof),
    Unproven {
        pid: Option<u32>,
        why: String,
    },
}

/// A process pinned by a pidfd, which runs a binary this install ran before and holds this home's lock.
#[cfg(target_os = "linux")]
pub struct Proof {
    pinned: crate::process::Pinned,
    runs: String,
}

/// No system but Linux gives the evidence, so there is never a proof.
#[cfg(not(target_os = "linux"))]
pub enum Proof {}

impl Proof {
    #[cfg(target_os = "linux")]
    fn pid(&self) -> u32 {
        self.pinned.pid()
    }

    #[cfg(not(target_os = "linux"))]
    fn pid(&self) -> u32 {
        match *self {}
    }

    #[cfg(target_os = "linux")]
    fn describe(&self) -> String {
        format!(
            "pid {} runs {} and holds this home's lock",
            self.pinned.pid(),
            self.runs
        )
    }

    #[cfg(not(target_os = "linux"))]
    fn describe(&self) -> String {
        match *self {}
    }

    /// True when the signal was sent, false when the process had exited already.
    #[cfg(target_os = "linux")]
    fn terminate(&self) -> Result<bool, String> {
        Ok(self.pinned.terminate()? == crate::process::Signalled::Sent)
    }

    #[cfg(not(target_os = "linux"))]
    fn terminate(&self) -> Result<bool, String> {
        match *self {}
    }
}

#[cfg(target_os = "linux")]
pub fn examine(home: &Home) -> Evidence {
    use crate::process::{Pinned, file_id};

    let Some(pid) = home.pid_hint() else {
        return Evidence::Unproven {
            pid: None,
            why: "its runtime file names no process".to_owned(),
        };
    };
    let unproven = |why: String| Evidence::Unproven {
        pid: Some(pid),
        why,
    };
    let lock_path = home.prepared.lock();
    let Some(lock) = file_id(&lock_path) else {
        return unproven(format!("{} cannot be read", lock_path.display()));
    };
    let pinned = match Pinned::open(pid) {
        Ok(Some(pinned)) => pinned,
        Ok(None) => return unproven(format!("no process has pid {pid}")),
        Err(why) => return unproven(why),
    };
    let expected = Expected::of_this_install();
    let runs_path = pinned.runs_path().map_or_else(
        || "a file that /proc does not name".to_owned(),
        |path| path.display().to_string(),
    );
    if !pinned
        .runs()
        .is_some_and(|runs| expected.files.contains(&runs))
    {
        return unproven(format!(
            "pid {pid} runs {runs_path}, which is neither the installed binary {} nor a binary that `quadratique upgrade` replaced",
            expected.installed
        ));
    }
    if !pinned.holds(lock) {
        return unproven(format!(
            "pid {pid} holds no descriptor of this home's lock, {}",
            lock_path.display()
        ));
    }
    Evidence::Proven(Proof {
        pinned,
        runs: runs_path,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn examine(home: &Home) -> Evidence {
    Evidence::Unproven {
        pid: home.pid_hint(),
        why: "only on Linux can this command line see what a process runs and holds".to_owned(),
    }
}

/// The binaries that this install ran before: what `install.sh` of release 0.1 installed
/// (`${XDG_BIN_HOME:-$HOME/.local/bin}/quadratique`), and the file that the last `quadratique upgrade` beside this
/// binary replaced. This binary itself is never one: it serves on the socket.
#[cfg(target_os = "linux")]
struct Expected {
    installed: String,
    files: Vec<crate::process::FileId>,
}

#[cfg(target_os = "linux")]
impl Expected {
    fn of_this_install() -> Expected {
        use std::path::PathBuf;

        use crate::process::file_id;

        let variable = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let installed = variable("XDG_BIN_HOME")
            .or_else(|| variable("HOME").map(|home| home.join(".local/bin")))
            .map(|folder| folder.join("quadratique"));
        let this = std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .ok();
        let replaced = this
            .as_deref()
            .and_then(std::path::Path::parent)
            .and_then(crate::upgrade::recorded_replaced);
        let this = this.as_deref().and_then(file_id);
        let files = [installed.as_deref().and_then(file_id), replaced]
            .into_iter()
            .flatten()
            .filter(|file| Some(*file) != this)
            .collect();
        Expected {
            installed: installed.map_or_else(
                || "(neither XDG_BIN_HOME nor HOME is set)".to_owned(),
                |path| path.display().to_string(),
            ),
            files,
        }
    }
}

/// The refusal of a command that meets this row. A start names `server stop` when a stop would be proven, and the
/// instructions otherwise.
pub fn refusal(home: &Home, evidence: Evidence, attempt: Attempt) -> LifeError {
    let (pid, unproven) = match evidence {
        Evidence::Proven(proof) => (Some(proof.pid()), None),
        Evidence::Unproven { pid, why } => (pid, Some(why)),
    };
    LifeError::Legacy {
        home: home.prepared.folder().to_path_buf(),
        pid,
        unproven,
        attempt,
    }
}

/// `server status` for this row: what serves, and what stops it. The pid is the runtime file's, verified only when the
/// stop is proven.
pub fn status(home: &Home) -> Value {
    let runtime = home.runtime_file();
    let mut value = json!({
        "running": true,
        "legacy": true,
        "sameInterface": false,
        "pid": runtime.as_ref().map(|runtime| runtime.pid),
        "url": runtime.as_ref().map(|runtime| runtime.url.clone()),
        "database": home.prepared.database(),
    });
    match examine(home) {
        Evidence::Proven(proof) => {
            value["provenStop"] = json!(true);
            value["reason"] = json!(format!(
                "a server from before the socket (release 0.1) serves this home over TCP only; {}",
                proof.describe()
            ));
            value["stop"] = json!("quadratique server stop");
        }
        Evidence::Unproven { pid, why } => {
            value["provenStop"] = json!(false);
            value["reason"] = json!(format!(
                "a server from before the socket (release 0.1) serves this home over TCP only, and this command line will not signal it: {why}"
            ));
            value["stop"] = json!(legacy_instructions(pid));
        }
    }
    value
}

pub enum Stop {
    Done(Stopped),
    /// The home no longer shows this row right before the signal: the caller observes again.
    NotLegacy,
}

/// Called with `start.lock` held, after an observation of this row. Signals the process only with the proof, and only
/// if the home still shows this row right before the signal; then waits for the home's lock to be free within the
/// deadline.
pub async fn stop(home: &Home, deadline: Deadline, attempt: Attempt) -> Result<Stop, LifeError> {
    let proof = match examine(home) {
        Evidence::Proven(proof) => proof,
        unproven @ Evidence::Unproven { .. } => return Err(refusal(home, unproven, attempt)),
    };
    if !matches!(observe_by(home, deadline).await?, Some(Observed::Legacy)) {
        return Ok(Stop::NotLegacy);
    }
    let pid = proof.pid();
    proof.terminate().map_err(|why| {
        refusal(
            home,
            Evidence::Unproven {
                pid: Some(pid),
                why,
            },
            attempt,
        )
    })?;
    loop {
        if !home.held().map_err(|error| home.unusable(error, attempt))? {
            if home
                .runtime_file()
                .is_some_and(|runtime| runtime.pid == pid)
            {
                let _ = std::fs::remove_file(home.prepared.runtime());
            }
            return Ok(Stop::Done(Stopped::Ended {
                pid,
                generation: None,
            }));
        }
        if !deadline.pause().await {
            return Ok(Stop::Done(Stopped::Closing {
                pid: Some(pid),
                generation: None,
            }));
        }
    }
}
