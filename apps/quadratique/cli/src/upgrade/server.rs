//! After the binary changed, makes the server of the caller's home stop running the replaced file, or says exactly
//! what state was left and what to run (docs/decisions/0004-…, §3).
//!
//! The home's server is found on its socket, as every command finds it (the state table, §2), and never over TCP.
//! Before the rename, the upgrade reads the server's identity: its generation and the file it runs (device, inode and
//! path, which the server took at its start). The answer comes over the verified socket, so it is as trusted as the
//! account. After the rename, only a server whose file is the one replaced is stopped, by a stop request addressed to
//! the generation seen before: a `409` means another generation serves now, and nothing is stopped. No process is
//! signalled from here.
//!
//! `start.lock` is held from before the last look through the rename and the stop, so no server starts in between.
//! The next command starts the new build; a server that a person or a supervisor started with `quadratique serve` is
//! restarted the way it was started.

use std::path::Path;
use std::time::{Duration, Instant};

use quadratique_server::server::{Identity, Started};

use crate::background::{self, Deadline, Home, Observed, StartLockHeld, Stopped};
use crate::process::{FileId, file_id};

/// One deadline for the stop after the rename.
const BRING_UP_LIMIT: Duration = Duration::from_secs(60);
/// A debug build, which the tests run, takes a shorter limit from this variable, in milliseconds, as it takes its
/// release source from `QUADRATIQUE_UPGRADE_SOURCE`. No release build reads it.
#[cfg(debug_assertions)]
const BRING_UP_VARIABLE: &str = "QUADRATIQUE_UPGRADE_BRING_UP_MS";

fn bring_up_limit() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(milliseconds) = std::env::var(BRING_UP_VARIABLE)
        .ok()
        .and_then(|value| value.parse().ok())
    {
        return Duration::from_millis(milliseconds);
    }
    BRING_UP_LIMIT
}
/// The store schema of every build before servers reported theirs.
const SCHEMA_BEFORE_REPORTS: i64 = 1;
const NEXT: &str = "the next quadratique command starts this build";

/// What served the home when the upgrade looked, before it replaced the binary.
#[derive(Debug)]
pub enum Before {
    Nobody,
    /// A server on the socket that answers this build's control version.
    Socket(Identity),
    /// A server from before the socket (release 0.1): the record's legacy row.
    Legacy,
    /// A row in which the upgrade may stop nothing, and why.
    Unknown(String),
}

/// The server seen on the socket, and the store schema it opened.
pub struct Seen {
    pub before: Before,
    /// `None` when no server reported one: the store file tells.
    pub schema: Option<i64>,
}

/// Looks at the home through the socket and the state table, and starts nothing. A socket that this process may not
/// reach is an error: the upgrade cannot know what serves the home.
pub async fn look(home: &Home) -> Result<Seen, String> {
    let observed = background::observe(home)
        .await
        .map_err(|error| error.to_string())?;
    let reported = |identity: &Identity| Some(identity.schema.unwrap_or(SCHEMA_BEFORE_REPORTS));
    Ok(match observed {
        Observed::Serving { identity, .. } => Seen {
            schema: reported(&identity),
            before: Before::Socket(identity),
        },
        Observed::Older(identity) => Seen {
            schema: reported(&identity),
            before: Before::Socket(identity),
        },
        Observed::Newer(identity) if background::lists_this_control(&identity) => Seen {
            schema: reported(&identity),
            before: Before::Socket(identity),
        },
        Observed::Free => Seen {
            before: Before::Nobody,
            schema: None,
        },
        Observed::Legacy => Seen {
            before: Before::Legacy,
            schema: None,
        },
        Observed::Starting { .. } => Seen {
            before: Before::Unknown(
                "a server of this home was starting or stopping, so the upgrade could not tell which file it runs, and stopped nothing. Run `quadratique server status`: if it does not name this build's interface, run `quadratique server stop`"
                    .to_owned(),
            ),
            schema: None,
        },
        Observed::Unreachable(reason) => return Err(reason),
        refused @ (Observed::Newer(_)
        | Observed::Malformed(_)
        | Observed::Stalled
        | Observed::Untrusted(_)
        | Observed::OtherDatabase(_)) => Seen {
            before: Before::Unknown(background::refused_to_stop(home, refused)),
            schema: None,
        },
    })
}

