use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use quadratique_server::app::interface;
use quadratique_server::home::{self, Prepared};
#[cfg(not(unix))]
use quadratique_server::server::identity;
#[cfg(unix)]
use quadratique_server::server::{
    CONTROL, Identity, PROBE_LIMIT, StopAnswer, identity_over_socket_within, stop_over_socket,
};
use quadratique_server::server::{Paths, RuntimeFile, START_LIMIT, Unanswered};
use quadratique_server::store::canonical_folder;

use crate::process::detach;
use crate::route::McpTool;

#[cfg(unix)]
mod legacy;
#[cfg(not(unix))]
mod over_tcp;

pub const STOP_LIMIT: Duration = Duration::from_secs(30);
/// Set in the environment of a server that a command starts, so that its identity says it was started on demand.
pub const STARTED_VARIABLE: &str = "QUADRATIQUE_STARTED";
pub const ON_DEMAND: &str = "on-demand";
const POLL: Duration = Duration::from_millis(20);
const LOG_TAIL: usize = 2000;

/// Windows has no socket yet (QUA-61), so the rows that only the socket shows never happen there.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug)]
pub enum LifeError {
    Io(std::io::Error),
    /// The home policy refuses the data folder, or its path is not one: the person changes the folder.
    Refused(String),
    /// The runtime file names another database: only Windows reads it to find a server (QUA-61).
    #[cfg(not(unix))]
    OtherDatabase {
        runtime: PathBuf,
        database: PathBuf,
    },
    Exited {
        status: std::process::ExitStatus,
        log: PathBuf,
        tail: String,
    },
    NoAnswer {
        pid: Option<u32>,
        log: PathBuf,
    },
    OtherInterface {
        pid: u32,
        theirs: TheirInterface,
    },
    Unreachable {
        reason: String,
    },
    UnreachableToStop {
        reason: String,
    },
    /// A server holds the home past the operation's deadline: it never became ready, or never let go.
    Hung {
        hang: Hang,
        home: PathBuf,
        log: PathBuf,
        pid: Option<u32>,
        attempt: Attempt,
    },
    /// The socket accepted a connection and gave no full answer within the probe limit.
    Stalled {
        socket: PathBuf,
        log: PathBuf,
        pid: Option<u32>,
        attempt: Attempt,
    },
    Malformed {
        socket: PathBuf,
        reason: String,
        attempt: Attempt,
    },
    ServesOtherDatabase {
        socket: PathBuf,
        database: PathBuf,
        attempt: Attempt,
    },
    Newer {
        control: u32,
        attempt: Attempt,
    },
    /// The server answered `409`: it is not the generation this command asked to stop.
    OtherGeneration {
        asked: String,
        serving: Option<String>,
    },
    /// The server answered the stop request with another status than `202` or `409`. A build between the socket and
    /// the stop request answers `405`.
    CannotStop {
        status: String,
        pid: u32,
    },
    /// A server from before the socket (release 0.1) holds the home. `unproven` says why this command line will not
    /// signal it; without it, `server stop` stops it.
    Legacy {
        home: PathBuf,
        pid: Option<u32>,
        unproven: Option<String>,
        attempt: Attempt,
    },
    /// The whole stop request was sent and its answer was lost: the server may or may not be stopping.
    StopUnconfirmed {
        generation: String,
        reason: String,
        lock: String,
    },
}

#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hang {
    NotReady,
    NotStopped,
}

#[derive(Debug)]
pub enum TheirInterface {
    Named(String),
    FromABuildBeforeInterfaces,
}

impl TheirInterface {
    #[cfg(not(unix))]
    pub fn named(&self) -> Option<&str> {
        match self {
            TheirInterface::Named(interface) => Some(interface),
            TheirInterface::FromABuildBeforeInterfaces => None,
        }
    }
}

impl std::fmt::Display for TheirInterface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TheirInterface::Named(interface) => formatter.write_str(interface),
            TheirInterface::FromABuildBeforeInterfaces => {
                formatter.write_str("none, from a build before interfaces")
            }
        }
    }
}

impl std::fmt::Display for LifeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifeError::Io(error) => {
                write!(formatter, "the Quadratique home is not usable: {error}")
            }
            LifeError::Refused(reason) => formatter.write_str(reason),
            #[cfg(not(unix))]
            LifeError::OtherDatabase { runtime, database } => write!(
                formatter,
                "the runtime file {} names a server of another database, {}: stop that server, or set QUADRATIQUE_HOME",
                runtime.display(),
                database.display()
            ),
            LifeError::Exited { status, log, tail } => write!(
                formatter,
                "the Quadratique server stopped as it started ({status}); its log, {}, ends with: {tail}",
                log.display()
            ),
            LifeError::NoAnswer { pid, log } => {
                let process = pid.map_or_else(String::new, |pid| format!(" (pid {pid})"));
                write!(
                    formatter,
                    "a Quadratique server{process} holds this home but does not answer; see {} and end that process",
                    log.display()
                )
            }
            LifeError::OtherInterface { pid, theirs } => {
                write!(
                    formatter,
                    "the Quadratique server of this home (pid {pid}) is of another build: its interface is {theirs}, this command's is {}. Run `quadratique server stop`; the next command starts this build's server",
                    interface()
                )
            }
            LifeError::Unreachable { reason } => {
                formatter.write_str(&cannot_reach(reason, "Nothing was started."))
            }
            LifeError::UnreachableToStop { reason } => {
                formatter.write_str(&cannot_reach(reason, "Nothing was stopped."))
            }
            LifeError::Hung {
                hang,
                home,
                log,
                pid,
                attempt,
            } => {
                let what = match hang {
                    Hang::NotReady => "did not become ready",
                    Hang::NotStopped => "did not let go of it",
                };
                write!(
                    formatter,
                    "a Quadratique server holds {} and {what} within the time allowed; its log is {}. {} {}",
                    home.display(),
                    log.display(),
                    attempt.outcome(),
                    recovery(*pid)
                )
            }
            LifeError::Stalled {
                socket,
                log,
                pid,
                attempt,
            } => write!(
                formatter,
                "the Quadratique server on {} accepted a connection and gave no full answer within {:?}; its log is {}. {} {}",
                socket.display(),
                PROBE,
                log.display(),
                attempt.outcome(),
                recovery(*pid)
            ),
            LifeError::Malformed {
                socket,
                reason,
                attempt,
            } => write!(
                formatter,
                "what answers on {} is not a Quadratique server that this command line can use: {reason}. {} See what listens there with `quadratique server status`.",
                socket.display(),
                attempt.outcome()
            ),
            LifeError::ServesOtherDatabase {
                socket,
                database,
                attempt,
            } => write!(
                formatter,
                "the server on {} serves another database, {}. {} Set QUADRATIQUE_HOME to the home of that database, or stop that server from its own home.",
                socket.display(),
                database.display(),
                attempt.outcome()
            ),
            LifeError::Newer { control, attempt } => write!(
                formatter,
                "the server of this home speaks control version {control}, which this command line does not know. {} Upgrade this command line.",
                attempt.outcome()
            ),
            LifeError::OtherGeneration { asked, serving } => {
                let serving = serving
                    .as_deref()
                    .map_or_else(String::new, |serving| format!(" ({serving})"));
                write!(
                    formatter,
                    "the server of this home is no longer generation {asked}, which this command asked to stop: another generation{serving} serves now. Nothing was stopped."
                )
            }
            LifeError::StopUnconfirmed {
                generation,
                reason,
                lock,
            } => write!(
                formatter,
                "the stop of generation {generation} is not confirmed: {reason}. The home's lock is {lock}. Run `quadratique server status` to see what serves now."
            ),
            LifeError::CannotStop { status, pid } => {
                write!(
                    formatter,
                    "the server of this home cannot be stopped from here: the stop request got status {status}. Nothing was stopped."
                )?;
                if status == "405" {
                    formatter.write_str(
                        " It may be a build between the socket and the stop request, which does not serve that request.",
                    )?;
                }
                write!(
                    formatter,
                    " End it yourself, then run the command again. {}",
                    recovery(Some(*pid))
                )
            }
            LifeError::Legacy {
                home,
                pid,
                unproven,
                attempt,
            } => {
                write!(
                    formatter,
                    "a Quadratique server from before the socket (release 0.1) holds {} and serves only over TCP, which this command line does not use.",
                    home.display()
                )?;
                match (unproven, attempt) {
                    (None, Attempt::Start) => formatter.write_str(
                        " Nothing was started. Run `quadratique server stop`, which stops it, then run the command again.",
                    ),
                    (None, Attempt::Stop) => write!(formatter, " {}", attempt.outcome()),
                    (Some(why), attempt) => write!(
                        formatter,
                        " It sends no signal without proof that the process is that server, and has none: {why}. {} {}",
                        attempt.outcome(),
                        legacy_instructions(*pid)
                    ),
                }
            }
        }
    }
}

