mod advance;
mod ancestry;
mod board;
mod caller;
mod catalog;
mod changed;
mod cursor;
mod findings;
mod history;
mod human_word;
mod instructions;
mod issues;
pub(crate) mod landings;
mod lifecycle;
mod links;
mod model;
mod projects;
mod tokens;
mod views;

use std::sync::Arc;

use acid_cat_core::{Application, Defect, RunError, RunResult};
use serde_json::{Value, json};

use crate::credentials::Credentials;
use crate::domain::{Actor, Caller, Failure, WorkingCopy};
use crate::git::watcher::Looks;
use crate::store::{AsOf, Author, Call, Kind, Store, StoreError, Tx, View, Written};
use model::IssueSummary;

pub use ancestry::branch_checks_in;
pub use board::board_loads_on_this_thread;
pub use caller::{
    ACTOR_HEADER, BRANCH_HEADER, REPOSITORY_HEADER, WORKTREE_HEADER, caller_of, decode_header,
    encode_header,
};
pub use catalog::{CATALOG_JSON, input_refusal, interface};

type IssueReader<R> = fn(&View<'_>, &str, &Caller) -> Result<R, Refused>;

pub struct Context {
    store: Option<Store>,
    caller: Caller,
    looks: Option<Looks>,
    credentials: Option<Credentials>,
}

impl Context {
    pub fn new(store: Store, caller: Caller) -> Context {
        Context {
            store: Some(store),
            caller,
            looks: None,
            credentials: None,
        }
    }

    pub fn watching(store: Store, caller: Caller, looks: Looks) -> Context {
        Context {
            looks: Some(looks),
            ..Context::new(store, caller)
        }
    }

    pub fn remote() -> Context {
        Context {
            store: None,
            caller: Caller {
                actor: Actor::without_header(),
                working_copy: WorkingCopy::default(),
            },
            looks: None,
            credentials: None,
        }
    }

    pub fn with_credentials(self, credentials: Credentials) -> Context {
        Context {
            credentials: Some(credentials),
            ..self
        }
    }

    fn credentials(&self) -> Result<&Credentials, Refused> {
        self.credentials.as_ref().ok_or_else(|| {
            Refused::defect("this route has no credentials: only the command line manages tokens")
        })
    }

    fn store(&self) -> Result<&Store, Refused> {
        self.store.as_ref().ok_or_else(|| {
            Refused::defect("this process has no store: its command line calls the server")
        })
    }

    fn call(&self, operation: &str, reason: Option<String>) -> Result<Call, Refused> {
        let record = serde_json::to_string(&self.caller.actor).map_err(Refused::defect)?;
        let mut call = Call::new(Author::new(&record)?, operation);
        call.reason = reason;
        call.branch = self.caller.working_copy.branch.clone();
        call.worktree = self.caller.working_copy.worktree.clone();
        Ok(call)
    }

    async fn write<T, F>(
        &self,
        operation: &str,
        reason: Option<String>,
        work: F,
    ) -> Result<Written<T>, Refused>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&mut Tx<'c>) -> Result<T, Refused> + Send + 'static,
    {
        self.store()?
            .write(self.call(operation, reason)?, work)
            .await
    }

    async fn read<T, F>(&self, as_of: AsOf, work: F) -> Result<T, Refused>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&View<'c>) -> Result<T, Refused> + Send + 'static,
    {
        self.store()?.read(as_of, work).await
    }

    async fn write_issue<T, F>(
        &self,
        operation: &str,
        reason: Option<String>,
        work: F,
    ) -> Result<(T, IssueSummary), Refused>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&mut Tx<'c>) -> Result<(T, String), Refused> + Send + 'static,
    {
        self.write_then_read(operation, reason, work, board::summary)
            .await
    }

    async fn write_then_read<T, R, F>(
        &self,
        operation: &str,
        reason: Option<String>,
        work: F,
        answer: IssueReader<R>,
    ) -> Result<(T, R), Refused>
    where
        T: Send + 'static,
        R: Send + 'static,
        F: for<'c> FnOnce(&mut Tx<'c>) -> Result<(T, String), Refused> + Send + 'static,
    {
        let caller = self.caller.clone();
        let written = self
            .write(operation, reason, move |tx| {
                let (value, id) = work(tx)?;
                let seen = answer(tx, &id, &caller)?;
                Ok((value, id, seen))
            })
            .await?;
        let (value, id, seen) = written.value;
        let Some(commit) = written.commit else {
            return Ok((value, seen));
        };
        let caller = self.caller.clone();
        let committed = self
            .read(AsOf::Commit(commit.hash), move |view| {
                answer(view, &id, &caller)
            })
            .await?;
        Ok((value, committed))
    }
}

pub fn application() -> Result<Arc<Application<Context>>, Vec<String>> {
    let default_workflow = catalog::default_workflow().map_err(|problem| vec![problem])?;
    catalog::app()
        .capability(instructions::instructions())
        .capability(projects::list())
        .capability(projects::create(default_workflow))
        .capability(projects::update())
        .capability(issues::ready())
        .capability(issues::list())
        .capability(issues::get())
        .capability(issues::create())
        .capability(issues::update())
        .capability(lifecycle::claim())
        .capability(lifecycle::release())
        .capability(lifecycle::close())
        .capability(lifecycle::close_several())
        .capability(links::link())
        .capability(findings::import())
        .capability(advance::advance())
        .capability(history::query())
        .capability(tokens::create())
        .capability(tokens::list())
        .capability(tokens::revoke())
        .capability(views::list())
        .capability(views::save())
        .capability(views::rename())
        .capability(views::delete())
        .capability(views::import())
        .event(changed::event())
        .event(views::event())
        .build()
}

#[derive(Debug)]
pub(crate) enum Refused {
    Failure(Value),
    Store(StoreError),
    Defect(String),
}

impl Refused {
    fn tagged(tag: &str) -> Refused {
        Refused::Failure(json!({ "_tag": tag }))
    }

    fn not_found(kind: Kind, id: &str) -> Refused {
        Refused::Failure(json!({ "_tag": "NotFound", "kind": kind.to_string(), "id": id }))
    }

    fn defect(error: impl std::fmt::Display) -> Refused {
        Refused::Defect(error.to_string())
    }
}

impl From<StoreError> for Refused {
    fn from(error: StoreError) -> Refused {
        match error {
            StoreError::NotFound { kind, id } => Refused::not_found(kind, &id),
            StoreError::UnknownCommit(commit) => {
                Refused::Failure(json!({ "_tag": "UnknownCommit", "commit": commit }))
            }
            StoreError::BadCursor => Refused::tagged("BadCursor"),
            StoreError::RepositoryBound(project) => {
                Refused::Failure(json!({ "_tag": "RepositoryBound", "project": project }))
            }
            StoreError::ProjectExists(project) => {
                Refused::Failure(json!({ "_tag": "KeyTaken", "project": project }))
            }
            other => Refused::Store(other),
        }
    }
}

impl From<Failure> for Refused {
    fn from(failure: Failure) -> Refused {
        match serde_json::to_value(failure) {
            Ok(value) => Refused::Failure(value),
            Err(error) => Refused::defect(error),
        }
    }
}

fn finish<O>(outcome: Result<O, Refused>) -> RunResult<O, Value> {
    outcome.map_err(|refused| match refused {
        Refused::Failure(failure) => RunError::Failure(failure),
        Refused::Store(error) => RunError::Defect(Defect::from(error)),
        Refused::Defect(message) => RunError::Defect(Defect::new(message)),
    })
}
