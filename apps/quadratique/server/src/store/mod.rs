mod clock;
mod engine;
mod error;
mod git;
mod history;
mod index;
mod json;
mod message;
mod migrate;
mod recorded;
mod saved_views;
mod tx;
mod types;
mod view;
mod writer;

pub use engine::{canonical_folder, refuse_git_work_tree};
pub use error::{Kind, StoreError};
pub use git::{ClaimStart, ClaimedWork, GitLanding, GitWatch, GitWork};
pub use recorded::{Recorded, SCHEMA_VERSION, read_recorded};
pub use saved_views::{SavedViewList, SavedViewRecord};
pub use tx::Tx;
pub use types::{
    AsOf, Author, Call, Change, Claim, ClaimFilter, Commit, Direction, FieldChange, FindingLink,
    Graph, HolderSession, IssueCreated, IssueFields, IssueFilter, IssueRecord, KeyedIssue, Link,
    NewIssue, NoteRecord, Page, Project, ProjectChanges, RelationRecord, SourceRecord, Subject,
    WaitNode, Workflow, WorkflowStatus, Written,
};
pub use view::View;

use doltlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, oneshot};
use writer::Job;

const COMMIT_STREAM_CAPACITY: usize = 1024;

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    jobs: writer::Jobs,
    readers: Arc<Readers>,
    commits: broadcast::Sender<Commit>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Store, StoreError> {
        Store::open_with(path.as_ref(), migrate::MIGRATIONS)
    }

    fn open_with(path: &Path, migrations: &[migrate::Migration]) -> Result<Store, StoreError> {
        let path = engine::database_path(path)?;
        let connection = engine::open_writer(&path)?;
        let (commits, _) = broadcast::channel(COMMIT_STREAM_CAPACITY);
        let jobs = writer::Writer::start(connection, commits.clone(), &path, migrations)?;
        Ok(Store {
            inner: Arc::new(Inner {
                readers: Arc::new(Readers {
                    path: path.clone(),
                    idle: Mutex::new(Vec::new()),
                    held: Mutex::new(Vec::new()),
                }),
                path,
                jobs,
                commits,
            }),
        })
    }

    pub fn hold_until_closed(&self, held: impl Send + Sync + 'static) {
        let held: writer::Held = Arc::new(held);
        if let Ok(mut kept) = self.inner.readers.held.lock() {
            kept.push(Arc::clone(&held));
        }
        let job: Job = Box::new(move |writer| writer.hold(held));
        let _ = self.inner.jobs.send(job);
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub async fn write<T, E, F>(&self, call: Call, work: F) -> Result<Written<T>, E>
    where
        T: Send + 'static,
        E: From<StoreError> + Send + 'static,
        F: for<'c> FnOnce(&mut Tx<'c>) -> Result<T, E> + Send + 'static,
    {
        let (reply, outcome) = oneshot::channel();
        let job: Job = Box::new(move |writer| {
            let _ = reply.send(writer.write(&call, work));
        });
        self.inner.jobs.send(job).map_err(|_| StoreError::Closed)?;
        outcome.await.map_err(|_| StoreError::Closed)?
    }

    pub async fn read<T, E, F>(&self, as_of: AsOf, work: F) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<StoreError> + Send + 'static,
        F: for<'c> FnOnce(&View<'c>) -> Result<T, E> + Send + 'static,
    {
        let readers = Arc::clone(&self.inner.readers);
        tokio::task::spawn_blocking(move || readers.read(&as_of, work))
            .await
            .map_err(|_| StoreError::Closed)?
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Commit> {
        self.inner.commits.subscribe()
    }

    #[cfg(test)]
    pub(crate) fn fail_next_index(&self) {
        let job: Job = Box::new(|writer| writer.fail_next_index());
        let _ = self.inner.jobs.send(job);
    }

    #[cfg(test)]
    pub(crate) fn pause_after_next_seal(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (sealed, on_seal) = std::sync::mpsc::channel();
        let (resume, on_resume) = std::sync::mpsc::channel();
        let pause = writer::Pause {
            sealed,
            resume: on_resume,
        };
        let job: Job = Box::new(move |writer| writer.pause_after_next_seal(pause));
        let _ = self.inner.jobs.send(job);
        (on_seal, resume)
    }
}

#[cfg(test)]
mod tests;

struct Readers {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
    held: Mutex<Vec<writer::Held>>,
}

impl Readers {
    fn read<T, E, F>(&self, as_of: &AsOf, work: F) -> Result<T, E>
    where
        E: From<StoreError>,
        F: for<'c> FnOnce(&View<'c>) -> Result<T, E>,
    {
        let idle = self.idle.lock().ok().and_then(|mut idle| idle.pop());
        let connection = match idle {
            Some(connection) => connection,
            None => engine::open_reader(&self.path)?,
        };
        connection
            .execute_batch("BEGIN")
            .map_err(StoreError::from)?;
        let outcome = View::resolve(&connection, as_of)
            .map_err(E::from)
            .and_then(|view| work(&view));
        let closed = connection.execute_batch("COMMIT").is_ok();
        if closed && let Ok(mut idle) = self.idle.lock() {
            idle.push(connection);
        }
        outcome
    }
}
