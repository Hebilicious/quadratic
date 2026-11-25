//! A server of release 0.1, or of any build before the socket (docs/decisions/0004-…, §2 and §3): the home's lock is
//! held, no socket answers, and a runtime file without a control version names a TCP address. Finding it is cheap and
//! stays. This command line never uses it, and stops it only with proof.
//!
//! The proof exists so that no process that is not this home's old server is ever signalled by mistake: a stale
//! runtime file, a reused process number, a wrong path, another program. A process of the same account that
//! deliberately imitates the old server is out of scope: it already has the account's files.
//!
//! On Linux, a pidfd pins the process that the runtime file names. Then `/proc` must number processes as this
//! process's namespace does, and show that the process runs the installed binary (a regular file of this account,
//! reached without a link, in a folder that the install folder's rule accepts) and holds the exclusive `flock` of this
//! home's `server.lock`; the process must still be alive after those reads. Under `start.lock`, right before the
//! signal, the home must still show this row, the runtime file must still name that process, and the process must
//! still run that file and hold that lock. Then SIGTERM goes through the pidfd. Nothing the server answers over TCP
//! counts. A verified server could still `exec` another program between the last check and the signal: release 0.1's
//! server never does, and only the same account could make it.
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

/// A process pinned by a pidfd, which ran the installed binary and held this home's lock when it was examined.
#[cfg(target_os = "linux")]
pub struct Proof {
    pinned: crate::process::Pinned,
    runs: crate::process::FileId,
    runs_path: String,
    lock: crate::process::FileId,
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
            self.runs_path
        )
    }

    #[cfg(not(target_os = "linux"))]
    fn describe(&self) -> String {
        match *self {}
    }

    /// The last check, under `start.lock`: the runtime file still names this process, and it still runs the file and
    /// holds the lock that were verified.
    #[cfg(target_os = "linux")]
    fn still_holds(&self, home: &Home) -> Result<(), String> {
        let pid = self.pinned.pid();
        match home.pid_hint() {
            Some(named) if named == pid => {}
            Some(named) => {
                return Err(format!(
                    "the runtime file now names pid {named}, not the verified pid {pid}"
                ));
            }
            None => return Err("the runtime file no longer names a process".to_owned()),
        }
        verify(&self.pinned, self.lock, Ok(self.runs)).map(|_| ())
    }

    #[cfg(not(target_os = "linux"))]
    fn still_holds(&self, _home: &Home) -> Result<(), String> {
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
    match verify(&pinned, lock, installed_binary()) {
        Ok((runs, runs_path)) => Evidence::Proven(Proof {
            pinned,
            runs,
            runs_path,
            lock,
        }),
        Err(why) => unproven(why),
    }
}

/// What the pinned process runs, when `/proc` describes that very process, it runs `expected`, it holds the exclusive
/// lock of `lock`, and it is still alive after the reads.
#[cfg(target_os = "linux")]
fn verify(
    pinned: &crate::process::Pinned,
    lock: crate::process::FileId,
    expected: Result<crate::process::FileId, String>,
) -> Result<(crate::process::FileId, String), String> {
    let pid = pinned.pid();
    if !pinned.numbered_alike_in_proc() {
        return Err(
            "/proc does not number processes as this command line's own PID namespace does"
                .to_owned(),
        );
    }
    let runs_path = pinned.runs_path().map_or_else(
        || "a file that /proc does not name".to_owned(),
        |path| path.display().to_string(),
    );
    let runs = pinned.runs();
    match &expected {
        Ok(expected) if runs == Some(*expected) => {}
        Ok(_) => {
            return Err(format!(
                "pid {pid} runs {runs_path}, which is not the installed binary"
            ));
        }
        Err(why) => {
            return Err(format!(
                "pid {pid} runs {runs_path}, and the installed binary does not count: {why}"
            ));
        }
    }
    if !pinned.holds_lock(lock) {
        return Err(format!("pid {pid} does not hold this home's lock"));
    }
    if !pinned.alive() {
        return Err(format!("pid {pid} exited while it was examined"));
    }
    Ok((runs.unwrap_or_default(), runs_path))
}

