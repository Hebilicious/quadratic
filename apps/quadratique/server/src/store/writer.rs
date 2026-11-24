use super::clock::Clock;
use super::engine::{Cells, query_one};
use super::error::StoreError;
use super::index::{self, Subjects};
use super::message::{Message, committer};
use super::migrate::{self, Migration, Version, schema_version};
use super::tx::Tx;
use super::types::{Author, Call, Commit, Written};
use super::view::View;
use doltlite::{Connection, Value};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use tokio::sync::{broadcast, mpsc};

pub(crate) type Job = Box<dyn FnOnce(&mut Writer) + Send>;
pub(crate) type Held = std::sync::Arc<dyn Send + Sync>;

pub(crate) struct Jobs {
    queue: mpsc::UnboundedSender<Job>,
    _thread: Joined,
}

impl Jobs {
    pub(crate) fn send(&self, job: Job) -> Result<(), mpsc::error::SendError<Job>> {
        self.queue.send(job)
    }
}

struct Joined(Option<std::thread::JoinHandle<()>>);

impl Drop for Joined {
    fn drop(&mut self) {
        if let Some(thread) = self.0.take()
            && thread.thread().id() != std::thread::current().id()
        {
            let _ = thread.join();
        }
    }
}

pub(crate) struct Writer {
    connection: Connection,
    clock: Clock,
    between_jobs: BetweenJobs,
    commits: broadcast::Sender<Commit>,
    #[cfg(test)]
    fail_next_index: bool,
    #[cfg(test)]
    pause_after_seal: Option<Pause>,
    held: Vec<Held>,
}

#[cfg(test)]
pub(crate) struct Pause {
    pub(crate) sealed: std::sync::mpsc::Sender<()>,
    pub(crate) resume: std::sync::mpsc::Receiver<()>,
}

enum BetweenJobs {
    Settled,
    Unreconciled,
}

struct Durable {
    parent: String,
    hash: String,
}

const STORE_ACTOR: &str = r#"{"harness":"quadratique","kind":"agent","name":"quadratique"}"#;

impl Writer {
    pub(crate) fn start(
        connection: Connection,
        commits: broadcast::Sender<Commit>,
        path: &Path,
        migrations: &[Migration],
    ) -> Result<Jobs, StoreError> {
        let mut writer = Writer {
            connection,
            clock: Clock::default(),
            between_jobs: BetweenJobs::Settled,
            commits,
            #[cfg(test)]
            fail_next_index: false,
            #[cfg(test)]
            pause_after_seal: None,
            held: Vec::new(),
        };
        writer.prepare(path, migrations)?;
        let (jobs, mut queue) = mpsc::unbounded_channel::<Job>();
        let thread = std::thread::Builder::new()
            .name("quadratique-store-writer".into())
            .spawn(move || {
                while let Some(job) = queue.blocking_recv() {
                    let served = catch_unwind(AssertUnwindSafe(|| job(&mut writer)));
                    if served.is_err() {
                        writer.rollback();
                        writer.between_jobs = BetweenJobs::Unreconciled;
                    }
                }
            })?;
        Ok(Jobs {
            queue: jobs,
            _thread: Joined(Some(thread)),
        })
    }

    pub(crate) fn hold(&mut self, held: Held) {
        self.held.push(held);
    }

    fn prepare(&mut self, path: &Path, migrations: &[Migration]) -> Result<(), StoreError> {
        let found = schema_version(&self.connection)?;
        match migrate::compare(found, migrations) {
            Version::Current => {}
            Version::Behind(version) => {
                if self.has_commit_index()? {
                    self.between_jobs = BetweenJobs::Unreconciled;
                    self.reconcile()?;
                    self.clock = Clock::after(index::last_at(&self.connection)?.as_deref());
                }
                for (version, migration) in migrate::numbered(migrations).skip(version) {
                    self.migrate(version, migration)?;
                }
            }
            Version::Ahead(found) => {
                return Err(StoreError::NewerStore {
                    path: path.to_path_buf(),
                    found,
                    known: migrations.len() as i64,
                });
            }
        }
        self.between_jobs = BetweenJobs::Unreconciled;
        self.reconcile()?;
        self.clock = Clock::after(index::last_at(&self.connection)?.as_deref());
        Ok(())
    }

    fn has_commit_index(&self) -> Result<bool, StoreError> {
        Ok(query_one(
            &self.connection,
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'commit_index'",
            &[],
        )?
        .is_some())
    }