#[cfg(unix)]
const PROBE: Duration = PROBE_LIMIT;
#[cfg(not(unix))]
const PROBE: Duration = quadratique_server::server::PROBE_LIMIT;

/// What a person does about a server that this command line will not signal: the process number comes from the
/// server's last answer or its runtime file, and nothing has verified it.
fn recovery(pid: Option<u32>) -> String {
    let check = match usable_pid(pid) {
        Some(pid) => format!(
            "its process number was last given as {pid}, unverified: check it with `ps -p {pid} -o args` before you end it"
        ),
        None => "find its process first".to_owned(),
    };
    format!(
        "To recover: press Ctrl-C in the terminal that runs `quadratique serve`, stop it with the supervisor that started it, or end the process ({check})."
    )
}

/// A process number worth printing in a command: positive, a valid pid, and not this process. `kill 0` would signal the
/// caller's own process group, and a negative number a whole group.
fn usable_pid(pid: Option<u32>) -> Option<u32> {
    pid.filter(|pid| *pid > 0 && i32::try_from(*pid).is_ok() && *pid != std::process::id())
}

/// What a person runs to stop a server from before the socket that this command line will not signal.
fn legacy_instructions(pid: Option<u32>) -> String {
    let process = match usable_pid(pid) {
        Some(pid) => format!(
            "otherwise check pid {pid}, which nothing has verified, with `ps -p {pid} -o args`, and only if it shows this data folder's Quadratique server, end it with `kill {pid}`"
        ),
        None => "otherwise find the process that serves this data folder and end it".to_owned(),
    };
    format!(
        "Stop it yourself, then run the command again: if `quadratique.service` runs it (`systemctl --user status quadratique.service` names its main PID), run `systemctl --user stop quadratique.service`; if it runs in a terminal, press Ctrl-C there; {process}."
    )
}

impl LifeError {
    /// A sandbox that blocks the command still lets the harness start `quadratique mcp`, so the refusal tells the
    /// agent which tool to call instead, or that the command works only on the command line.
    pub fn for_a_command(&self, mcp_tool: &McpTool) -> String {
        let LifeError::Unreachable { reason } = self else {
            return self.to_string();
        };
        let outcome = match mcp_tool {
            McpTool::Offered(tool) => format!(
                "Nothing was started. Call the Quadratique MCP tool `{tool}` with the same inputs instead. \
                 The MCP tools act from the folder this session started in; for work in another worktree, \
                 pass its absolute path as `worktree` to `issue.claim`; once that claim succeeds, later calls act from it. \
                 If this session has no Quadratique MCP tools, tell the human that its harness must start \
                 `quadratique mcp` as an MCP server."
            ),
            McpTool::OnePerIssue(tool) => format!(
                "Nothing was started. To act on these issues together, all or none, run this command \
                 outside the sandbox, or ask the human to. If separate writes are acceptable, call the \
                 Quadratique MCP tool `{tool}` once for each issue, with the same other inputs."
            ),
            McpTool::Absent => "Nothing was started. This command works only on the command line: \
                 run it outside the sandbox, or ask the human to."
                .to_owned(),
            McpTool::NotACommand => {
                "Nothing was started. `quadratique --help` lists the commands.".to_owned()
            }
        };
        cannot_reach(reason, &outcome)
    }
}

fn cannot_reach(reason: &str, outcome: &str) -> String {
    format!("Cannot reach the Quadratique server from here: {reason}. {outcome}")
}

impl From<std::io::Error> for LifeError {
    fn from(error: std::io::Error) -> LifeError {
        LifeError::Io(error)
    }
}

/// The home of this command, as the policy checked it. Every file it names comes from `Prepared`, which only the
/// policy makes.
pub struct Home {
    prepared: Prepared,
}

impl Home {
    /// Applies the home policy before anything opens a lock file in the home or starts a server for it, and makes the
    /// home when it is missing.
    pub fn new(paths: Paths) -> Result<Home, LifeError> {
        let prepared = home::prepare(&paths).map_err(|refusal| match refusal.denied() {
            Some(error) => LifeError::Unreachable {
                reason: format!(
                    "this process cannot use {}: {error}; a sandbox may be blocking it",
                    paths.database.parent().unwrap_or(Path::new(".")).display()
                ),
            },
            None => LifeError::Refused(refusal.to_string()),
        })?;
        Ok(Home { prepared })
    }

