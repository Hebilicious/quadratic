use acid_cat_core::CapabilityDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ancestry::{Scope, verified};
use super::board::{self, Board, all_issues, caller_project, require_issue};
use super::catalog::capability;
use super::cursor::Listing;
use super::lifecycle::apply;
use super::model::{self, Issue, IssueAnswer, IssueSummary, Note, Relation, Source};
use super::{Context, Refused, finish};
use crate::domain::{
    Assignee, Caller, Category, Event, Link, Outcome, RelationChange, create_status, decide,
};
use crate::store::{AsOf, ClaimFilter, IssueCreated, IssueFields, IssueFilter, NewIssue, View};

pub(super) fn as_of(text: Option<String>) -> AsOf {
    match text {
        None => AsOf::Now,
        Some(text) if text.bytes().all(|byte| byte.is_ascii_hexdigit()) => AsOf::Commit(text),
        Some(text) => AsOf::Time(text),
    }
}

#[derive(Serialize)]
pub(super) struct Items {
    items: Vec<IssueSummary>,
}

#[derive(Deserialize)]
pub(super) struct ReadyInput {
    project: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default = "ready_limit")]
    limit: usize,
}

fn ready_limit() -> usize {
    5
}

pub(super) fn ready() -> CapabilityDefinition<Context, ReadyInput, Items, Value> {
    capability::<ReadyInput, Items>("issue.ready")
        .run(|input, call| async move { finish(ready_issues(call.context(), input).await) })
}

async fn ready_issues(context: &Context, input: ReadyInput) -> Result<Items, Refused> {
    let caller = verified(context, Scope::Every).await?;
    context
        .read(AsOf::Now, move |view| {
            let project = caller_project(view, input.project.as_deref(), &caller)?;
            let filter = IssueFilter {
                project: Some(project.key),
                labels: input.labels,
                ..IssueFilter::default()
            };
            let board = Board::load(view)?;
            let mut items = Vec::new();
            for record in all_issues(view, &filter)? {
                let summary = board.summary(&record, &caller)?;
                if summary.ready {
                    items.push(summary);
                }
            }
            items.sort_by_key(|issue| (issue.assignee.is_none(), urgency(issue.priority)));
            items.truncate(input.limit);
            Ok(Items { items })
        })
        .await
}

