//! A server of a build before the Unix socket (release 0.1): the lock of the home is held, no socket answers, and a
//! runtime file without a control version names its TCP address. The command line finds it over TCP, uses it, and
//! stops it with SIGTERM to the process number it answers with, as release 0.1 did. This is the only code that signals
//! a process; QUA-55 replaces it with the proven legacy stop of docs/decisions/0004-…, §3. On Windows, which has no
//! socket yet (QUA-61), every server is reached this way.

use std::fs::File;
use std::fs::TryLockError;
use std::time::{Duration, Instant};

use quadratique_server::server::RuntimeFile;

use super::{Attempt, Found, Home, LifeError, OwnedChild, POLL, Stopped, find, log_tail};
use crate::process::{alive, ask_to_stop};

pub(super) async fn ensure(home: &Home, limit: Duration) -> Result<RuntimeFile, LifeError> {
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

pub(super) async fn stop(home: &Home, limit: Duration) -> Result<Stopped, LifeError> {
    match drive(home, Phase::FindingToStop, limit).await? {
        Settled::Stopped { pid } => Ok(Stopped::Ended {
            pid,
            generation: None,
        }),
        Settled::Closing { pid } => Ok(Stopped::Closing {
            pid,
            generation: None,
        }),
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

fn remove_runtime(home: &Home, stopped: Option<u32>) {
    let stale = home.runtime_file().is_some_and(|runtime| match stopped {
        Some(pid) => runtime.pid == pid,
        None => !alive(runtime.pid),
    });
    if stale {
        let _ = std::fs::remove_file(home.prepared.runtime());
    }
}