    pub fn prepared(&self) -> &Prepared {
        &self.prepared
    }

    /// The socket of this home, which a command calls once `observe` has found its server there.
    #[cfg(unix)]
    pub fn endpoint(&self) -> Endpoint {
        Endpoint {
            base: quadratique_server::server::base_over_socket(),
            socket: Some(self.prepared.socket()),
        }
    }

    pub fn log(&self) -> PathBuf {
        self.prepared.log()
    }

    fn start_lock(&self) -> PathBuf {
        self.prepared.start_lock()
    }

    fn open_lock(&self, path: &Path) -> std::io::Result<File> {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
    }

    /// The store's path as a server names it in its identity and its runtime file.
    fn database(&self) -> PathBuf {
        let database = self.prepared.database();
        let file = database.file_name().unwrap_or_default();
        canonical_folder(self.prepared.folder())
            .map(|directory| directory.join(file))
            .unwrap_or(database)
    }

    fn runtime_file(&self) -> Option<RuntimeFile> {
        let text = std::fs::read(self.prepared.runtime()).ok()?;
        serde_json::from_slice(&text).ok()
    }

    /// A runtime file without a control version was written by a build before the socket. A missing, malformed or
    /// newer runtime file is no hint.
    #[cfg(unix)]
    fn legacy_hint(&self) -> bool {
        self.runtime_file()
            .is_some_and(|runtime| runtime.control.is_none())
    }

    /// For a message only: nothing has verified it.
    fn pid_hint(&self) -> Option<u32> {
        self.runtime_file().map(|runtime| runtime.pid)
    }