/// Whether the server's identity names `file` (device and inode) as the file it runs.
fn runs(identity: &Identity, file: FileId) -> bool {
    identity
        .executable
        .as_ref()
        .is_some_and(|executable| (executable.device, executable.inode) == file)
}

/// Whether the server seen is the one that a run killed between its rename and its stop left on the replaced file:
/// it was started from this path, from the file that the install lock recorded, and the path holds another file now.
/// The path guards against a freed inode that another file took.
pub fn left_on_the_replaced_file(identity: &Identity, binary: &Path, replaced: FileId) -> bool {
    runs(identity, replaced)
        && identity
            .executable
            .as_ref()
            .is_some_and(|executable| executable.path == binary)
        && file_id(binary) != Some(replaced)
}

/// What was done after the rename, or the state that was left with what to run. `Err` when the person must act.
/// `start` is `start.lock`, held since before the look that gave `before`; it is given up before this returns.
pub async fn bring_up(
    home: &Home,
    replaced: FileId,
    before: Before,
    start: StartLockHeld,
) -> Result<String, String> {
    let identity = match before {
        Before::Nobody => return Ok(format!("no server runs for this home; {NEXT}")),
        Before::Unknown(why) => return Err(why),
        Before::Legacy => return Err(legacy(home)),
        Before::Socket(identity) => identity,
    };
    let pid = identity.pid;
    let Some(executable) = identity.executable.as_ref() else {
        return Err(format!(
            "the server of this home (pid {pid}) does not say which file it runs, so it was left running. Run: quadratique server stop; {NEXT}"
        ));
    };
    if !runs(&identity, replaced) {
        return Ok(format!(
            "the server of this home (pid {pid}) runs {}, not the replaced binary: it was left running",
            executable.path.display()
        ));
    }
    stop_replaced(home, &identity, start).await
}

/// The legacy row's own account of the server and of what stops it: `server stop` with its proof, or the person.
fn legacy(home: &Home) -> String {
    let status = background::legacy_status(home);
    format!(
        "{}. The upgrade stopped nothing. To stop it: {}",
        status["reason"].as_str().unwrap_or_default(),
        status["stop"].as_str().unwrap_or_default()
    )
}

/// Stops the generation seen before the rename, which ran the replaced file, within one deadline.
async fn stop_replaced(
    home: &Home,
    identity: &Identity,
    start: StartLockHeld,
) -> Result<String, String> {
    let pid = identity.pid;
    let generation = identity.generation.clone().unwrap_or_default();
    let earlier = format!("the server of the earlier build (pid {pid}, generation {generation})");
    let deadline = Instant::now() + bring_up_limit();
    let what = stopped(home, &start, &generation, deadline, &earlier).await?;
    drop(start);
    match identity.started {
        Some(Started::Serve) => Err(format!(
            "{what}. It was started with `quadratique serve`: restart it the way you started it (your supervisor, your terminal), and it runs this build"
        )),
        _ => Ok(format!("{what}; {NEXT}")),
    }
}

/// Asks the generation to stop and waits until it lets go of the home. What happened, or why nothing was stopped.
async fn stopped(
    home: &Home,
    start: &StartLockHeld,
    generation: &str,
    deadline: Instant,
    earlier: &str,
) -> Result<String, String> {
    match background::stop_generation(home, start, generation, Deadline::at(deadline)).await {
        Ok(Stopped::Ended { .. }) => Ok(format!("stopped {earlier}")),
        Ok(Stopped::NotRunning) => Ok(format!("{earlier} had stopped already")),
        Ok(Stopped::Closing { .. }) => Err(format!(
            "{earlier} accepted the stop and still holds this home after {} seconds. Run: quadratique server status",
            bring_up_limit().as_secs()
        )),
        Err(error) => Err(error.to_string()),
    }
}
