use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use quadratique_server::app::interface;
use quadratique_server::server::{Paths, RuntimeFile, START_LIMIT, Unanswered, identity};
use quadratique_server::store::{StoreError, canonical_folder, refuse_git_work_tree};

use crate::process::{alive, ask_to_stop, detach};
use crate::route::McpTool;

const STOP_LIMIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(20);
const LOG_TAIL: usize = 2000;

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
        }
    }
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

pub struct Home {
    pub paths: Paths,
    directory: PathBuf,
}

impl Home {
    pub fn new(paths: Paths) -> Result<Home, StoreError> {
        let directory = paths
            .database
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        refuse_git_work_tree(&directory)?;
        Ok(Home { paths, directory })
    }

    pub fn log(&self) -> PathBuf {
        self.directory.join("server.log")
    }

    fn start_lock(&self) -> PathBuf {
        self.directory.join("start.lock")
    }

    fn open_lock(&self, path: &Path) -> std::io::Result<File> {
        std::fs::create_dir_all(&self.directory)?;
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
    }

    fn database(&self) -> PathBuf {
        let file = self.paths.database.file_name().unwrap_or_default();
        canonical_folder(&self.directory)
            .map(|directory| directory.join(file))
            .unwrap_or_else(|_| self.paths.database.clone())
    }

    fn runtime_file(&self) -> Option<RuntimeFile> {
        let text = std::fs::read(&self.paths.runtime).ok()?;
        serde_json::from_slice(&text).ok()
    }

