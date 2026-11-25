//! Windows has no socket yet (QUA-61): every server is found over TCP through its runtime file, and stopped by the
//! process number it answers with, as release 0.1 did. `ask_to_stop` refuses there, so this path stops nothing until
//! QUA-61 brings the named pipe. On Unix the socket and `legacy` take its place.

use std::fs::File;
use std::fs::TryLockError;
use std::time::{Duration, Instant};

use quadratique_server::server::RuntimeFile;

use super::{Attempt, Found, Home, LifeError, OwnedChild, POLL, Stopped, find, log_tail};
use crate::process::{alive, ask_to_stop};

pub(super) async fn ensure(home: &Home, limit: Duration) -> Result<RuntimeFile, LifeError> {
    match drive(home, Phase::Finding, limit, false).await? {
        Settled::Running(runtime) => Ok(runtime),
        Settled::NotRunning
        | Settled::Stopped { .. }
        | Settled::Closing { .. }
        | Settled::NotLegacy => Err(LifeError::NoAnswer {
            pid: None,
            log: home.log(),
        }),
    }
}

pub(super) enum Stop {
    Done(Stopped),
    /// The server answered with a control version: it is not a build before the socket, and this path signals
    /// nothing. The caller goes on through the socket.
    NotLegacy,
}

/// Signals the process number that the server's identity gives. With `only_legacy`, the identity is checked again
/// right before the signal, and one with a control version ends this path without a signal.
pub(super) async fn stop(
    home: &Home,
    limit: Duration,
    only_legacy: bool,
) -> Result<Stop, LifeError> {
    Ok(
        match drive(home, Phase::FindingToStop, limit, only_legacy).await? {
            Settled::Stopped { pid } => Stop::Done(Stopped::Ended {
                pid,
                generation: None,
            }),
            Settled::Closing { pid } => Stop::Done(Stopped::Closing {
                pid,
                generation: None,
            }),
            Settled::NotRunning | Settled::Running(_) => Stop::Done(Stopped::NotRunning),
            Settled::NotLegacy => Stop::NotLegacy,
        },
    )
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
    NotLegacy,
}

enum Next {
    Now(Phase),
    AfterPoll(Phase),
    Settled(Settled),
}

async fn drive(
    home: &Home,
    first: Phase,
    limit: Duration,
    only_legacy: bool,
) -> Result<Settled, LifeError> {
    let deadline = Instant::now() + limit;
    let mut phase = first;
    loop {
        phase = match step(home, phase, only_legacy).await? {
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

async fn step(home: &Home, phase: Phase, only_legacy: bool) -> Result<Next, LifeError> {
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
                Found::Ours(runtime) | Found::OtherInterface { runtime, .. }
                    if only_legacy && runtime.control.is_some() =>
                {
                    Next::Settled(Settled::NotLegacy)
                }
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

fn remove_runtime(home: &Home, stopped: Option<u32>) {
    let stale = home.runtime_file().is_some_and(|runtime| match stopped {
        Some(pid) => runtime.pid == pid,
        None => !alive(runtime.pid),
    });
    if stale {
        let _ = std::fs::remove_file(home.prepared.runtime());
    }
}