    fn migrate(&mut self, version: usize, migration: &Migration) -> Result<(), StoreError> {
        let call = Call::new(Author::new(STORE_ACTOR)?, "store.migrate")
            .reason(format!("schema version {version}: {}", migration.name));
        self.write(&call, |tx: &mut Tx<'_>| {
            tx.connection.execute_batch(migration.sql)?;
            tx.connection
                .execute_batch(&format!("PRAGMA user_version = {version}"))
                .map_err(StoreError::from)
        })?;
        Ok(())
    }

    fn reconcile(&mut self) -> Result<Vec<Commit>, StoreError> {
        if let BetweenJobs::Settled = self.between_jobs {
            return Ok(Vec::new());
        }
        let recovered = index::catch_up(&self.connection)?
            .into_iter()
            .map(|recorded| {
                let message = Message::decode(&recorded.message)?;
                Ok(Commit {
                    hash: recorded.hash,
                    at: message.at,
                    author: message.actor,
                    operation: message.operation,
                    reason: message.reason,
                    branch: message.branch,
                    worktree: message.worktree,
                    issues: recorded.subjects.issues.into_iter().collect(),
                    projects: recorded.subjects.projects.into_iter().collect(),
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        self.between_jobs = BetweenJobs::Settled;
        for commit in &recovered {
            self.publish(commit);
        }
        Ok(recovered)
    }

    pub(crate) fn write<T, E, F>(&mut self, call: &Call, work: F) -> Result<Written<T>, E>
    where
        E: From<StoreError>,
        F: for<'c> FnOnce(&mut Tx<'c>) -> Result<T, E>,
    {
        self.reconcile()?;
        let at = self.clock.next();
        let message = Message::of(call, &at).encode();
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(StoreError::from)?;
        let value = match self.run(&at, work) {
            Ok(value) => value,
            Err(error) => {
                self.rollback();
                return Err(error);
            }
        };
        let durable = match self.seal(call, &message) {
            Ok(Some(durable)) => durable,
            Ok(None) => {
                return Ok(Written {
                    value,
                    commit: None,
                });
            }
            Err(error) => {
                self.rollback();
                return Err(error.into());
            }
        };
        #[cfg(test)]
        if let Some(pause) = self.pause_after_seal.take() {
            let _ = pause.sealed.send(());
            let _ = pause
                .resume
                .recv_timeout(std::time::Duration::from_secs(60));
        }
        let commit = match self.index(call, &at, &message, &durable) {
            Ok(commit) => {
                self.publish(&commit);
                commit
            }
            Err(_) => {
                self.rollback();
                self.between_jobs = BetweenJobs::Unreconciled;
                self.reconcile()?
                    .into_iter()
                    .find(|commit| commit.hash == durable.hash)
                    .ok_or_else(|| {
                        StoreError::Engine(format!(
                            "the commit {} is durable but the index lacks it",
                            durable.hash
                        ))
                    })?
            }
        };
        Ok(Written {
            value,
            commit: Some(commit),
        })
    }

    fn run<T, E, F>(&self, at: &str, work: F) -> Result<T, E>
    where
        E: From<StoreError>,
        F: for<'c> FnOnce(&mut Tx<'c>) -> Result<T, E>,
    {
        let mut tx = Tx::new(View::now(&self.connection), at.to_owned());
        let value = work(&mut tx)?;
        tx.finish()?;
        Ok(value)
    }

    fn seal(&mut self, call: &Call, message: &str) -> Result<Option<Durable>, StoreError> {
        if !self.dirty()? {
            self.connection.execute_batch("COMMIT")?;
            return Ok(None);
        }
        let parent = index::head(&self.connection)?;
        let row = query_one(
            &self.connection,
            "SELECT dolt_commit('-Am', ?, '--author', ?)",
            &[message.into(), Value::from(committer(call.author.name()))],
        )?
        .ok_or_else(|| StoreError::Engine("dolt_commit returned no hash".into()))?;
        Ok(Some(Durable {
            parent,
            hash: Cells(&row).text(0),
        }))
    }

    fn index(
        &mut self,
        call: &Call,
        at: &str,
        message: &str,
        durable: &Durable,
    ) -> Result<Commit, StoreError> {
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_index) {
            return Err(StoreError::Engine("the index step fails once".into()));
        }
        let first =
            query_one(&self.connection, "SELECT 1 FROM commit_index LIMIT 1", &[])?.is_none();
        let subjects = if first {
            Subjects::default()
        } else {
            index::subjects(&self.connection, &durable.parent, &durable.hash)?
        };
        index::record(
            &self.connection,
            &durable.parent,
            &durable.hash,
            message,
            at,
            &subjects,
        )?;
        self.connection.execute_batch("COMMIT")?;
        Ok(Commit {
            hash: durable.hash.clone(),
            at: at.to_owned(),
            author: call.author.record().to_owned(),
            operation: call.operation.clone(),
            reason: call.reason.clone(),
            branch: call.branch.clone(),
            worktree: call.worktree.clone(),
            issues: subjects.issues.into_iter().collect(),
            projects: subjects.projects.into_iter().collect(),
        })
    }

    fn publish(&self, commit: &Commit) {
        let _ = self.commits.send(commit.clone());
    }

    fn dirty(&self) -> Result<bool, StoreError> {
        Ok(query_one(&self.connection, "SELECT 1 FROM dolt_status LIMIT 1", &[])?.is_some())
    }

    fn rollback(&self) {
        let _ = self.connection.execute_batch("ROLLBACK");
    }
}

#[cfg(test)]
impl Writer {
    pub(crate) fn fail_next_index(&mut self) {
        self.fail_next_index = true;
    }

    pub(crate) fn pause_after_next_seal(&mut self, pause: Pause) {
        self.pause_after_seal = Some(pause);
    }
}
