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
//!
//! Until the cutover (QUA-60), `quadratique.service` may run that server. When the unit's main process is the
//! generation's process, the upgrade stops the generation the same way, gives up `start.lock`, waits for systemd to see
//! its main process leave, and starts the unit again: its `serve --take-over` now runs the new build. A unit with
//! `Restart=on-failure` does not start a server that exited 0 by itself. A server that a command starts between the
//! stop and the unit's take-over is replaced by the take-over; the answer names each one the upgrade saw.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use quadratique_server::server::{Identity, Started};

use crate::background::{self, Deadline, Home, Observed, StartLockHeld, Stopped};
use crate::process::{FileId, file_id};

const SERVICE: &str = "quadratique.service";
/// For a `systemctl` call that is not part of the bring-up's one deadline.
const SYSTEMCTL_LIMIT: Duration = Duration::from_secs(120);
/// One deadline for the whole step after the rename: the stop, the unit's start and its readiness.
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
const POLL: Duration = Duration::from_millis(100);
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
/// `start` is `start.lock`, held since before the look that gave `before`; it is given up before this returns, and
/// before a service's own start.
pub async fn bring_up(
    home: &Home,
    binary: &Path,
    version: &str,
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
    stop_replaced(home, binary, version, &identity, start).await
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

/// Stops the generation seen before the rename, which ran the replaced file. Under the service, starts the unit again.
/// Every step shares one deadline.
async fn stop_replaced(
    home: &Home,
    binary: &Path,
    version: &str,
    identity: &Identity,
    start: StartLockHeld,
) -> Result<String, String> {
    let pid = identity.pid;
    let generation = identity.generation.clone().unwrap_or_default();
    let earlier = format!("the server of the earlier build (pid {pid}, generation {generation})");
    let deadline = Instant::now() + bring_up_limit();
    match inspect_service(left(deadline)).await {
        Inspected::Unknown(problem) => Err(format!(
            "{SERVICE} could not be inspected ({problem}), so {earlier} was left running. Run: quadratique server stop, then systemctl --user start {SERVICE} if the service ran it"
        )),
        Inspected::Service {
            main_pid,
            needs_reload,
        } if main_pid == pid => {
            if needs_reload {
                return Err(format!(
                    "{SERVICE} runs {earlier}, and its unit changed on disk since systemd loaded it; nothing was stopped. Run: systemctl --user daemon-reload && systemctl --user restart {SERVICE}"
                ));
            }
            stopped(home, &start, &generation, deadline, &earlier).await?;
            // The unit's `serve --take-over` takes `start.lock` too.
            drop(start);
            service_left(main_pid, deadline).await?;
            let mut others = Others::new(&generation);
            others.see(observed_by(home, deadline).await, None);
            if left(deadline).is_zero() {
                return Err(not_ready(
                    &earlier,
                    "the deadline passed before the unit's start",
                ));
            }
            run_systemctl(&["start", SERVICE], left(deadline))
                .await
                .map_err(|problem| {
                    format!(
                        "{earlier} was stopped, and starting {SERVICE} again failed: {}. Run: systemctl --user start {SERVICE}",
                        problem.unwrap_or_else(|| "systemctl is missing".to_owned())
                    )
                })?;
            ready(home, binary, version, deadline, &earlier, others).await
        }
        Inspected::NoService | Inspected::Service { .. } => {
            let what = stopped(home, &start, &generation, deadline, &earlier).await?;
            drop(start);
            match identity.started {
                Some(Started::Serve) => Err(format!(
                    "{what}. It was started with `quadratique serve`: restart it the way you started it (your supervisor, your terminal), and it runs this build"
                )),
                _ => Ok(format!("{what}; {NEXT}")),
            }
        }
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

fn left(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// One observation of the socket within what is left of the deadline: `None` once it has passed.
async fn observed_by(home: &Home, deadline: Instant) -> Option<Result<Observed, String>> {
    match background::observe_by(home, Deadline::at(deadline)).await {
        Ok(Some(observed)) => Some(Ok(observed)),
        Ok(None) => None,
        Err(error) => Some(Err(error.to_string())),
    }
}

fn not_ready(earlier: &str, seen: &str) -> String {
    format!(
        "{earlier} was stopped, and the new build did not answer for this home as the main process of {SERVICE} within {} seconds ({seen}). Run: systemctl --user status {SERVICE}",
        bring_up_limit().as_secs()
    )
}

/// The generations other than the stopped one that the upgrade saw serving while the service started: a command
/// started each one after the stop, and the unit's take-over replaces it.
struct Others {
    stopped: String,
    seen: Vec<(String, u32)>,
}

impl Others {
    fn new(stopped: &str) -> Others {
        Others {
            stopped: stopped.to_owned(),
            seen: Vec::new(),
        }
    }

    /// Records a server on the socket that is neither the stopped generation nor the unit's main process.
    fn see(&mut self, observed: Option<Result<Observed, String>>, main_pid: Option<u32>) {
        let Some(Ok(Observed::Serving { identity, .. } | Observed::Newer(identity))) = observed
        else {
            return;
        };
        let generation = identity.generation.unwrap_or_default();
        if generation != self.stopped
            && Some(identity.pid) != main_pid
            && !self.seen.iter().any(|(seen, _)| *seen == generation)
        {
            self.seen.push((generation, identity.pid));
        }
    }

    /// What the answer says of them, given the generation that serves at the end.
    fn told(&self, serving: &str) -> String {
        self.seen
            .iter()
            .filter(|(generation, _)| generation != serving)
            .map(|(generation, pid)| {
                format!(
                    ". Generation {generation} (pid {pid}), which a command started after the earlier build stopped, was replaced by the take-over of {SERVICE}, whose server serves now"
                )
            })
            .collect()
    }
}

/// Waits until systemd no longer names `main_pid` as the unit's main process: the unit is inactive, or systemd itself
/// started another.
async fn service_left(main_pid: u32, deadline: Instant) -> Result<(), String> {
    loop {
        let seen = match inspect_service(left(deadline)).await {
            Inspected::NoService => return Ok(()),
            Inspected::Service { main_pid: now, .. } if now != main_pid => return Ok(()),
            Inspected::Service { .. } => format!("its main process is still {main_pid}"),
            Inspected::Unknown(problem) => problem,
        };
        if left(deadline).is_zero() {
            return Err(format!(
                "the server of the earlier build stopped, and {SERVICE} did not let go of it within {} seconds ({seen}). Run: systemctl --user start {SERVICE}",
                bring_up_limit().as_secs()
            ));
        }
        tokio::time::sleep(POLL.min(left(deadline))).await;
    }
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

/// Waits until the server on the home's socket is the service's main process and says that it runs the installed
/// file. The identity comes over the verified socket, the main process from systemd: no process is read through
/// `/proc`. Each call gets what is left of the one deadline.
async fn ready(
    home: &Home,
    binary: &Path,
    version: &str,
    deadline: Instant,
    earlier: &str,
    mut others: Others,
) -> Result<String, String> {
    let installed = file_id(binary);
    let mut seen = "nothing was seen yet".to_owned();
    loop {
        if left(deadline).is_zero() {
            return Err(not_ready(earlier, &seen));
        }
        match inspect_service(left(deadline)).await {
            Inspected::Service { main_pid, .. } => match observed_by(home, deadline).await {
                None => seen = "the deadline passed during a look at the socket".to_owned(),
                Some(Ok(Observed::Serving { identity, .. } | Observed::Newer(identity)))
                    if identity.pid == main_pid
                        && installed.is_some_and(|installed| runs(&identity, installed)) =>
                {
                    let serving = identity.generation.unwrap_or_default();
                    return Ok(format!(
                        "stopped the earlier build and started {SERVICE} again; {version} serves this home (pid {main_pid}, generation {serving}){}",
                        others.told(&serving)
                    ));
                }
                Some(observed) => {
                    seen = match &observed {
                        Ok(observed) => format!("the home's socket shows {observed:?}"),
                        Err(error) => error.clone(),
                    };
                    others.see(Some(observed), Some(main_pid));
                }
            },
            Inspected::NoService => seen = format!("{SERVICE} is not active"),
            Inspected::Unknown(problem) => seen = problem,
        }
        tokio::time::sleep(POLL.min(left(deadline))).await;
    }
}

/// On a run that finds no server: when `quadratique.service` is loaded, not active, and starts this binary, what to
/// run. It starts nothing: the person may have stopped the unit on purpose.
pub async fn service_not_running(binary: &Path) -> Option<String> {
    let shown = run_systemctl(
        &[
            "show",
            SERVICE,
            "--property=LoadState,ActiveState,ExecStart",
        ],
        SYSTEMCTL_LIMIT,
    )
    .await
    .ok()?;
    let value = |name: &str| {
        shown
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .unwrap_or_default()
            .to_owned()
    };
    let runs_this_binary = value("ExecStart")
        .split(" ; ")
        .find_map(|part| part.trim_start_matches("{ ").strip_prefix("path="))
        .is_some_and(|path| Path::new(path.trim()) == binary);
    let active = value("ActiveState");
    (value("LoadState") == "loaded" && active != "active" && active != "activating" && runs_this_binary)
        .then(|| {
            format!(
                "no server runs for this home, and {SERVICE}, which starts this binary, is not running ({active}). If an earlier upgrade stopped it, start it: systemctl --user start {SERVICE}. Otherwise the next quadratique command starts this build"
            )
        })
}

/// The binary that the service's main process runs, when the service runs one. It only describes: nothing is
/// stopped on the strength of it.
pub async fn service_binary() -> Option<PathBuf> {
    let Inspected::Service { main_pid, .. } = inspect_service(SYSTEMCTL_LIMIT).await else {
        return None;
    };
    std::fs::read_link(format!("/proc/{main_pid}/exe")).ok()
}
