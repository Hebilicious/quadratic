use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{Refused, catalog};
use crate::domain::{
    self, Actor, Assignee, Category, Moves, NotReadyReason, Readiness, Workflow, WorkflowStatus,
};
use crate::store::{self, Direction, IssueRecord, RelationRecord};

fn parse<T: DeserializeOwned>(what: &str, text: &str) -> Result<T, Refused> {
    serde_json::from_str(text).map_err(|error| {
        Refused::defect(format!("the stored {what} {text:?} is not valid: {error}"))
    })
}

pub(crate) fn category(text: &str) -> Option<Category> {
    serde_json::from_value(Value::from(text)).ok()
}

fn known_category(owner: &str, category_text: Option<&str>) -> Result<Category, Refused> {
    category_text
        .and_then(category)
        .ok_or_else(|| Refused::defect(format!("{owner} is in a status its workflow lacks")))
}

pub(crate) fn actor(record: &str) -> Result<Actor, Refused> {
    parse("actor", record)
}

pub(crate) fn assignee_text(assignee: &Assignee) -> Result<String, Refused> {
    serde_json::to_string(assignee).map_err(Refused::defect)
}

pub(crate) fn workflow(stored: &store::Workflow) -> Result<Workflow, Refused> {
    let statuses = stored
        .statuses
        .iter()
        .map(|status| {
            Ok(WorkflowStatus {
                key: status.key.clone(),
                name: status.name.clone(),
                category: known_category(&status.key, Some(&status.category))?,
            })
        })
        .collect::<Result<_, Refused>>()?;
    Ok(Workflow {
        statuses,
        moves: parse::<Moves>("moves", &stored.moves)?,
    })
}

pub(crate) fn stored_workflow(workflow: &Workflow) -> Result<store::Workflow, Refused> {
    Ok(store::Workflow {
        statuses: workflow
            .statuses
            .iter()
            .map(|status| store::WorkflowStatus {
                key: status.key.clone(),
                name: status.name.clone(),
                category: serde_json::to_value(status.category)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default(),
            })
            .collect(),
        moves: serde_json::to_string(&workflow.moves).map_err(Refused::defect)?,
    })
}

pub(crate) fn domain_project(project: &store::Project) -> Result<domain::Project, Refused> {
    Ok(domain::Project {
        workflow: workflow(&project.workflow)?,
        default_branch: project.default_branch.clone(),
        repository: project.repository.clone(),
    })
}

pub(crate) fn domain_issue(record: &IssueRecord) -> Result<domain::Issue, Refused> {
    Ok(domain::Issue {
        id: record.id.clone(),
        status: record.status.clone(),
        category: known_category(&record.id, record.category.as_deref())?,
        revision: u64::try_from(record.revision).map_err(Refused::defect)?,
        claim: record.claim.as_ref().map(claim).transpose()?,
        assignee: record
            .assignee
            .as_deref()
            .map(|text| parse("assignee", text))
            .transpose()?,
        bases: Vec::new(),
    })
}

