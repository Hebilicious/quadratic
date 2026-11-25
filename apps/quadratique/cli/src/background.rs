use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use quadratique_server::app::interface;
use quadratique_server::home::{self, Prepared};
#[cfg(unix)]
use quadratique_server::server::{
    CONTROL, Identity, PROBE_LIMIT, StopAnswer, identity_over_socket_within, stop_over_socket,
};
use quadratique_server::server::{Paths, RuntimeFile, START_LIMIT, Unanswered, identity};
use quadratique_server::store::canonical_folder;

use crate::process::detach;
use crate::route::McpTool;

mod legacy;

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
    /// The server does not serve the stop request.
    CannotStop {
        reason: String,
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
            LifeError::CannotStop { reason } => {
                write!(
                    formatter,
                    "the server of this home cannot be stopped from here: {reason}. Nothing was stopped."
                )
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
    let check = match pid {
        Some(pid) => format!(
            "its process number was last given as {pid}, unverified: check it with `ps -p {pid} -o args` before you end it"
        ),
        None => "find its process first".to_owned(),
    };
    format!(
        "To recover: press Ctrl-C in the terminal that runs `quadratique serve`, stop it with the supervisor that started it, or end the process ({check})."
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
    pub fn new(paths: Paths) -> Result<Home, String> {
        let prepared = home::prepare(&paths).map_err(|refusal| match refusal.denied() {
            Some(error) => LifeError::Unreachable {
                reason: format!(
                    "this process cannot use {}: {error}; a sandbox may be blocking it",
                    paths.database.parent().unwrap_or(Path::new(".")).display()
                ),
            }
            .to_string(),
            None => refusal.to_string(),
        })?;
        Ok(Home { prepared })
    }

    pub fn prepared(&self) -> &Prepared {
        &self.prepared
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

    /// A process that may not write the home can still ask: `flock` takes a lock on a read-only descriptor, and a
    /// missing lock file means that no server ever held the home.
    pub fn held(&self) -> std::io::Result<bool> {
        let lock = match self.open_lock(&self.prepared.lock()) {
            Ok(lock) => lock,
            Err(error) if cannot_write(&error) => match File::open(self.prepared.lock()) {
                Ok(lock) => lock,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error),
            },
            Err(error) => return Err(error),
        };
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
pub async fn on_socket(home: &Home) -> OnSocket {
    on_socket_within(home, PROBE_LIMIT).await
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
    match identity.control {
        None => return OnSocket::Malformed("the identity has no control version".to_owned()),
        Some(control) if control != CONTROL => return OnSocket::Newer(identity),
        Some(_) => {}
    }
    let generation_is_valid = identity.generation.as_deref().is_some_and(|generation| {
        generation.len() == 32 && generation.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    if !generation_is_valid || identity.started.is_none() || identity.url.is_none() {
        return OnSocket::Malformed(
            "the identity lacks a valid generation, its start or its url".to_owned(),
        );
    }
    if identity.database != home.database() {
        return OnSocket::OtherDatabase(identity);
    }
    OnSocket::Serving(identity)
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
        runtime: RuntimeFile,
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
    Ok(match on_socket(home).await {
        OnSocket::Serving(identity) => match runtime_of(&identity) {
            Some(runtime) => Observed::Serving { identity, runtime },
            None => Observed::Malformed(
                "the identity's url names no loopback address of the command line".to_owned(),
            ),
        },
        OnSocket::OtherDatabase(identity) => Observed::OtherDatabase(identity),
        OnSocket::Newer(identity) => Observed::Newer(identity),
        OnSocket::Malformed(why) => Observed::Malformed(why),
        OnSocket::Stalled => Observed::Stalled,
        OnSocket::Denied(reason) | OnSocket::PathError(reason) => Observed::Unreachable(reason),
        OnSocket::NotReady(why) => behind_the_socket(home, Some(why), false)?,
        OnSocket::Absent => behind_the_socket(home, None, true)?,
    })
}

/// A build before the socket never makes one, so only a missing socket can hide a legacy server.
#[cfg(unix)]
fn behind_the_socket(
    home: &Home,
    reason: Option<String>,
    no_socket: bool,
) -> Result<Observed, LifeError> {
    match home.held() {
        Ok(false) => Ok(Observed::Free),
        Ok(true) if no_socket && home.legacy_hint() => Ok(Observed::Legacy),
        Ok(true) => Ok(Observed::Starting { reason }),
        Err(error) => match home.unusable(error, Attempt::Start) {
            LifeError::Unreachable { reason } => Ok(Observed::Unreachable(reason)),
            other => Err(other),
        },
    }
}

/// The TCP address of the command line's routes, which the generated client still uses (QUA-53 moves it to the
/// socket), from the identity's `url`: `http://127.0.0.1:<port>/cli`.
#[cfg(unix)]
fn runtime_of(identity: &Identity) -> Option<RuntimeFile> {
    let url = identity.url.as_deref()?;
    let (authority, path) = url.strip_prefix("http://")?.split_once('/')?;
    let address: std::net::SocketAddr = authority.parse().ok()?;
    (address.ip().is_loopback() && path == "cli").then(|| RuntimeFile {
        pid: identity.pid,
        address,
        url: url.to_owned(),
        database: identity.database.clone(),
        control: identity.control,
    })
}

#[cfg(unix)]
fn lists_this_control(identity: &Identity) -> bool {
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

pub async fn ensure(home: &Home) -> Result<RuntimeFile, LifeError> {
    ensure_within(home, START_LIMIT).await
}

/// Finds this home's server, or starts one. A start happens only under `start.lock`, after a second observation that
/// finds the home free; the lock is held until the child answers or exits. A stalled, malformed, foreign or newer
/// server is never replaced. The only process this may end is its own child, through its `Child` handle, when the
/// child is not ready by the deadline.
#[cfg(unix)]
async fn ensure_within(home: &Home, limit: Duration) -> Result<RuntimeFile, LifeError> {
    let deadline = Instant::now() + limit;
    let mut start_lock: Option<File> = None;
    let mut child: Option<OwnedChild> = None;
    loop {
        match observe(home).await? {
            Observed::Serving { identity, runtime } => {
                if let Some(child) = child.take()
                    && child.id() == identity.pid
                {
                    child.hand_off();
                }
                return match other_interface(&identity) {
                    Some(refused) => Err(refused),
                    None => Ok(runtime),
                };
            }
            Observed::Legacy => {
                drop(child);
                drop(start_lock);
                return legacy::ensure(home, deadline.saturating_duration_since(Instant::now()))
                    .await;
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
                None if matches!(observed, Observed::Free) => {
                    child = Some(
                        OwnedChild::spawn(home)
                            .map_err(|error| home.unusable(error, Attempt::Start))?,
                    );
                }
                None => {}
            },
            refused => return Err(refusal(home, refused, Attempt::Start)),
        }
        if Instant::now() > deadline {
            return Err(match (&child, &start_lock) {
                (None, None) if !home.held().unwrap_or(true) => LifeError::NoAnswer {
                    pid: home.pid_hint(),
                    log: home.log(),
                },
                _ => LifeError::Hung {
                    hang: Hang::NotReady,
                    home: home.prepared.folder().to_path_buf(),
                    log: home.log(),
                    pid: child
                        .as_ref()
                        .map(OwnedChild::id)
                        .or_else(|| home.pid_hint()),
                    attempt: Attempt::Start,
                },
            });
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(not(unix))]
async fn ensure_within(home: &Home, limit: Duration) -> Result<RuntimeFile, LifeError> {
    legacy::ensure(home, limit).await
}

/// `start.lock`, held: no command starts a server while its holder starts, stops or hands off one.
pub struct StartLockHeld {
    _start_lock: File,
}

/// `quadratique serve` holds `start.lock` while its server starts, as every start does.
pub async fn reserve_start(home: &Home) -> Result<StartLockHeld, LifeError> {
    let start_lock = await_start_lock(home, Instant::now() + START_LIMIT, Attempt::Start).await?;
    Ok(StartLockHeld {
        _start_lock: start_lock,
    })
}

/// Stops the server of this home, of any build that serves this control version, and keeps `start.lock` until the
/// caller's own server is ready. A server that does not let go by the deadline ends the take-over: nothing is served.
#[cfg(unix)]
pub async fn take_over(home: &Home) -> Result<StartLockHeld, LifeError> {
    let deadline = Instant::now() + START_LIMIT;
    let start_lock = await_start_lock(home, deadline, Attempt::Start).await?;
    if let Stopped::Closing { pid, .. } = stop_holding(home, deadline, Attempt::Start).await? {
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
    })
}

#[cfg(not(unix))]
pub async fn take_over(home: &Home) -> Result<StartLockHeld, LifeError> {
    let start_lock = await_start_lock(home, Instant::now() + START_LIMIT, Attempt::Start).await?;
    legacy::stop(home, STOP_LIMIT).await?;
    Ok(StartLockHeld {
        _start_lock: start_lock,
    })
}

async fn await_start_lock(
    home: &Home,
    deadline: Instant,
    attempt: Attempt,
) -> Result<File, LifeError> {
    let start_lock = home
        .open_lock(&home.start_lock())
        .map_err(|error| home.unusable(error, attempt))?;
    loop {
        match start_lock.try_lock() {
            Ok(()) => return Ok(start_lock),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        if Instant::now() > deadline {
            return Err(LifeError::NoAnswer {
                pid: home.pid_hint(),
                log: home.log(),
            });
        }
        tokio::time::sleep(POLL).await;
    }
}

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

/// A home that is free, or a server that may not be stopped, is answered without `start.lock`. Otherwise the stop
/// takes it, observes again, and keeps it until the server has let go of the home.
#[cfg(unix)]
async fn stop_within(home: &Home, limit: Duration) -> Result<Stopped, LifeError> {
    let deadline = Instant::now() + limit;
    match observe(home).await? {
        Observed::Free => return Ok(Stopped::NotRunning),
        Observed::Legacy => return legacy::stop(home, limit).await,
        Observed::Serving { .. } | Observed::Starting { .. } => {}
        Observed::Newer(identity) if lists_this_control(&identity) => {}
        refused => return Err(refusal(home, refused, Attempt::Stop)),
    }
    let _start_lock = await_start_lock(home, deadline, Attempt::Stop).await?;
    stop_holding(home, deadline, Attempt::Stop).await
}

#[cfg(not(unix))]
async fn stop_within(home: &Home, limit: Duration) -> Result<Stopped, LifeError> {
    legacy::stop(home, limit).await
}

/// Called with `start.lock` held. Asks the serving generation to stop, once, then waits for the home's lock to be
/// free: the stopped server's successor can only be a server this command's caller starts. A take-over (`Start`)
/// refuses a newer server even when it lists this control version, because it would replace it with an older one.
#[cfg(unix)]
async fn stop_holding(
    home: &Home,
    deadline: Instant,
    attempt: Attempt,
) -> Result<Stopped, LifeError> {
    let mut asked: Option<Identity> = None;
    loop {
        let serving = match observe(home).await? {
            Observed::Free => {
                return Ok(match asked {
                    Some(identity) => Stopped::Ended {
                        pid: identity.pid,
                        generation: identity.generation,
                    },
                    None => Stopped::NotRunning,
                });
            }
            Observed::Legacy => {
                return legacy::stop(home, deadline.saturating_duration_since(Instant::now()))
                    .await;
            }
            Observed::Serving { identity, .. } => Some(identity),
            Observed::Newer(identity)
                if attempt == Attempt::Stop && lists_this_control(&identity) =>
            {
                Some(identity)
            }
            Observed::Starting { .. } => None,
            refused => return Err(refusal(home, refused, attempt)),
        };
        if let Some(identity) = serving {
            let generation = identity.generation.clone().unwrap_or_default();
            match asked.as_ref().and_then(|asked| asked.generation.clone()) {
                Some(stopped) if stopped != generation => {
                    return Ok(Stopped::Ended {
                        pid: asked.as_ref().map_or(0, |asked| asked.pid),
                        generation: Some(stopped),
                    });
                }
                Some(_) => {}
                None => match stop_over_socket(&home.prepared, &generation).await {
                    Ok(StopAnswer::Accepted) => asked = Some(identity),
                    Ok(StopAnswer::OtherGeneration(serving)) => {
                        return Err(LifeError::OtherGeneration {
                            asked: generation,
                            serving,
                        });
                    }
                    Ok(StopAnswer::Refused(reason)) => {
                        return Err(LifeError::CannotStop { reason });
                    }
                    Err(Unanswered::NotListening(_)) => {}
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
                },
            }
        }
        if Instant::now() > deadline {
            return match asked {
                Some(identity) => Ok(Stopped::Closing {
                    pid: Some(identity.pid),
                    generation: identity.generation,
                }),
                None => Err(LifeError::Hung {
                    hang: Hang::NotReady,
                    home: home.prepared.folder().to_path_buf(),
                    log: home.log(),
                    pid: home.pid_hint(),
                    attempt,
                }),
            };
        }
        tokio::time::sleep(POLL).await;
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

    /// The errno values are Unix's; on Windows a connect is refused with `WSAEACCES`, which std reads as the same
    /// `PermissionDenied` and nothing tests yet.
    #[cfg(unix)]
    #[test]
    fn a_connect_that_is_not_permitted_is_refused_and_a_refused_connection_is_nobody() {
        use quadratique_server::server::{RuntimeFile, Unanswered};
        use rustix::io::Errno;

        use super::Found;

        let runtime = RuntimeFile {
            pid: 1,
            address: "127.0.0.1:9".parse().expect("an address"),
            url: String::new(),
            database: "/home/workspace.db".into(),
            control: None,
        };
        let after =
            |errno: Errno| Found::unanswered(&runtime, Unanswered::of_connect(errno.into()));

        assert!(matches!(after(Errno::PERM), Found::Refused { .. }));
        assert!(matches!(after(Errno::ACCESS), Found::Refused { .. }));
        assert!(matches!(after(Errno::CONNREFUSED), Found::Nobody));
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

    #[cfg(unix)]
    #[test]
    fn the_tcp_address_comes_from_the_identity_url_only_when_it_is_a_loopback_url_of_the_command_line()
     {
        use quadratique_server::server::Identity;

        let identity = |url: &str| Identity {
            pid: 1,
            database: "/home/workspace.db".into(),
            interface: None,
            schema: None,
            generation: None,
            control: Some(1),
            controls: None,
            executable: None,
            started: None,
            url: Some(url.to_owned()),
        };

        let runtime =
            super::runtime_of(&identity("http://127.0.0.1:4100/cli")).expect("an address");

        assert_eq!(
            runtime.address,
            "127.0.0.1:4100".parse().expect("an address")
        );
        assert_eq!(runtime.control, Some(1));
        for refused in [
            "http://192.0.2.1:4100/cli",
            "http://127.0.0.1:4100/web",
            "https://127.0.0.1:4100/cli",
            "http://localhost:4100/cli",
        ] {
            assert!(super::runtime_of(&identity(refused)).is_none(), "{refused}");
        }
    }
}