fn urgency(priority: i64) -> i64 {
    if priority == 0 { i64::MAX } else { priority }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct IssuePage {
    items: Vec<IssueSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ListInput {
    project: Option<String>,
    #[serde(default)]
    status: Vec<String>,
    #[serde(default)]
    category: Vec<String>,
    #[serde(default)]
    labels: Vec<String>,
    assignee: Option<Assignee>,
    claim: Option<String>,
    ready: Option<bool>,
    parent: Option<String>,
    tool: Option<String>,
    text: Option<String>,
    as_of: Option<String>,
    #[serde(default = "list_limit")]
    limit: usize,
    cursor: Option<String>,
}

fn list_limit() -> usize {
    50
}

pub(super) fn list() -> CapabilityDefinition<Context, ListInput, IssuePage, Value> {
    capability::<ListInput, IssuePage>("issue.list")
        .run(|input, call| async move { finish(list_issues(call.context(), input).await) })
}

async fn list_issues(context: &Context, input: ListInput) -> Result<IssuePage, Refused> {
    let caller = match input.as_of {
        None => verified(context, Scope::Every).await?,
        Some(_) => context.caller.clone(),
    };
    let mine = ClaimFilter::HeldBy(caller.holder_key().as_str().to_owned());
    let claim = match input.claim.as_deref() {
        Some("mine") => Some(mine),
        Some("any") => Some(ClaimFilter::Any),
        Some("none") => Some(ClaimFilter::Unclaimed),
        _ => None,
    };
    let assignee = input
        .assignee
        .as_ref()
        .map(model::assignee_text)
        .transpose()?;
    let caller_matters = input.ready.is_some() || input.claim.as_deref() == Some("mine");
    let listing = Listing::of(&json!({
        "listing": "issue.list",
        "project": input.project,
        "status": input.status,
        "category": input.category,
        "labels": input.labels,
        "assignee": input.assignee,
        "claim": input.claim,
        "ready": input.ready,
        "parent": input.parent,
        "tool": input.tool,
        "text": input.text,
        "asOf": input.as_of,
        "caller": caller_matters.then(|| json!({
            "actor": caller.actor,
            "worktree": caller.working_copy.worktree,
        })),
    }));
    let cursor = listing.position(input.cursor)?;
    context
        .read(as_of(input.as_of), move |view| {
            if let Some(key) = &input.project {
                board::project(view, key)?;
            }
            if let Some(parent) = &input.parent {
                require_issue(view, parent)?;
            }
            let filter = IssueFilter {
                project: input.project,
                statuses: input.status,
                categories: input.category,
                labels: input.labels,
                assignee,
                parent: input.parent,
                claim,
                tool: input.tool,
                text: input.text,
            };
            let board = Board::load(view)?;
            let mut kept: Vec<(IssueSummary, String)> = Vec::new();
            let mut more = false;
            let mut cursor = cursor;
            'pages: loop {
                let page = view.issues(&filter, cursor.as_deref(), input.limit)?;
                for record in &page.items {
                    let summary = board.summary(record, &caller)?;
                    if input.ready.is_some_and(|ready| ready != summary.ready) {
                        continue;
                    }
                    if kept.len() == input.limit {
                        more = true;
                        break 'pages;
                    }
                    kept.push((summary, record.cursor()));
                }
                match page.next_cursor {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
            let next_cursor = if more {
                kept.last()
                    .map(|(_, position)| listing.bind(position.clone()))
            } else {
                None
            };
            Ok(IssuePage {
                items: kept.into_iter().map(|(summary, _)| summary).collect(),
                next_cursor,
            })
        })
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GetInput {
    issue: String,
    as_of: Option<String>,
}

pub(super) fn get() -> CapabilityDefinition<Context, GetInput, IssueAnswer<Issue>, Value> {
    capability::<GetInput, IssueAnswer<Issue>>("issue.get")
        .run(|input, call| async move { finish(get_issue(call.context(), input).await) })
}

async fn get_issue(context: &Context, input: GetInput) -> Result<IssueAnswer<Issue>, Refused> {
    if input.as_of.is_none() {
        look_at_repository_of(context, &input.issue).await;
    }
    let caller = match input.as_of {
        None => verified(context, Scope::Issue(input.issue.clone())).await?,
        Some(_) => context.caller.clone(),
    };
    let issue = context
        .read(as_of(input.as_of), move |view| {
            whole_issue(view, &input.issue, &caller)
        })
        .await?;
    Ok(IssueAnswer { issue })
}

async fn look_at_repository_of(context: &Context, id: &str) {
    let Some(looks) = &context.looks else {
        return;
    };
    let id = id.to_owned();
    let watched = context
        .read(AsOf::Now, move |view| {
            let record = board::issue(view, &id)?;
            let started = model::domain_issue(&record)?.category == Category::Started;
            let project = board::project(view, &record.project)?;
            Ok((started && project.repository.is_some()).then_some(project.key))
        })
        .await;
    if let Ok(Some(project)) = watched {
        looks.project(&project).await;
    }
}

pub(super) fn whole_issue(view: &View<'_>, id: &str, caller: &Caller) -> Result<Issue, Refused> {
    let record = board::issue(view, id)?;
    let summary = Board::load(view)?.summary(&record, caller)?;
    let relations = view
        .relations(&record.id)?
        .iter()
        .map(Relation::of)
        .collect::<Result<_, _>>()?;
    Ok(Issue {
        summary,
        children: view.children(&record.id)?,
        relations,
        created_by: model::actor(&record.created_by)?,
        created_at: record.created_at,
        description: record.description,
        acceptance: record.acceptance,
        notes: view
            .notes(&record.id, Note::shown())?
            .into_iter()
            .map(Note::of)
            .collect::<Result<_, _>>()?,
        evidence: record.evidence,
        milestone: record.milestone,
        due: record.due,
        source: view.source(&record.id)?.map(Source::of),
        links: view
            .finding_links(&record.id)?
            .into_iter()
            .map(|link| Link {
                kind: link.kind,
                id: link.id,
            })
            .collect(),
    })
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateInput {
    key: Option<String>,
    project: Option<String>,
    title: String,
    description: Option<String>,
    acceptance: String,
    notes: Option<String>,
    status: Option<String>,
    #[serde(default)]
    priority: i64,
    #[serde(default)]
    labels: Vec<String>,
    parent: Option<String>,
    origin: Option<String>,
    assignee: Option<Assignee>,
    milestone: Option<String>,
    due: Option<String>,
}

#[derive(Serialize)]
pub(super) struct Created {
    issue: IssueSummary,
    created: bool,
}

pub(super) fn create() -> CapabilityDefinition<Context, CreateInput, Created, Value> {
    capability::<CreateInput, Created>("issue.create")
        .run(|input, call| async move { finish(create_issue(call.context(), input).await) })
}

async fn create_issue(context: &Context, input: CreateInput) -> Result<Created, Refused> {
    let caller = context.caller.clone();
    let created_by = serde_json::to_string(&caller.actor).map_err(Refused::defect)?;
    let request = serde_json::to_value(&input).map_err(Refused::defect)?;
    let assignee = input
        .assignee
        .as_ref()
        .map(model::assignee_text)
        .transpose()?;
    let (created, issue) = context
        .write_issue("issue.create", None, move |tx| {
            if let Some(key) = &input.key
                && let Some(keyed) = tx.keyed_issue(key)?
            {
                return if same_request(keyed.request.as_deref(), &request)? {
                    Ok((false, keyed.id))
                } else {
                    Err(Refused::Failure(
                        json!({ "_tag": "KeyConflict", "issue": keyed.id }),
                    ))
                };
            }
            let project = caller_project(tx, input.project.as_deref(), &caller)?;
            let workflow = model::workflow(&project.workflow)?;
            let status = create_status(&workflow, input.status.as_deref())?;
            for linked in input.parent.iter().chain(&input.origin) {
                require_issue(tx, linked)?;
            }
            let created = tx.create_issue(&NewIssue {
                project: project.key.clone(),
                title: input.title.clone(),
                status,
                priority: input.priority,
                labels: input.labels.clone(),
                assignee,
                parent: input.parent.clone(),
                milestone: input.milestone.clone(),
                order: None,
                due: input.due.clone(),
                description: input.description.clone(),
                acceptance: Some(input.acceptance.clone()),
                notes: input.notes.clone(),
                client_key: input.key.clone(),
                client_request: input.key.is_some().then(|| request.to_string()),
                created_by,
            })?;
            let IssueCreated::Created(id) = created else {
                return Err(Refused::defect(format!(
                    "the key of {} was taken inside the write that looked it up",
                    created.id()
                )));
            };
            if let Some(origin) = &input.origin {
                tx.add_relation(&id, "discovered-from", origin)?;
            }
            Ok((true, id))
        })
        .await?;
    Ok(Created { issue, created })
}

fn same_request(stored: Option<&str>, request: &Value) -> Result<bool, Refused> {
    let Some(stored) = stored else {
        return Ok(false);
    };
    let stored: Value = serde_json::from_str(stored).map_err(Refused::defect)?;
    Ok(stored == *request)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateInput {
    issue: String,
    revision: u64,
    title: Option<String>,
    status: Option<String>,
    priority: Option<i64>,
    labels: Option<Vec<String>>,
    parent: Option<String>,
    assignee: Option<Assignee>,
    milestone: Option<String>,
    due: Option<String>,
    description: Option<String>,
    acceptance: Option<String>,
    notes: Option<String>,
    #[serde(default)]
    clear: Vec<String>,
    order: Option<f64>,
    reason: Option<String>,
}

impl UpdateInput {
    fn given_or_cleared<T>(&self, field: &str, given: &Option<T>) -> Option<Option<T>>
    where
        T: Clone,
    {
        match given {
            Some(value) => Some(Some(value.clone())),
            None if self.clear.iter().any(|cleared| cleared == field) => Some(None),
            None => None,
        }
    }
}

pub(super) fn update()
-> CapabilityDefinition<Context, UpdateInput, IssueAnswer<IssueSummary>, Value> {
    capability::<UpdateInput, IssueAnswer<IssueSummary>>("issue.update")
        .run(|input, call| async move { finish(update_issue(call.context(), input).await) })
}

async fn update_issue(
    context: &Context,
    input: UpdateInput,
) -> Result<IssueAnswer<IssueSummary>, Refused> {
    let caller = context.caller.clone();
    let author = serde_json::to_string(&caller.actor).map_err(Refused::defect)?;
    let reason = input.reason.clone();
    let ((), issue) = context
        .write_issue("issue.update", reason, move |tx| {
            let record = board::issue(tx, &input.issue)?;
            let project = model::domain_project(&board::project(tx, &record.project)?)?;
            let issue = Board::load(tx)?.issue(&record)?;
            let assignee = input.given_or_cleared("assignee", &input.assignee);
            let event = Event::Update {
                revision: input.revision,
                status: input.status.clone(),
                changes_assignee: assignee
                    .as_ref()
                    .is_some_and(|assignee| *assignee != issue.assignee),
            };
            let outcome = decide(&event, &issue, &caller, &project)?;
            match input.given_or_cleared("parent", &input.parent) {
                Some(Some(parent)) => {
                    require_issue(tx, &parent)?;
                    Board::load(tx)?.check(&RelationChange::Parent {
                        child: record.id.clone(),
                        parent: parent.clone(),
                    })?;
                    tx.set_parent(&record.id, Some(&parent))?;
                }
                Some(None) => {
                    tx.set_parent(&record.id, None)?;
                }
                None => {}
            }
            tx.update_fields(
                &record.id,
                &IssueFields {
                    title: input.title.clone(),
                    priority: input.priority,
                    assignee: assignee
                        .map(|assignee| assignee.as_ref().map(model::assignee_text).transpose())
                        .transpose()?,
                    milestone: input.given_or_cleared("milestone", &input.milestone),
                    due: input.given_or_cleared("due", &input.due),
                    description: input.given_or_cleared("description", &input.description),
                    acceptance: input.acceptance.clone().map(Some),
                    evidence: None,
                },
            )?;
            if let Some(labels) = &input.labels {
                tx.set_labels(&record.id, labels)?;
            }
            if let Some(note) = &input.notes {
                tx.add_note(&record.id, note, &author)?;
            }
            if let Some(order) = input.order {
                tx.set_order(&record.id, order)?;
            }
            if let Outcome::Changed(change) = outcome {
                apply(tx, &record.id, &change, None)?;
            }
            Ok(((), record.id))
        })
        .await?;
    Ok(IssueAnswer { issue })
}