#[cfg(not(target_os = "linux"))]
pub fn examine(home: &Home) -> Evidence {
    Evidence::Unproven {
        pid: home.pid_hint(),
        why: "only on Linux can this command line see what a process runs and holds".to_owned(),
    }
}

/// Where `install.sh` of release 0.1 put the binary: `${XDG_BIN_HOME:-$HOME/.local/bin}/quadratique`. It counts only as
/// a regular file of this account, reached without a link, in a folder that the install folder's rule accepts, and
/// never as this binary's own file. A file that `quadratique upgrade` replaced does not count: nothing durable says
/// which file it was (QUA-56).
#[cfg(target_os = "linux")]
fn installed_binary() -> Result<crate::process::FileId, String> {
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;

    use crate::process::file_id;

    let variable = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let folder = variable("XDG_BIN_HOME")
        .or_else(|| variable("HOME").map(|home| home.join(".local/bin")))
        .ok_or("neither XDG_BIN_HOME nor HOME names an absolute folder")?;
    let binary = folder.join("quadratique");
    let resolved = folder
        .canonicalize()
        .map_err(|error| format!("{} cannot be read: {error}", folder.display()))?;
    if resolved != folder {
        return Err(format!("{} is reached through a link", folder.display()));
    }
    crate::upgrade::check_install_folder(&folder)?;
    let meta = std::fs::symlink_metadata(&binary)
        .map_err(|error| format!("{} cannot be read: {error}", binary.display()))?;
    if !meta.file_type().is_file() {
        return Err(format!("{} is not a regular file", binary.display()));
    }
    let me = quadratique_server::home::own_account();
    if meta.uid() != me {
        return Err(format!(
            "{} belongs to another account (uid {})",
            binary.display(),
            meta.uid()
        ));
    }
    let installed = (meta.dev(), meta.ino());
    if file_id(std::path::Path::new("/proc/self/exe")) == Some(installed) {
        return Err(format!(
            "{} is this command line's own file",
            binary.display()
        ));
    }
    Ok(installed)
}

/// The refusal of a command that meets this row. A start names `server stop` when the process evidence holds, and the
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

/// The proof, or the refusal: what a stop or a take-over looks at before it reserves `start.lock`, so that a refusal
/// leaves nothing behind.
pub fn proof_or_refusal(home: &Home, attempt: Attempt) -> Result<Proof, LifeError> {
    match examine(home) {
        Evidence::Proven(proof) => Ok(proof),
        unproven @ Evidence::Unproven { .. } => Err(refusal(home, unproven, attempt)),
    }
}

/// `server status` for this row: what serves, and what stops it. The pid is the runtime file's. `provenStop` is the
/// evidence about the process; a stop still needs `start.lock`, and checks the evidence again under it.
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
    /// The home no longer shows this row right before the signal, or the process had exited: the caller observes
    /// again.
    NotLegacy,
}

/// Called with `start.lock` held, after an observation of this row, with the proof found before the reservation if
/// there was one. Signals that process only if the last check holds; then waits for the home's lock to be free within
/// the deadline.
pub async fn stop(
    home: &Home,
    deadline: Deadline,
    attempt: Attempt,
    proof: Option<Proof>,
) -> Result<Stop, LifeError> {
    let proof = match proof {
        Some(proof) => proof,
        None => proof_or_refusal(home, attempt)?,
    };
    if !matches!(observe_by(home, deadline).await?, Some(Observed::Legacy)) {
        return Ok(Stop::NotLegacy);
    }
    let pid = proof.pid();
    let unproven = |why: String| {
        refusal(
            home,
            Evidence::Unproven {
                pid: Some(pid),
                why,
            },
            attempt,
        )
    };
    proof.still_holds(home).map_err(unproven)?;
    if !proof.terminate().map_err(unproven)? {
        return Ok(Stop::NotLegacy);
    }
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