    /// Asking creates nothing, so that a command that only looks (`status`, `init --dry-run`, `upgrade --skills`)
    /// writes nothing in the home. `flock` takes a lock on a read-only descriptor, so a process that may not write the
    /// home can still ask, and a missing lock file means that no server ever held the home.
    pub fn held(&self) -> std::io::Result<bool> {
        let lock = match File::open(self.prepared.lock()) {
            Ok(lock) => lock,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if !lock.metadata()?.is_file() {
            return Err(std::io::Error::other("it is not a regular file"));
        }
        match lock.try_lock() {
            Ok(()) => Ok(false),
            Err(TryLockError::WouldBlock) => Ok(true),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempt {
    Start,
    Stop,
}

impl Attempt {
    fn outcome(self) -> &'static str {
        match self {
            Attempt::Start => "Nothing was started.",
            Attempt::Stop => "Nothing was stopped.",
        }
    }
}

impl Home {
    /// A home this process may not write is one it cannot start or stop a server of: the same outcome as a server it
    /// cannot connect to.
    pub fn unusable(&self, error: std::io::Error, attempt: Attempt) -> LifeError {
        if !cannot_write(&error) {
            return LifeError::Io(error);
        }
        let reason = format!(
            "this process cannot write {}: {error}; a sandbox may be blocking it",
            self.prepared.folder().display()
        );
        match attempt {
            Attempt::Start => LifeError::Unreachable { reason },
            Attempt::Stop => LifeError::UnreachableToStop { reason },
        }
    }
}

/// `EROFS`, `EACCES` and `EPERM` on Unix; `ERROR_ACCESS_DENIED` on Windows.
fn cannot_write(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ReadOnlyFilesystem | std::io::ErrorKind::PermissionDenied
    )
}

/// What the runtime file and TCP show. Only Windows, which has no socket yet (QUA-61), finds a server this way.
#[cfg(not(unix))]
pub enum Found {
    Nobody,
    Ours(RuntimeFile),
    OtherInterface {
        runtime: RuntimeFile,
        theirs: TheirInterface,
    },
    /// The probe was refused (`EPERM`, `EACCES` or the owner-only `403`). It names a server of this home only while the
    /// home is held: otherwise the runtime file is stale and its port belongs to someone else.
    Refused {
        reason: String,
    },
}

#[cfg(not(unix))]
impl Found {
    fn refusal(runtime: &RuntimeFile, theirs: TheirInterface) -> LifeError {
        LifeError::OtherInterface {
            pid: runtime.pid,
            theirs,
        }
    }

    fn unanswered(runtime: &RuntimeFile, unanswered: Unanswered) -> Found {
        let address = runtime.address;
        match unanswered {
            Unanswered::NotPermitted(error) => Found::Refused {
                reason: format!("connecting to {address} is not permitted ({error})"),
            },
            Unanswered::OwnerOnly => Found::Refused {
                reason: format!(
                    "the server at {address} answers only processes of its own account"
                ),
            },
            Unanswered::NotListening(_)
            | Unanswered::PathError(_)
            | Unanswered::Stalled
            | Unanswered::Malformed(_)
            | Unanswered::NotReady(_) => Found::Nobody,
        }
    }
}

#[cfg(not(unix))]
pub async fn find(home: &Home) -> Result<Found, LifeError> {
    let Some(runtime) = home.runtime_file() else {
        return Ok(Found::Nobody);
    };
    let database = home.database();
    if runtime.database != database {
        return Err(LifeError::OtherDatabase {
            runtime: home.prepared.runtime(),
            database: runtime.database,
        });
    }
    Ok(match identity(runtime.address).await {
        Ok(identity) if identity.database == database => {
            let runtime = RuntimeFile {
                pid: identity.pid,
                control: identity.control,
                ..runtime
            };
            match identity.interface {
                Some(theirs) if theirs == interface() => Found::Ours(runtime),
                Some(theirs) => Found::OtherInterface {
                    runtime,
                    theirs: TheirInterface::Named(theirs),
                },
                None => Found::OtherInterface {
                    runtime,
                    theirs: TheirInterface::FromABuildBeforeInterfaces,
                },
            }
        }
        Ok(_) => Found::Nobody,
        Err(unanswered) => Found::unanswered(&runtime, unanswered),
    })
}

/// What the socket of this home shows, kept apart as the record's state table needs it (§2).
#[cfg(unix)]
#[derive(Debug)]
pub enum OnSocket {
    /// A server of this build's control version, for this home's database.
    Serving(Identity),
    OtherDatabase(Identity),
    /// A control version that this build does not know: a newer server.
    Newer(Identity),
    Malformed(String),
    Stalled,
    /// An entry that is not yet a socket of this account with mode 0600.
    NotReady(String),
    Denied(String),
    PathError(String),
    /// Confirmed absence or refusal: no socket, or `ECONNREFUSED`.
    Absent,
}

#[cfg(unix)]
async fn on_socket_within(home: &Home, limit: Duration) -> OnSocket {
    let socket = home.prepared.socket();
    let identity = match identity_over_socket_within(&home.prepared, limit).await {
        Ok(identity) => identity,
        Err(Unanswered::NotListening(_)) => return OnSocket::Absent,
        Err(Unanswered::NotPermitted(error)) => {
            return OnSocket::Denied(format!(
                "connecting to {} is not permitted ({error})",
                socket.display()
            ));
        }
        Err(Unanswered::PathError(error)) => {
            return OnSocket::PathError(format!("{} cannot be reached: {error}", socket.display()));
        }
        Err(Unanswered::NotReady(why)) => return OnSocket::NotReady(why),
        Err(Unanswered::Stalled) => return OnSocket::Stalled,
        Err(Unanswered::Malformed(why)) => return OnSocket::Malformed(why),
        Err(Unanswered::OwnerOnly) => {
            return OnSocket::Malformed("the socket answered as a TCP listener does".to_owned());
        }
    };
    // The generation and the database keep their meaning in every control version, so they are checked first: a newer
    // server is stopped only by its generation, and only in its own home.
    let generation_is_valid = identity.generation.as_deref().is_some_and(|generation| {
        generation.len() == 32 && generation.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    if !generation_is_valid {
        return OnSocket::Malformed("the identity lacks a valid generation".to_owned());
    }
    if identity.database != home.database() {
        return OnSocket::OtherDatabase(identity);
    }
    match identity.control {
        None => return OnSocket::Malformed("the identity has no control version".to_owned()),
        Some(control) if control != CONTROL => return OnSocket::Newer(identity),
        Some(_) => {}
    }
    if identity.started.is_none() || identity.url.is_none() {
        return OnSocket::Malformed("the identity lacks its start or its url".to_owned());
    }
    OnSocket::Serving(identity)
}

/// `server status` of a server from before the socket: what serves, and what stops it.
#[cfg(unix)]
pub fn legacy_status(home: &Home) -> serde_json::Value {
    legacy::status(home)
}

/// One row of the record's state table (docs/decisions/0004-…, §2): what the socket shows, and behind a socket that is
/// absent or refuses, the home's lock and the runtime file. `status`, `ensure`, `stop` and `serve --take-over` each act
/// on it.
#[cfg(unix)]
#[derive(Debug)]
pub enum Observed {
    /// A server of this home's database and of this build's control version. Its interface may be another build's.
    Serving {
        identity: Identity,
    },
    Newer(Identity),
    OtherDatabase(Identity),
    Malformed(String),
    Stalled,
    /// `EACCES`, `EPERM` or a path error on the socket or the lock.
    Unreachable(String),
    /// The lock is held and the socket does not answer yet, or no longer: a server that has not bound its socket, or
    /// one that is stopping.
    Starting {
        reason: Option<String>,
    },
    /// The lock is held, no socket answers, and a runtime file of a build before the socket names a TCP address.
    Legacy,
    /// The lock is free, and the socket is absent or refuses.
    Free,
}

#[cfg(unix)]
pub async fn observe(home: &Home) -> Result<Observed, LifeError> {
    observe_within(home, PROBE_LIMIT).await
}

#[cfg(unix)]
async fn observe_within(home: &Home, limit: Duration) -> Result<Observed, LifeError> {
    Ok(match on_socket_within(home, limit).await {
        OnSocket::Serving(identity) => Observed::Serving { identity },
        OnSocket::OtherDatabase(identity) => Observed::OtherDatabase(identity),
        OnSocket::Newer(identity) => Observed::Newer(identity),
        OnSocket::Malformed(why) => Observed::Malformed(why),
        OnSocket::Stalled => Observed::Stalled,
        OnSocket::Denied(reason) | OnSocket::PathError(reason) => Observed::Unreachable(reason),
        OnSocket::NotReady(why) => behind_the_socket(home, Some(why), false),
        OnSocket::Absent => behind_the_socket(home, None, true),
    })
}

/// One deadline for one operation (record 0004, §2). Each probe gets what is left of it, at most the probe limit,
/// and nothing new begins once it has passed.
#[derive(Clone, Copy, Debug)]
pub struct Deadline(Instant);

impl Deadline {
    pub fn after(limit: Duration) -> Deadline {
        Deadline(Instant::now() + limit)
    }

    /// `quadratique upgrade` shares one deadline between its stop and its service's start (Unix only).
    #[cfg(unix)]
    pub fn at(instant: Instant) -> Deadline {
        Deadline(instant)
    }

    /// What is left, or nothing once the deadline has passed.
    pub fn left(self) -> Option<Duration> {
        Some(self.0.checked_duration_since(Instant::now())?).filter(|left| !left.is_zero())
    }

    /// Waits one poll, cut to what is left. False once the deadline has passed.
    async fn pause(self) -> bool {
        match self.left() {
            Some(left) => {
                tokio::time::sleep(POLL.min(left)).await;
                true
            }
            None => false,
        }
    }
}

/// An observation within what is left of `deadline`, or nothing once it has passed. A probe that the deadline cut
/// short is the deadline, not a stalled server.
#[cfg(unix)]
pub async fn observe_by(home: &Home, deadline: Deadline) -> Result<Option<Observed>, LifeError> {
    let Some(left) = deadline.left() else {
        return Ok(None);
    };
    let limit = PROBE_LIMIT.min(left);
    match observe_within(home, limit).await? {
        Observed::Stalled if limit < PROBE_LIMIT => Ok(None),
        observed => Ok(Some(observed)),
    }
}

/// A build before the socket never makes one, so only a missing socket can hide a legacy server. A lock that cannot
/// be looked at is a path the command cannot use, as a socket that cannot be reached is.
#[cfg(unix)]
fn behind_the_socket(home: &Home, reason: Option<String>, no_socket: bool) -> Observed {
    match home.held() {
        Ok(false) => Observed::Free,
        Ok(true) if no_socket && home.legacy_hint() => Observed::Legacy,
        Ok(true) => Observed::Starting { reason },
        Err(error) if cannot_write(&error) => match home.unusable(error, Attempt::Start) {
            LifeError::Unreachable { reason } => Observed::Unreachable(reason),
            other => Observed::Unreachable(other.to_string()),
        },
        Err(error) => Observed::Unreachable(format!(
            "{} cannot be used: {error}",
            home.prepared.lock().display()
        )),
    }
}

#[cfg(unix)]
pub fn lists_this_control(identity: &Identity) -> bool {
    identity
        .controls
        .as_deref()
        .is_some_and(|controls| controls.contains(&CONTROL))
}

#[cfg(unix)]
fn other_interface(identity: &Identity) -> Option<LifeError> {
    let theirs = match &identity.interface {
        Some(theirs) if *theirs == interface() => return None,
        Some(theirs) => TheirInterface::Named(theirs.clone()),
        None => TheirInterface::FromABuildBeforeInterfaces,
    };
    Some(LifeError::OtherInterface {
        pid: identity.pid,
        theirs,
    })
}

/// The outcome of a row in which nothing may be started or stopped.
#[cfg(unix)]
fn refusal(home: &Home, observed: Observed, attempt: Attempt) -> LifeError {
    let socket = home.prepared.socket();
    match observed {
        Observed::Stalled => LifeError::Stalled {
            socket,
            log: home.log(),
            pid: home.pid_hint(),
            attempt,
        },
        Observed::Malformed(reason) => LifeError::Malformed {
            socket,
            reason,
            attempt,
        },
        Observed::OtherDatabase(identity) => LifeError::ServesOtherDatabase {
            socket,
            database: identity.database,
            attempt,
        },
        Observed::Newer(identity) => LifeError::Newer {
            control: identity.control.unwrap_or_default(),
            attempt,
        },
        Observed::Unreachable(reason) => match attempt {
            Attempt::Start => LifeError::Unreachable { reason },
            Attempt::Stop => LifeError::UnreachableToStop { reason },
        },
        Observed::Serving { .. }
        | Observed::Starting { .. }
        | Observed::Legacy
        | Observed::Free => LifeError::Io(std::io::Error::other(format!(
            "{observed:?} is not a refusal"
        ))),
    }
}

/// Where a command reaches the server of a home: on Unix its socket, and never TCP; on Windows, which has no socket
/// yet (QUA-61), the TCP address of its runtime file.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub base: String,
    pub socket: Option<PathBuf>,
}

impl Endpoint {
    #[cfg(not(unix))]
    pub fn over_tcp(runtime: &RuntimeFile) -> Endpoint {
        Endpoint {
            base: runtime.url.clone(),
            socket: None,
        }
    }
}

pub async fn ensure(home: &Home) -> Result<Endpoint, LifeError> {
    ensure_within(home, START_LIMIT).await
}

/// Finds this home's server, or starts one. A start happens only under `start.lock`, after a second observation that
/// finds the home free; the lock is held until the child answers or exits. A stalled, malformed, foreign or newer
/// server is never replaced. The only process this may end is its own child, through its `Child` handle, when the
/// child is not ready by the deadline.
#[cfg(unix)]
async fn ensure_within(home: &Home, limit: Duration) -> Result<Endpoint, LifeError> {
    let deadline = Deadline::after(limit);
    let mut start_lock: Option<File> = None;
    let mut child: Option<OwnedChild> = None;
    loop {
        let Some(observed) = observe_by(home, deadline).await? else {
            return Err(not_ready_by_the_deadline(
                home,
                child.as_ref(),
                start_lock.is_some(),
            ));
        };
        match observed {
            Observed::Serving { identity } => {
                if let Some(child) = child.take()
                    && child.id() == identity.pid
                {
                    child.hand_off();
                }
                return match other_interface(&identity) {
                    Some(refused) => Err(refused),
                    None => Ok(home.endpoint()),
                };
            }
            Observed::Legacy => {
                return Err(legacy::refusal(home, legacy::examine(home), Attempt::Start));
            }
            Observed::Free if start_lock.is_none() => {
                let lock = home
                    .open_lock(&home.start_lock())
                    .map_err(|error| home.unusable(error, Attempt::Start))?;
                match lock.try_lock() {
                    Ok(()) => {
                        start_lock = Some(lock);
                        continue;
                    }
                    Err(TryLockError::WouldBlock) => {}
                    Err(TryLockError::Error(error)) => return Err(error.into()),
                }
            }
            observed @ (Observed::Free | Observed::Starting { .. }) => match child.as_mut() {
                Some(running) => {
                    if let Some(status) = running.try_wait()? {
                        return Err(LifeError::Exited {
                            status,
                            log: home.log(),
                            tail: log_tail(&home.log()),
                        });
                    }
                }
                None if matches!(observed, Observed::Free) && deadline.left().is_some() => {
                    child = Some(
                        OwnedChild::spawn(home)
                            .map_err(|error| home.unusable(error, Attempt::Start))?,
                    );
                }
                None => {}
            },
            refused => return Err(refusal(home, refused, Attempt::Start)),
        }
        if !deadline.pause().await {
            return Err(not_ready_by_the_deadline(
                home,
                child.as_ref(),
                start_lock.is_some(),
            ));
        }
    }
}

/// Another starter that never lets go of `start.lock` is `NoAnswer`; anything else that is not ready is `Hung`.
#[cfg(unix)]
fn not_ready_by_the_deadline(
    home: &Home,
    child: Option<&OwnedChild>,
    holds_start_lock: bool,
) -> LifeError {
    if child.is_none() && !holds_start_lock && !home.held().unwrap_or(true) {
        return LifeError::NoAnswer {
            pid: home.pid_hint(),
            log: home.log(),
        };
    }
    LifeError::Hung {
        hang: Hang::NotReady,
        home: home.prepared.folder().to_path_buf(),
        log: home.log(),
        pid: child.map(OwnedChild::id).or_else(|| home.pid_hint()),
        attempt: Attempt::Start,
    }
}

#[cfg(not(unix))]
async fn ensure_within(home: &Home, limit: Duration) -> Result<Endpoint, LifeError> {
    over_tcp::ensure(home, limit)
        .await
        .map(|runtime| Endpoint::over_tcp(&runtime))
}

/// `start.lock`, held: no command starts a server while its holder starts, stops or hands off one. It carries the
/// deadline of the command that took it, which the server's start then uses.
pub struct StartLockHeld {
    _start_lock: File,
    deadline: Deadline,
}

impl StartLockHeld {
    /// What is left of the command's deadline for its own server's start.
    pub fn left(&self) -> Duration {
        self.deadline.left().unwrap_or_default()
    }
}

/// `quadratique serve` holds `start.lock` while its server starts, as every start does; `quadratique upgrade` holds it
/// from its last look through the rename and the stop.
pub async fn reserve_start(home: &Home) -> Result<StartLockHeld, LifeError> {
    let deadline = Deadline::after(START_LIMIT);
    let start_lock = await_start_lock(home, deadline, Attempt::Start).await?;
    Ok(StartLockHeld {
        _start_lock: start_lock,
        deadline,
    })
}

/// Stops the server of this home, of any build that serves this control version, and keeps `start.lock` until the
/// caller's own server is ready, within one deadline for the whole command. A server that does not let go by the
/// deadline ends the take-over: nothing is served.
#[cfg(unix)]
pub async fn take_over(home: &Home) -> Result<StartLockHeld, LifeError> {
    let deadline = Deadline::after(START_LIMIT);
    let proof = match observe_by(home, deadline).await? {
        Some(Observed::Legacy) => Some(legacy::proof_or_refusal(home, Attempt::Start)?),
        _ => None,
    };
    let start_lock = await_start_lock(home, deadline, Attempt::Start).await?;
    if let Stopped::Closing { pid, .. } =
        stop_holding(home, deadline, Attempt::Start, None, proof).await?
    {
        return Err(LifeError::Hung {
            hang: Hang::NotStopped,
            home: home.prepared.folder().to_path_buf(),
            log: home.log(),
            pid,
            attempt: Attempt::Start,
        });
    }
    Ok(StartLockHeld {
        _start_lock: start_lock,
        deadline,
    })
}

#[cfg(not(unix))]
pub async fn take_over(home: &Home) -> Result<StartLockHeld, LifeError> {
    let deadline = Deadline::after(START_LIMIT);
    let start_lock = await_start_lock(home, deadline, Attempt::Start).await?;
    over_tcp::stop(home, deadline.left().unwrap_or_default(), false).await?;
    Ok(StartLockHeld {
        _start_lock: start_lock,
        deadline,
    })
}

/// Tries the lock only while the deadline has not passed.
async fn await_start_lock(
    home: &Home,
    deadline: Deadline,
    attempt: Attempt,
) -> Result<File, LifeError> {
    let start_lock = home
        .open_lock(&home.start_lock())
        .map_err(|error| home.unusable(error, attempt))?;
    let expired = || LifeError::NoAnswer {
        pid: home.pid_hint(),
        log: home.log(),
    };
    loop {
        if deadline.left().is_none() {
            return Err(expired());
        }
        match start_lock.try_lock() {
            Ok(()) => return Ok(start_lock),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        if !deadline.pause().await {
            return Err(expired());
        }
    }
}

#[derive(Debug)]
pub enum Stopped {
    NotRunning,
    Ended {
        pid: u32,
        generation: Option<String>,
    },
    /// The server acknowledged the stop, or was signalled, and still holds the home at the deadline.
    Closing {
        pid: Option<u32>,
        generation: Option<String>,
    },
}

pub async fn stop(home: &Home) -> Result<Stopped, LifeError> {
    stop_within(home, STOP_LIMIT).await
}

/// A home that is free, or a server that may not be stopped, is answered without `start.lock`: a server from before
/// the socket without its proof too. Otherwise the stop takes it, then observes again, and keeps it until the server
/// has let go of the home. The generation it first saw is the one it stops; one that becomes ready while it waits for
/// the lock is stopped only if the first observation saw none.
#[cfg(unix)]
async fn stop_within(home: &Home, limit: Duration) -> Result<Stopped, LifeError> {
    let deadline = Deadline::after(limit);
    let Some(first) = observe_by(home, deadline).await? else {
        return Err(not_stopped_by_the_deadline(home, Attempt::Stop));
    };
    let mut proof = None;
    let meant = match first {
        Observed::Free => return Ok(Stopped::NotRunning),
        Observed::Legacy => {
            proof = Some(legacy::proof_or_refusal(home, Attempt::Stop)?);
            None
        }
        Observed::Starting { .. } => None,
        Observed::Serving { identity, .. } => identity.generation,
        Observed::Newer(identity) if lists_this_control(&identity) => identity.generation,
        refused => return Err(refusal(home, refused, Attempt::Stop)),
    };
    let _start_lock = await_start_lock(home, deadline, Attempt::Stop).await?;
    stop_holding(home, deadline, Attempt::Stop, meant, proof).await
}

/// Stops generation `generation` of this home's server, and no other: `quadratique upgrade` saw it before it replaced
/// the binary, with `start.lock` held (`held`) since before it looked. Within `deadline`. Another generation that serves
/// now is refused (`409`, or seen before the request), a server from before the socket is never signalled, and a free
/// home is `NotRunning`.
#[cfg(unix)]
pub async fn stop_generation(
    home: &Home,
    _held: &StartLockHeld,
    generation: &str,
    deadline: Deadline,
) -> Result<Stopped, LifeError> {
    stop_holding(
        home,
        deadline,
        Attempt::Stop,
        Some(generation.to_owned()),
        None,
    )
    .await
}

/// Why nothing may be stopped in a row that refuses a stop, as `server stop` says it.
#[cfg(unix)]
pub fn refused_to_stop(home: &Home, observed: Observed) -> String {
    refusal(home, observed, Attempt::Stop).to_string()
}

#[cfg(not(unix))]
async fn stop_within(home: &Home, limit: Duration) -> Result<Stopped, LifeError> {
    over_tcp::stop(home, limit, false)
        .await
        .map(|stopped| match stopped {
            over_tcp::Stop::Done(stopped) => stopped,
            over_tcp::Stop::NotLegacy => Stopped::NotRunning,
        })
}

#[cfg(unix)]
fn not_stopped_by_the_deadline(home: &Home, attempt: Attempt) -> LifeError {
    LifeError::Hung {
        hang: Hang::NotReady,
        home: home.prepared.folder().to_path_buf(),
        log: home.log(),
        pid: home.pid_hint(),
        attempt,
    }
}

/// Called with `start.lock` held. Asks the serving generation to stop, once, then waits for the home's lock to be
/// free: the stopped server's successor can only be a server this command's caller starts.
/// - `meant`: the generation the command saw before it waited for `start.lock`. Another generation serving now is
///   refused, and nothing is stopped.
/// - A take-over (`Start`) refuses a newer server even when it lists this control version, because it would replace
///   it with an older one.
/// - A server from before the socket is stopped by the legacy path only with its proof (`proof`, when the caller found
///   it before the reservation), and only while the home still shows that row; otherwise it hands back, and from then
///   on the socket alone decides. A stop that meant a generation stops no such server.
#[cfg(unix)]
async fn stop_holding(
    home: &Home,
    deadline: Deadline,
    attempt: Attempt,
    meant: Option<String>,
    mut proof: Option<legacy::Proof>,
) -> Result<Stopped, LifeError> {
    let mut asked: Option<Identity> = None;
    let mut legacy_possible = true;
    loop {
        let Some(observed) = observe_by(home, deadline).await? else {
            return closing_at_the_deadline(home, asked, attempt);
        };
        let serving = match observed {
            Observed::Free => {
                return Ok(match asked {
                    Some(identity) => Stopped::Ended {
                        pid: identity.pid,
                        generation: identity.generation,
                    },
                    None => Stopped::NotRunning,
                });
            }
            Observed::Legacy if legacy_possible && asked.is_none() => {
                if let Some(meant) = meant {
                    return Err(LifeError::OtherGeneration {
                        asked: meant,
                        serving: Some("a server from before the socket".to_owned()),
                    });
                }
                match legacy::stop(home, deadline, attempt, proof.take()).await? {
                    legacy::Stop::Done(stopped) => return Ok(stopped),
                    legacy::Stop::NotLegacy => {
                        legacy_possible = false;
                        None
                    }
                }
            }
            Observed::Legacy | Observed::Starting { .. } => None,
            Observed::Serving { identity, .. } => Some(identity),
            Observed::Newer(identity)
                if attempt == Attempt::Stop && lists_this_control(&identity) =>
            {
                Some(identity)
            }
            refused => return Err(refusal(home, refused, attempt)),
        };
        if let Some(identity) = serving {
            let generation = identity.generation.clone().unwrap_or_default();
            if let Some(stopped) = asked.as_ref().and_then(|asked| asked.generation.clone()) {
                if stopped != generation {
                    return Ok(Stopped::Ended {
                        pid: asked.as_ref().map_or(0, |asked| asked.pid),
                        generation: Some(stopped),
                    });
                }
            } else if let Some(meant) = meant.as_ref().filter(|meant| **meant != generation) {
                return Err(LifeError::OtherGeneration {
                    asked: meant.clone(),
                    serving: Some(generation),
                });
            } else {
                let Some(left) = deadline.left() else {
                    return closing_at_the_deadline(home, asked, attempt);
                };
                let answer = stop_over_socket(&home.prepared, &generation, PROBE_LIMIT.min(left));
                match answer.await {
                    Ok(StopAnswer::Accepted) => asked = Some(identity),
                    Ok(StopAnswer::OtherGeneration(serving)) => {
                        return Err(LifeError::OtherGeneration {
                            asked: generation,
                            serving,
                        });
                    }
                    Ok(StopAnswer::Refused { status }) => {
                        return Err(LifeError::CannotStop {
                            status,
                            pid: identity.pid,
                        });
                    }
                    Ok(StopAnswer::Unconfirmed(reason)) => {
                        return Err(LifeError::StopUnconfirmed {
                            generation,
                            reason,
                            lock: match home.held() {
                                Ok(true) => "held: the server may still be stopping".to_owned(),
                                Ok(false) => "free: no server holds the home now".to_owned(),
                                Err(error) => format!("unknown ({error})"),
                            },
                        });
                    }
                    Err(Unanswered::NotListening(_)) => {}
                    Err(Unanswered::Stalled) if deadline.left().is_none() => {
                        return closing_at_the_deadline(home, asked, attempt);
                    }
                    Err(Unanswered::Stalled) => {
                        return Err(refusal(home, Observed::Stalled, attempt));
                    }
                    Err(Unanswered::NotPermitted(error) | Unanswered::PathError(error)) => {
                        return Err(refusal(
                            home,
                            Observed::Unreachable(format!(
                                "{} cannot be reached: {error}",
                                home.prepared.socket().display()
                            )),
                            attempt,
                        ));
                    }
                    Err(Unanswered::NotReady(why) | Unanswered::Malformed(why)) => {
                        return Err(refusal(home, Observed::Malformed(why), attempt));
                    }
                    Err(Unanswered::OwnerOnly) => {
                        return Err(refusal(
                            home,
                            Observed::Malformed(
                                "the socket answered as a TCP listener does".to_owned(),
                            ),
                            attempt,
                        ));
                    }
                }
            }
        }
        if !deadline.pause().await {
            return closing_at_the_deadline(home, asked, attempt);
        }
    }
}

/// An acknowledged stop still holding the home is `Closing`; a server that never became ready to be stopped is
/// `Hung`.
#[cfg(unix)]
fn closing_at_the_deadline(
    home: &Home,
    asked: Option<Identity>,
    attempt: Attempt,
) -> Result<Stopped, LifeError> {
    match asked {
        Some(identity) => Ok(Stopped::Closing {
            pid: Some(identity.pid),
            generation: identity.generation,
        }),
        None => Err(not_stopped_by_the_deadline(home, attempt)),
    }
}

struct OwnedChild {
    child: Option<Child>,
}

/// The server that a command starts gets the checked home, by its resolved path, never the path as typed.
fn serve_command(home: &Home) -> std::io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("serve")
        .env("QUADRATIQUE_HOME", home.prepared.folder())
        .env(STARTED_VARIABLE, ON_DEMAND)
        .current_dir(home.prepared.folder());
    Ok(command)
}

impl OwnedChild {
    fn spawn(home: &Home) -> std::io::Result<OwnedChild> {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.log())?;
        let mut command = serve_command(home)?;
        let child = detach(&mut command)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        Ok(OwnedChild { child: Some(child) })
    }

    fn id(&self) -> u32 {
        self.child.as_ref().map_or(0, Child::id)
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match &mut self.child {
            Some(child) => child.try_wait(),
            None => Ok(None),
        }
    }

    fn hand_off(mut self) {
        if let Some(mut child) = self.child.take() {
            std::thread::spawn(move || child.wait());
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn log_tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let start = text
        .char_indices()
        .rev()
        .nth(LOG_TAIL)
        .map_or(0, |(index, _)| index);
    text[start..].trim().to_owned()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use quadratique_server::server::Paths;

    use super::{Home, LifeError, ensure_within};

    const HANG_GUARD: Duration = Duration::from_secs(20);

    /// The home policy refuses a home that a group or other accounts may use, and `tempfile` makes 0755 folders.
    fn private_home() -> tempfile::TempDir {
        let builder = tempfile::Builder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::PermissionsExt;
            let mut builder = builder;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
            builder
        };
        builder.tempdir().expect("a temporary home")
    }

    /// The checked home is named by its resolved path from then on: a link on the typed path that changes later
    /// changes nothing that this command opens, and the server it starts gets the resolved path.
    #[cfg(unix)]
    #[test]
    fn every_file_and_the_started_server_use_the_resolved_home_not_the_path_as_typed() {
        let base = private_home();
        let real = base.path().join("real");
        let other = base.path().join("other");
        std::fs::create_dir(&other).expect("another folder");
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("a link to the home");
        let typed = link.join("quadratique");
        std::fs::create_dir(&real).expect("the link's target");

        let home = Home::new(Paths::in_home(&typed).expect("a home")).expect("a home");
        std::fs::remove_file(&link).expect("the link goes");
        std::os::unix::fs::symlink(&other, &link).expect("the link names another folder");

        let resolved = base
            .path()
            .canonicalize()
            .expect("a real path")
            .join("real/quadratique");
        assert_eq!(home.prepared().folder(), resolved);
        for file in [
            home.prepared().database(),
            home.prepared().runtime(),
            home.prepared().lock(),
            home.start_lock(),
            home.log(),
            home.prepared().socket(),
        ] {
            assert!(file.starts_with(&resolved), "{}", file.display());
        }
        let command = super::serve_command(&home).expect("the server's command");
        let environment: Vec<_> = command.get_envs().collect();
        assert!(
            environment.contains(&(
                std::ffi::OsStr::new("QUADRATIQUE_HOME"),
                Some(resolved.as_os_str())
            )),
            "{environment:?}"
        );
        assert_eq!(command.get_current_dir(), Some(resolved.as_path()));
        assert!(!other.join("quadratique").exists());
    }

    #[tokio::test]
    async fn a_start_lock_that_another_starter_never_releases_ends_at_the_deadline() {
        let directory = private_home();
        let home = Home::new(Paths::in_home(directory.path()).expect("a home")).expect("a home");
        let other_starter = home.open_lock(&home.start_lock()).expect("the start lock");
        other_starter
            .lock()
            .expect("the other starter holds the start lock");

        let started =
            tokio::time::timeout(HANG_GUARD, ensure_within(&home, Duration::from_millis(200)))
                .await;

        drop(other_starter);
        assert!(
            matches!(started, Ok(Err(LifeError::NoAnswer { .. }))),
            "the start did not end at its deadline: {started:?}"
        );
    }

    /// A held home with no socket and no runtime file is the row "starting": the stop waits for it to answer or to
    /// let go, and at the deadline it ends with the recovery text, having signalled nothing.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_stop_of_a_home_that_stays_held_without_an_answer_ends_at_its_deadline_and_says_how_to_recover()
     {
        use super::stop_within;

        let directory = private_home();
        let home = Home::new(Paths::in_home(directory.path()).expect("a home")).expect("a home");
        let closing_server = home
            .open_lock(&home.prepared.lock())
            .expect("the home's lock");
        closing_server
            .lock()
            .expect("a server closing its store holds the home");

        let stopped =
            tokio::time::timeout(HANG_GUARD, stop_within(&home, Duration::from_millis(200))).await;

        drop(closing_server);
        let Ok(Err(error @ LifeError::Hung { .. })) = stopped else {
            panic!("the stop did not end at its deadline");
        };
        let message = error.to_string();
        assert!(
            message.contains("Nothing was stopped.") && message.contains("Ctrl-C"),
            "{message}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_that_meets_a_held_home_without_a_socket_waits_then_ends_at_its_deadline_having_started_nothing()
     {
        let directory = private_home();
        let home = Home::new(Paths::in_home(directory.path()).expect("a home")).expect("a home");
        let starting_server = home
            .open_lock(&home.prepared.lock())
            .expect("the home's lock");
        starting_server
            .lock()
            .expect("a server that has not bound its socket holds the home");

        let started =
            tokio::time::timeout(HANG_GUARD, ensure_within(&home, Duration::from_millis(200)))
                .await;

        drop(starting_server);
        let Ok(Err(error @ LifeError::Hung { .. })) = started else {
            panic!("the start did not end at its deadline: {started:?}");
        };
        assert!(
            error.to_string().contains("Nothing was started."),
            "{error}"
        );
        assert!(!home.log().exists(), "a server was spawned");
        assert!(!home.start_lock().exists(), "a start was reserved");
    }

    /// `EACCES`, `EPERM` and path errors on the socket cannot be made for this account's own socket without root; the
    /// row's outcome is this mapping, which every operation uses.
    #[cfg(unix)]
    #[test]
    fn an_unreachable_socket_refuses_a_start_and_a_stop_each_with_its_own_outcome() {
        use super::{Attempt, Observed, refusal};

        let directory = private_home();
        let home = Home::new(Paths::in_home(directory.path()).expect("a home")).expect("a home");
        let reason = || Observed::Unreachable("connecting is not permitted".to_owned());

        let start = refusal(&home, reason(), Attempt::Start);
        let stop = refusal(&home, reason(), Attempt::Stop);

        assert!(matches!(start, LifeError::Unreachable { .. }), "{start}");
        assert!(
            matches!(stop, LifeError::UnreachableToStop { .. }),
            "{stop}"
        );
        assert!(stop.to_string().ends_with("Nothing was stopped."), "{stop}");
    }

    /// One deadline per operation: once it has passed, nothing new begins. A free home is not started in.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_start_whose_deadline_has_passed_reserves_and_spawns_nothing() {
        let directory = private_home();
        let home = Home::new(Paths::in_home(directory.path()).expect("a home")).expect("a home");

        let started = tokio::time::timeout(HANG_GUARD, ensure_within(&home, Duration::ZERO)).await;

        assert!(
            matches!(
                started,
                Ok(Err(LifeError::NoAnswer { .. } | LifeError::Hung { .. }))
            ),
            "the start went on past its deadline: {started:?}"
        );
        assert!(!home.start_lock().exists(), "a start was reserved");
        assert!(!home.log().exists(), "a server was spawned");
    }

    /// A probe gets what is left of the operation's deadline, not a probe limit of its own: a stop of a stalled socket
    /// with 200 ms left ends at its deadline, not five seconds later as "stalled".
    #[cfg(unix)]
    #[tokio::test]
    async fn a_probe_is_cut_to_what_is_left_of_the_deadline() {
        use std::os::unix::fs::PermissionsExt;

        use super::stop_within;

        let directory = private_home();
        let home = Home::new(Paths::in_home(directory.path()).expect("a home")).expect("a home");
        let holder = home
            .open_lock(&home.prepared.lock())
            .expect("the home's lock");
        holder.lock().expect("a server holds the home");
        let socket = home.prepared.socket();
        let _never_answers = std::os::unix::net::UnixListener::bind(&socket).expect("a socket");
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
            .expect("mode 0600");
        let limit = Duration::from_millis(200);

        let started = std::time::Instant::now();
        let stopped = tokio::time::timeout(HANG_GUARD, stop_within(&home, limit)).await;

        assert!(
            matches!(stopped, Ok(Err(LifeError::Hung { .. }))),
            "the stop did not end at its deadline: {stopped:?}"
        );
        assert!(
            started.elapsed() < quadratique_server::server::PROBE_LIMIT,
            "the probe outlived the deadline: {:?}",
            started.elapsed()
        );
    }
}