fn claim(stored: &store::Claim) -> Result<domain::Claim, Refused> {
    Ok(domain::Claim {
        holder: actor(&stored.holder)?,
        branch: stored.branch.clone(),
        worktree: stored.worktree.clone(),
        since: stored.since.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Project {
    key: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    default_branch: String,
    workflow: Workflow,
    extra: Value,
}

impl Project {
    pub(crate) fn of(project: &store::Project) -> Result<Project, Refused> {
        Ok(Project {
            key: project.key.clone(),
            name: project.name.clone(),
            repository: project.repository.clone(),
            default_branch: project.default_branch.clone(),
            workflow: workflow(&project.workflow)?,
            extra: parse("extra", &project.extra)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IssueSummary {
    pub(crate) id: String,
    pub(crate) project: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) category: Category,
    pub(crate) priority: i64,
    pub(crate) labels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) assignee: Option<Assignee>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) claim: Option<domain::Claim>,
    pub(crate) ready: bool,
    pub(crate) blocked_by: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) not_ready: Vec<NotReadyReason>,
    pub(crate) order: f64,
    pub(crate) revision: u64,
    pub(crate) updated_at: String,
}

impl IssueSummary {
    pub(crate) fn of(
        record: &IssueRecord,
        issue: &domain::Issue,
        readiness: Readiness,
    ) -> IssueSummary {
        let (not_ready, blocked_by) = match readiness {
            Readiness::Ready => (Vec::new(), Vec::new()),
            Readiness::NotReady { reasons, blockers } => (reasons, blockers),
        };
        IssueSummary {
            id: record.id.clone(),
            project: record.project.clone(),
            title: record.title.clone(),
            status: issue.status.clone(),
            category: issue.category,
            priority: record.priority,
            labels: record.labels.clone(),
            parent: record.parent.clone(),
            assignee: issue.assignee.clone(),
            claim: issue.claim.clone(),
            ready: not_ready.is_empty(),
            blocked_by,
            not_ready,
            order: record.order,
            revision: issue.revision,
            updated_at: record.updated_at.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Issue {
    #[serde(flatten)]
    pub(crate) summary: IssueSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) acceptance: Option<String>,
    pub(crate) notes: Vec<Note>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) evidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) milestone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) due: Option<String>,
    pub(crate) relations: Vec<Relation>,
    pub(crate) children: Vec<String>,
    pub(crate) created_at: String,
    pub(crate) created_by: Actor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<Source>,
    pub(crate) links: Vec<domain::Link>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Source {
    pub(crate) tool: String,
    pub(crate) key: String,
}

impl Source {
    pub(crate) fn of(record: store::SourceRecord) -> Source {
        Source {
            tool: record.tool,
            key: record.key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Note {
    text: String,
    author: String,
    at: String,
}

impl Note {
    pub(crate) fn shown() -> usize {
        catalog::max_items("Issue/properties/notes")
    }

    pub(crate) fn of(record: store::NoteRecord) -> Result<Note, Refused> {
        Ok(Note {
            author: actor(&record.author)?.name,
            text: record.text,
            at: record.at,
        })
    }
}

#[derive(Serialize)]
pub(crate) struct IssueAnswer<T> {
    pub(crate) issue: T,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Relation {
    kind: String,
    direction: &'static str,
    issue: String,
    title: String,
    status: String,
    category: Category,
}

impl Relation {
    pub(crate) fn of(record: &RelationRecord) -> Result<Relation, Refused> {
        Ok(Relation {
            kind: record.kind.clone(),
            direction: match record.direction {
                Direction::Outgoing => "outgoing",
                Direction::Incoming => "incoming",
            },
            issue: record.issue.clone(),
            title: record.title.clone(),
            status: record.status.clone(),
            category: known_category(&record.issue, record.category.as_deref())?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Change {
    commit: String,
    at: String,
    author: Actor,
    operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<String>,
    project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue: Option<String>,
    fields: Vec<FieldChange>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct FieldChange {
    field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<String>,
}

impl Change {
    pub(crate) fn of(change: store::Change) -> Result<Change, Refused> {
        Ok(Change {
            author: actor(&change.author)?,
            commit: change.commit,
            at: change.at,
            operation: change.operation,
            reason: change.reason,
            branch: change.branch,
            worktree: change.worktree,
            project: change.project,
            issue: change.issue,
            fields: change
                .fields
                .into_iter()
                .map(|field| FieldChange {
                    field: field.field,
                    before: field.before,
                    after: field.after,
                })
                .collect(),
        })
    }
}

#[derive(Serialize, Default)]
pub(crate) struct IssueIds {
    count: usize,
    issues: Vec<String>,
}

impl IssueIds {
    pub(crate) fn of(mut ids: Vec<String>) -> IssueIds {
        ids.sort_by_key(|id| issue_number(id));
        let count = ids.len();
        ids.truncate(catalog::max_items("IssueIds/properties/issues"));
        IssueIds { count, issues: ids }
    }
}

fn issue_number(id: &str) -> (String, u64) {
    match id.rsplit_once('-') {
        Some((project, number)) => (project.to_owned(), number.parse().unwrap_or(u64::MAX)),
        None => (id.to_owned(), u64::MAX),
    }
}
