use super::error::StoreError;
use super::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    name: String,
    record: String,
}

impl Author {
    pub fn new(record: &str) -> Result<Author, StoreError> {
        let value: serde_json::Value = serde_json::from_str(record).map_err(|error| {
            StoreError::InvalidAuthor(format!("the actor record is not JSON: {error}"))
        })?;
        let name = value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| StoreError::InvalidAuthor("the actor record has no name".into()))?;
        if name.is_empty() {
            return Err(StoreError::InvalidAuthor(
                "the actor's name is empty".into(),
            ));
        }
        Ok(Author {
            name: name.to_owned(),
            record: json::text(&value),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn record(&self) -> &str {
        &self.record
    }
}

#[derive(Debug, Clone)]
pub struct Call {
    pub author: Author,
    pub operation: String,
    pub reason: Option<String>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
}

impl Call {
    pub fn new(author: Author, operation: impl Into<String>) -> Call {
        Call {
            author,
            operation: operation.into(),
            reason: None,
            branch: None,
            worktree: None,
        }
    }

    pub fn reason(mut self, reason: impl Into<String>) -> Call {
        self.reason = Some(reason.into());
        self
    }

    pub fn working_copy(mut self, branch: impl Into<String>, worktree: impl Into<String>) -> Call {
        self.branch = Some(branch.into());
        self.worktree = Some(worktree.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub hash: String,
    pub at: String,
    pub author: String,
    pub operation: String,
    pub reason: Option<String>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub issues: Vec<String>,
    pub projects: Vec<String>,
}

#[derive(Debug)]
pub struct Written<T> {
    pub value: T,
    pub commit: Option<Commit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsOf {
    Now,
    Commit(String),
    Time(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowStatus {
    pub key: String,
    pub name: String,
    pub category: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workflow {
    pub statuses: Vec<WorkflowStatus>,
    pub moves: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub key: String,
    pub name: String,
    pub repository: Option<String>,
    pub default_branch: String,
    pub extra: String,
    pub workflow: Workflow,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectChanges {
    pub name: Option<String>,
    pub default_branch: Option<String>,
    pub repository: Option<Option<String>>,
    pub extra: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    pub holder: String,
    pub branch: String,
    pub worktree: String,
    pub since: String,
}

impl Claim {
    pub fn holder_name(&self) -> Option<String> {
        json::member(&self.holder, "name")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IssueRecord {
    pub id: String,
    pub project: String,
    pub number: i64,
    pub title: String,
    pub status: String,
    pub category: Option<String>,
    pub priority: i64,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub parent: Option<String>,
    pub milestone: Option<String>,
    pub order: f64,
    pub due: Option<String>,
    pub description: Option<String>,
    pub acceptance: Option<String>,
    pub evidence: Option<String>,
    pub claim: Option<Claim>,
    pub revision: i64,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewIssue {
    pub project: String,
    pub title: String,
    pub status: String,
    pub priority: i64,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub parent: Option<String>,
    pub milestone: Option<String>,
    pub order: Option<f64>,
    pub due: Option<String>,
    pub description: Option<String>,
    pub acceptance: Option<String>,
    pub notes: Option<String>,
    pub client_key: Option<String>,
    pub client_request: Option<String>,
    pub created_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRecord {
    pub text: String,
    pub author: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyedIssue {
    pub id: String,
    pub request: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueCreated {
    Created(String),
    Existing(String),
}

impl IssueCreated {
    pub fn id(&self) -> &str {
        match self {
            IssueCreated::Created(id) | IssueCreated::Existing(id) => id,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct IssueFields {
    pub title: Option<String>,
    pub priority: Option<i64>,
    pub assignee: Option<Option<String>>,
    pub milestone: Option<Option<String>>,
    pub due: Option<Option<String>>,
    pub description: Option<Option<String>>,
    pub acceptance: Option<Option<String>>,
    pub evidence: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimFilter {
    /// Claims with this holder key, and the claims of this session when there is one.
    Mine {
        key: String,
        session: Option<HolderSession>,
    },
    Any,
    Unclaimed,
}

/// A harness's session id, which is only unique within that harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HolderSession {
    pub harness: Option<String>,
    pub session: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueFilter {
    pub project: Option<String>,
    pub statuses: Vec<String>,
    pub categories: Vec<String>,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub parent: Option<String>,
    pub claim: Option<ClaimFilter>,
    pub tool: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelationRecord {
    pub kind: String,
    pub direction: Direction,
    pub issue: String,
    pub title: String,
    pub status: String,
    pub category: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitNode {
    pub id: String,
    pub parent: Option<String>,
    pub status: String,
    pub category: Option<String>,
    pub project: String,
    pub claim_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub source: String,
    pub kind: String,
    pub target: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    pub issues: Vec<WaitNode>,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    Board,
    Project(String),
    Issue(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChange {
    pub field: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub commit: String,
    pub at: String,
    pub author: String,
    pub operation: String,
    pub reason: Option<String>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub project: String,
    pub issue: Option<String>,
    pub fields: Vec<FieldChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRecord {
    pub issue: String,
    pub tool: String,
    pub key: String,
    pub decision: bool,
    pub category: Option<String>,
    pub finished_evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FindingLink {
    pub kind: String,
    pub id: String,
}
