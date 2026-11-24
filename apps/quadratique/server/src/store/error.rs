use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Project,
    Issue,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::Project => "project",
            Kind::Issue => "issue",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("the database engine failed: {0}")]
    Engine(String),
    #[error(
        "the linked SQLite is not doltlite ({0}); is a crate that links libsqlite3-sys in the build?"
    )]
    NotDoltlite(String),
    #[error(
        "the store file {path} is damaged or is not a Quadratique store ({reason}). Stop the server, check that it stopped, and copy a backup over the file: see \"Backups\" in Quadratique's README."
    )]
    Damaged { path: PathBuf, reason: String },
    #[error(
        "the store file {path} has schema version {found}, and this binary knows versions up to {known}. Nothing was written: install a newer binary."
    )]
    NewerStore {
        path: PathBuf,
        found: i64,
        known: i64,
    },
    #[error("refusing a database inside the git working tree at {0}")]
    InsideGitWorkTree(PathBuf),
    #[error("cannot use the database path: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid author: {0}")]
    InvalidAuthor(String),
    #[error("invalid JSON for {what}: {message}")]
    InvalidJson { what: &'static str, message: String },
    #[error("invalid time {0:?}")]
    InvalidTime(String),
    #[error("no single commit has the hash {0}")]
    UnknownCommit(String),
    #[error("the cursor does not belong to this listing")]
    BadCursor,
    #[error("no {kind} {id}")]
    NotFound { kind: Kind, id: String },
    #[error("a project with the key {0} exists")]
    ProjectExists(String),
    #[error("the repository is bound to the project {0}")]
    RepositoryBound(String),
    #[error("a commit message of the store cannot be read: {0}")]
    Corrupt(String),
    #[error("the store stopped before it answered: it closed, or the work panicked")]
    Closed,
}

impl StoreError {
    pub(crate) fn is_missing_table(&self, table: &str) -> bool {
        matches!(self, StoreError::Engine(message) if message.contains(&format!("table not found: {table}")))
    }
}

impl From<doltlite::Error> for StoreError {
    fn from(error: doltlite::Error) -> Self {
        StoreError::Engine(error.to_string())
    }
}