    /// A process that may not write the home can still ask: `flock` takes a lock on a read-only descriptor, and a
    /// missing lock file means that no server ever held the home.
    pub fn held(&self) -> std::io::Result<bool> {
        let lock = match self.open_lock(&self.paths.lock()) {
            Ok(lock) => lock,
            Err(error) if cannot_write(&error) => match File::open(self.paths.lock()) {
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

#[derive(Clone, Copy)]
pub enum Attempt {
    Start,
    Stop,
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
            self.directory.display()
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
            Unanswered::NotListening(_) | Unanswered::NoIdentity => Found::Nobody,
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
            runtime: home.paths.runtime.clone(),
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

pub async fn ensure(home: &Home) -> Result<RuntimeFile, LifeError> {
    ensure_within(home, START_LIMIT).await
}

async fn ensure_within(home: &Home, limit: Duration) -> Result<RuntimeFile, LifeError> {
    match drive(home, Phase::Finding, limit).await? {
        Settled::Running(runtime) => Ok(runtime),
        Settled::NotRunning | Settled::Stopped { .. } | Settled::Closing { .. } => {
            Err(LifeError::NoAnswer {
                pid: None,
                log: home.log(),
            })
        }
    }
}

pub struct StartLockHeld {
    _start_lock: File,
}

pub async fn take_over(home: &Home) -> Result<StartLockHeld, LifeError> {
    let start_lock = await_start_lock(home, START_LIMIT).await?;
    stop(home).await?;
    Ok(StartLockHeld {
        _start_lock: start_lock,
    })
}

async fn await_start_lock(home: &Home, limit: Duration) -> Result<File, LifeError> {
    let deadline = Instant::now() + limit;
    let start_lock = home
        .open_lock(&home.start_lock())
        .map_err(|error| home.unusable(error, Attempt::Start))?;
    loop {
        match start_lock.try_lock() {
            Ok(()) => return Ok(start_lock),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        if Instant::now() > deadline {
            return Err(LifeError::NoAnswer {
                pid: home.runtime_file().map(|runtime| runtime.pid),
                log: home.log(),
            });
        }
        tokio::time::sleep(POLL).await;
    }
}

pub enum Stopped {
    NotRunning,
    Ended { pid: u32 },
    Closing { pid: Option<u32> },
}

pub async fn stop(home: &Home) -> Result<Stopped, LifeError> {
    stop_within(home, STOP_LIMIT).await
}

async fn stop_within(home: &Home, limit: Duration) -> Result<Stopped, LifeError> {
    match drive(home, Phase::FindingToStop, limit).await? {
        Settled::Stopped { pid } => Ok(Stopped::Ended { pid }),
        Settled::Closing { pid } => Ok(Stopped::Closing { pid }),
        Settled::NotRunning | Settled::Running(_) => Ok(Stopped::NotRunning),
    }
}

enum Phase {
    Finding,
    AwaitingStartLock,
    Rechecking { start_lock: File },
    Starting { start_lock: File, child: OwnedChild },
    FindingToStop,
    Stopping { pid: u32 },
}

enum Settled {
    Running(RuntimeFile),
    NotRunning,
    Stopped { pid: u32 },
    Closing { pid: Option<u32> },
}

enum Next {
    Now(Phase),
    AfterPoll(Phase),
    Settled(Settled),
}

async fn drive(home: &Home, first: Phase, limit: Duration) -> Result<Settled, LifeError> {
    let deadline = Instant::now() + limit;
    let mut phase = first;
    loop {
        phase = match step(home, phase).await? {
            Next::Settled(settled) => return Ok(settled),
            Next::Now(next) => next,
            Next::AfterPoll(next) => {
                if Instant::now() > deadline {
                    if let Some(stopping) = next.still_stopping(home)? {
                        return Ok(stopping);
                    }
                    return Err(LifeError::NoAnswer {
                        pid: next
                            .pid()
                            .or_else(|| home.runtime_file().map(|runtime| runtime.pid)),
                        log: home.log(),
                    });
                }
                tokio::time::sleep(POLL).await;
                next
            }
        };
    }
}

async fn step(home: &Home, phase: Phase) -> Result<Next, LifeError> {
    Ok(match phase {
        Phase::Finding => match find(home).await? {
            Found::Ours(runtime) => Next::Settled(Settled::Running(runtime)),
            Found::OtherInterface { runtime, theirs } => {
                return Err(Found::refusal(&runtime, theirs));
            }
            Found::Refused { reason } => {
                unreachable_if_held(home, reason)?;
                Next::Now(Phase::AwaitingStartLock)
            }
            Found::Nobody => Next::Now(Phase::AwaitingStartLock),
        },
        Phase::AwaitingStartLock => {
            let start_lock = home
                .open_lock(&home.start_lock())
                .map_err(|error| home.unusable(error, Attempt::Start))?;
            match start_lock.try_lock() {
                Ok(()) => Next::Now(Phase::Rechecking { start_lock }),
                Err(TryLockError::WouldBlock) => Next::AfterPoll(Phase::AwaitingStartLock),
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
        Phase::Rechecking { start_lock } => match find(home).await? {
            Found::Ours(runtime) => Next::Settled(Settled::Running(runtime)),
            Found::OtherInterface { runtime, theirs } => {
                return Err(Found::refusal(&runtime, theirs));
            }
            Found::Refused { reason } => {
                unreachable_if_held(home, reason)?;
                Next::AfterPoll(Phase::Starting {
                    start_lock,
                    child: OwnedChild::spawn(home)
                        .map_err(|error| home.unusable(error, Attempt::Start))?,
                })
            }
            Found::Nobody => Next::AfterPoll(Phase::Starting {
                start_lock,
                child: OwnedChild::spawn(home)
                    .map_err(|error| home.unusable(error, Attempt::Start))?,
            }),
        },
        Phase::Starting {
            start_lock,
            mut child,
        } => match find(home).await? {
            Found::Ours(runtime) => {
                if runtime.pid == child.id() {
                    child.hand_off();
                }
                Next::Settled(Settled::Running(runtime))
            }
            Found::OtherInterface { runtime, theirs } => {
                return Err(Found::refusal(&runtime, theirs));
            }
            Found::Refused { .. } | Found::Nobody => match child.try_wait()? {
                Some(status) => match find(home).await? {
                    Found::Ours(runtime) => Next::Settled(Settled::Running(runtime)),
                    Found::OtherInterface { runtime, theirs } => {
                        return Err(Found::refusal(&runtime, theirs));
                    }
                    Found::Refused { .. } | Found::Nobody => {
                        return Err(LifeError::Exited {
                            status,
                            log: home.log(),
                            tail: log_tail(&home.log()),
                        });
                    }
                },
                None => Next::AfterPoll(Phase::Starting { start_lock, child }),
            },
        },
        Phase::FindingToStop => {
            if !home
                .held()
                .map_err(|error| home.unusable(error, Attempt::Stop))?
            {
                remove_runtime(home, None);
                return Ok(Next::Settled(Settled::NotRunning));
            }
            match find(home).await? {
                Found::Ours(runtime) | Found::OtherInterface { runtime, .. } => {
                    ask_to_stop(runtime.pid)?;
                    Next::AfterPoll(Phase::Stopping { pid: runtime.pid })
                }
                Found::Refused { reason } => return Err(LifeError::UnreachableToStop { reason }),
                Found::Nobody => Next::AfterPoll(Phase::FindingToStop),
            }
        }
        Phase::Stopping { pid } => {
            if home
                .held()
                .map_err(|error| home.unusable(error, Attempt::Stop))?
            {
                Next::AfterPoll(Phase::Stopping { pid })
            } else {
                remove_runtime(home, Some(pid));
                Next::Settled(Settled::Stopped { pid })
            }
        }
    })
}

fn unreachable_if_held(home: &Home, reason: String) -> Result<(), LifeError> {
    if home
        .held()
        .map_err(|error| home.unusable(error, Attempt::Start))?
    {
        return Err(LifeError::Unreachable { reason });
    }
    Ok(())
}

impl Phase {
    fn still_stopping(&self, home: &Home) -> Result<Option<Settled>, LifeError> {
        let pid = match self {
            Phase::FindingToStop => None,
            Phase::Stopping { pid } => Some(*pid),
            _ => return Ok(None),
        };
        Ok(home
            .held()
            .map_err(|error| home.unusable(error, Attempt::Stop))?
            .then_some(Settled::Closing { pid }))
    }

    fn pid(&self) -> Option<u32> {
        match self {
            Phase::Starting { child, .. } => Some(child.id()),
            Phase::Stopping { pid } => Some(*pid),
            Phase::Finding
            | Phase::AwaitingStartLock
            | Phase::Rechecking { .. }
            | Phase::FindingToStop => None,
        }
    }
}

struct OwnedChild {
    child: Option<Child>,
}

impl OwnedChild {
    fn spawn(home: &Home) -> std::io::Result<OwnedChild> {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.log())?;
        let mut command = Command::new(std::env::current_exe()?);
        if let Some(directory) = &home.paths.home {
            command.env("QUADRATIQUE_HOME", directory);
        }
        let child = detach(&mut command)
            .arg("serve")
            .current_dir(&home.directory)
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

fn remove_runtime(home: &Home, stopped: Option<u32>) {
    let stale = home.runtime_file().is_some_and(|runtime| match stopped {
        Some(pid) => runtime.pid == pid,
        None => !alive(runtime.pid),
    });
    if stale {
        let _ = std::fs::remove_file(&home.paths.runtime);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use quadratique_server::server::Paths;

    use super::{Home, LifeError, Stopped, ensure_within, stop_within};

    const HANG_GUARD: Duration = Duration::from_secs(20);

    #[tokio::test]
    async fn a_start_lock_that_another_starter_never_releases_ends_at_the_deadline() {
        let directory = tempfile::tempdir().expect("a temporary home");
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
        };
        let after =
            |errno: Errno| Found::unanswered(&runtime, Unanswered::of_connect(errno.into()));

        assert!(matches!(after(Errno::PERM), Found::Refused { .. }));
        assert!(matches!(after(Errno::ACCESS), Found::Refused { .. }));
        assert!(matches!(after(Errno::CONNREFUSED), Found::Nobody));
    }

    #[tokio::test]
    async fn a_stop_of_a_home_that_stays_held_without_an_answer_says_the_server_is_stopping() {
        let directory = tempfile::tempdir().expect("a temporary home");
        let home = Home::new(Paths::in_home(directory.path()).expect("a home")).expect("a home");
        let closing_server = home.open_lock(&home.paths.lock()).expect("the home's lock");
        closing_server
            .lock()
            .expect("a server closing its store holds the home");

        let stopped =
            tokio::time::timeout(HANG_GUARD, stop_within(&home, Duration::from_millis(200))).await;

        drop(closing_server);
        assert!(
            matches!(stopped, Ok(Ok(Stopped::Closing { pid: None }))),
            "the stop did not say the server is stopping"
        );
    }
}
