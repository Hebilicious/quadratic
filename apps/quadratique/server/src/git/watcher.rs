use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, Interval, MissedTickBehavior};

use super::repository::{GitError, LandingTips, Repository, landing_tips};
use super::watch::{Landed, Observed, continues, step};
use crate::app::landings::{self, Watched};
use crate::store::{ClaimedWork, Store};

pub const EVERY: Duration = Duration::from_secs(2);
pub const READ_WAIT: Duration = EVERY;
const MOVES_OF_A_HOLDER: [&str; 4] = [
    "issue.claim",
    "issue.close",
    "issue.release",
    "issue.update",
];

enum Request {
    Look(Scope, Waiter),
    Stop,
}

enum Waiter {
    Done(oneshot::Sender<()>),
    Confirm(String, oneshot::Sender<Result<Confirmed, LookFailed>>),
}

impl Waiter {
    fn answer(self, report: &Report) {
        match self {
            Waiter::Done(done) => {
                let _ = done.send(());
            }
            Waiter::Confirm(key, done) => {
                let outcome = report.get(&key).cloned().unwrap_or_else(|| {
                    Err(LookFailed::Failed(format!(
                        "the ref watcher did not look at the project {key}: it has no repository, or the projects could not be read"
                    )))
                });
                let _ = done.send(outcome);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed {
    pub repository: String,
    pub default_branch: String,
    pub tips: LandingTips,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LookFailed {
    #[error("{0}")]
    TimedOut(String),
    #[error("{0}")]
    Failed(String),
}

impl From<GitError> for LookFailed {
    fn from(error: GitError) -> LookFailed {
        match error {
            GitError::TooSlow { .. } => LookFailed::TimedOut(error.to_string()),
            other => LookFailed::Failed(other.to_string()),
        }
    }
}

type Report = BTreeMap<String, Result<Confirmed, LookFailed>>;

enum Scope {
    Every,
    Projects(BTreeSet<String>),
}

impl Scope {
    fn widen(self, other: Scope) -> Scope {
        match (self, other) {
            (Scope::Projects(mut keys), Scope::Projects(more)) => {
                keys.extend(more);
                Scope::Projects(keys)
            }
            _ => Scope::Every,
        }
    }

    fn covers(&self, key: &str) -> bool {
        match self {
            Scope::Every => true,
            Scope::Projects(keys) => keys.contains(key),
        }
    }
}

struct Looking {
    scope: Scope,
    waiting: Vec<Waiter>,
}

impl Looking {
    fn of(scope: Scope, done: Option<Waiter>) -> Looking {
        Looking {
            scope,
            waiting: done.into_iter().collect(),
        }
    }

    fn join(self, scope: Scope, done: Waiter) -> Looking {
        let mut waiting = self.waiting;
        waiting.push(done);
        Looking {
            scope: self.scope.widen(scope),
            waiting,
        }
    }
}

enum Phase {
    Idle,
    Observing(Looking),
    Stopping,
}

pub struct Watcher {
    looks: Looks,
    task: JoinHandle<()>,
}

#[derive(Clone)]
pub struct Looks {
    requests: mpsc::UnboundedSender<Request>,
    wait: Duration,
}

impl Looks {
    pub async fn project(&self, key: &str) {
        let (done, looked) = oneshot::channel();
        if self.ask(one(key), Waiter::Done(done)) {
            let _ = tokio::time::timeout(self.wait, looked).await;
        }
    }

    pub fn confirm(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Confirmed, LookFailed>> + use<> {
        let (done, looked) = oneshot::channel();
        let asked = self.ask(one(key), Waiter::Confirm(key.to_owned(), done));
        async move {
            let stopped = || LookFailed::Failed("the ref watcher stopped before it looked".into());
            if !asked {
                return Err(stopped());
            }
            looked.await.unwrap_or_else(|_| Err(stopped()))
        }
    }

    fn ask(&self, scope: Scope, waiter: Waiter) -> bool {
        self.requests.send(Request::Look(scope, waiter)).is_ok()
    }
}

fn one(key: &str) -> Scope {
    Scope::Projects(BTreeSet::from([key.to_owned()]))
}

#[derive(PartialEq, Eq)]
struct Looked {
    repository: String,
    default_branch: String,
    heads: String,
}

struct Memory {
    looked: BTreeMap<String, Looked>,
    unreadable: BTreeSet<String>,
    git_limit: Duration,
}

struct Triggers {
    queue: mpsc::UnboundedReceiver<Request>,
    ticks: Interval,
    commits: Option<broadcast::Receiver<crate::store::Commit>>,
}

impl Watcher {
    pub fn start(
        store: Store,
        every: Duration,
        git_limit: Duration,
        read_wait: Duration,
    ) -> Watcher {
        let (requests, queue) = mpsc::unbounded_channel();
        let mut ticks = tokio::time::interval_at(Instant::now() + every, every);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut triggers = Triggers {
            queue,
            ticks,
            commits: Some(store.subscribe()),
        };
        let task = tokio::spawn(async move {
            let mut memory = Memory {
                looked: BTreeMap::new(),
                unreadable: BTreeSet::new(),
                git_limit,
            };
            let mut phase = Phase::Observing(Looking::of(Scope::Every, None));
            loop {
                phase = match phase {
                    Phase::Idle => triggers.next_look().await,
                    Phase::Observing(looking) => {
                        triggers.observe(&store, &mut memory, looking).await
                    }
                    Phase::Stopping => return,
                };
            }
        });
        Watcher {
            looks: Looks {
                requests,
                wait: read_wait,
            },
            task,
        }
    }

    pub fn looks(&self) -> Looks {
        self.looks.clone()
    }

    pub async fn look(&self) {
        let (done, looked) = oneshot::channel();
        if self.looks.ask(Scope::Every, Waiter::Done(done)) {
            let _ = looked.await;
        }
    }

    pub async fn stop(self) {
        let _ = self.looks.requests.send(Request::Stop);
        let _ = self.task.await;
    }
}

impl Triggers {
    async fn next_look(&mut self) -> Phase {
        loop {
            tokio::select! {
                biased;
                request = self.queue.recv() => return match request {
                    Some(Request::Look(scope, done)) => Phase::Observing(Looking::of(scope, Some(done))),
                    Some(Request::Stop) | None => Phase::Stopping,
                },
                _ = self.ticks.tick() => return Phase::Observing(Looking::of(Scope::Every, None)),
                moved = holder_moved(&mut self.commits) => {
                    if moved {
                        return Phase::Observing(Looking::of(Scope::Every, None));
                    }
                }
            }
        }
    }

    async fn observe(&mut self, store: &Store, memory: &mut Memory, current: Looking) -> Phase {
        let looking = look(store, memory, &current.scope);
        tokio::pin!(looking);
        let mut again: Option<Looking> = None;
        let report = loop {
            tokio::select! {
                biased;
                request = self.queue.recv() => match request {
                    Some(Request::Look(scope, done)) => {
                        again = Some(match again {
                            Some(next) => next.join(scope, done),
                            None => Looking::of(scope, Some(done)),
                        });
                    }
                    Some(Request::Stop) | None => return Phase::Stopping,
                },
                report = &mut looking => break report,
            }
        };
        for waiter in current.waiting {
            waiter.answer(&report);
        }
        match again {
            Some(next) => Phase::Observing(next),
            None => Phase::Idle,
        }
    }
}

async fn holder_moved(commits: &mut Option<broadcast::Receiver<crate::store::Commit>>) -> bool {
    let Some(receiver) = commits else {
        return std::future::pending().await;
    };
    match receiver.recv().await {
        Ok(commit) => MOVES_OF_A_HOLDER.contains(&commit.operation.as_str()),
        Err(RecvError::Lagged(_)) => true,
        Err(RecvError::Closed) => {
            *commits = None;
            false
        }
    }
}

async fn look(store: &Store, memory: &mut Memory, scope: &Scope) -> Report {
    let mut report = Report::new();
    let Ok(projects) = landings::watched(store).await else {
        return report;
    };
    memory
        .looked
        .retain(|key, _| projects.iter().any(|project| project.key == *key));
    for project in projects.iter().filter(|project| scope.covers(&project.key)) {
        let outcome = watch(store, project, memory).await;
        match &outcome {
            Ok(_) => {
                memory.unreadable.remove(&project.key);
            }
            Err(problem) => {
                if memory.unreadable.insert(project.key.clone()) {
                    eprintln!(
                        "quadratique: the ref watcher skips the project {}: {problem}",
                        project.key
                    );
                }
            }
        }
        report.insert(project.key.clone(), outcome);
    }
    report
}

async fn watch(
    store: &Store,
    project: &Watched,
    memory: &mut Memory,
) -> Result<Confirmed, LookFailed> {
    let repository = Repository::at(&project.repository, memory.git_limit);
    let heads = repository.heads().await?;
    let confirmed = Confirmed {
        repository: project.repository.clone(),
        default_branch: project.default_branch.clone(),
        tips: landing_tips(&heads, &project.default_branch),
    };
    let looked = Looked {
        repository: project.repository.clone(),
        default_branch: project.default_branch.clone(),
        heads,
    };
    if memory.looked.get(&project.key) == Some(&looked) {
        return Ok(confirmed);
    }
    let known = project
        .known
        .as_ref()
        .filter(|known| continues(known, &project.repository, &project.default_branch));
    let seen = repository
        .look_at(&looked.heads, &project.default_branch, known)
        .await?;
    if let Some(seen) = seen {
        let (observed, next) = step(project.known.as_ref(), &seen);
        let mut landed = Vec::new();
        for observed in observed {
            landed.extend(confirm(&repository, observed, &project.claims).await?);
        }
        let refused = |refused| LookFailed::Failed(format!("{refused:?}"));
        landings::settle(store, project, landed)
            .await
            .map_err(refused)?;
        landings::save(store, project, next)
            .await
            .map_err(refused)?;
    }
    memory.looked.insert(project.key.clone(), looked);
    Ok(confirmed)
}

async fn confirm(
    repository: &Repository,
    observed: Observed,
    claims: &[ClaimedWork],
) -> Result<Vec<Landed>, GitError> {
    let mut landed = Vec::new();
    let place = observed.place;
    for candidate in observed.branches {
        let carried = candidate.work == candidate.tip
            || repository.reaches(&candidate.work, &candidate.tip).await?;
        if !carried {
            continue;
        }
        let mut since_claim = Vec::new();
        for claim in claims
            .iter()
            .filter(|claim| claim.branch == candidate.branch)
        {
            if claim.tip_at_start != candidate.work
                && repository
                    .reaches(&claim.tip_at_start, &candidate.work)
                    .await?
            {
                since_claim.push(claim.clone());
            }
        }
        landed.push(Landed::Branch {
            branch: candidate.branch,
            tip: candidate.tip,
            claims: since_claim,
            place: place.clone(),
        });
    }
    landed.extend(observed.trailers.into_iter().map(|commit| Landed::Trailer {
        commit: commit.hash,
        refs: commit.refs,
        place: place.clone(),
    }));
    Ok(landed)
}
