use std::collections::BTreeSet;
use std::path::Path;

use acid_cat_core::{CapabilityDefinition, NoInput};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::board::{Board, all_issues};
use super::catalog::{capability, capability_without_input};
use super::lifecycle::release_held;
use super::model::{self, Project};
use super::{Context, Refused, finish};
use crate::domain::{Failure, Workflow, check_project_update};
use crate::git::default_branch::named_by;
use crate::store::{self, AsOf, IssueFilter, Kind, ProjectChanges, Tx};

#[derive(Serialize)]
pub(super) struct ProjectList {
    items: Vec<Project>,
}

pub(super) fn list() -> CapabilityDefinition<Context, NoInput, ProjectList, Value> {
    capability_without_input::<ProjectList>("project.list")
        .run(|(), call| async move { finish(list_projects(call.context()).await) })
}

async fn list_projects(context: &Context) -> Result<ProjectList, Refused> {
    context
        .read(AsOf::Now, |view| {
            let items = view
                .projects()?
                .iter()
                .map(Project::of)
                .collect::<Result<_, _>>()?;
            Ok(ProjectList { items })
        })
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateInput {
    key: String,
    name: String,
    default_branch: Option<String>,
    extra: Option<Value>,
}

pub(super) fn create(
    default_workflow: Workflow,
) -> CapabilityDefinition<Context, CreateInput, Project, Value> {
    capability::<CreateInput, Project>("project.create").run(move |input, call| {
        let workflow = default_workflow.clone();
        async move { finish(create_project(call.context(), input, &workflow).await) }
    })
}

async fn create_project(
    context: &Context,
    input: CreateInput,
    workflow: &Workflow,
) -> Result<Project, Refused> {
    let repository = context.caller.working_copy.repository.clone();
    let default_branch = input
        .default_branch
        .or_else(|| {
            repository
                .as_deref()
                .and_then(|repository| named_by(Path::new(repository)))
                .map(|(branch, _)| branch)
        })
        .unwrap_or_else(|| "main".into());
    let project = store::Project {
        key: input.key,
        name: input.name,
        repository,
        default_branch,
        extra: input
            .extra
            .unwrap_or_else(|| Value::Object(Default::default()))
            .to_string(),
        workflow: model::stored_workflow(workflow)?,
    };
    project_written(context, "project.create", None, move |tx| {
        tx.create_project(&project)?;
        Ok(project.key)
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateInput {
    project: String,
    name: Option<String>,
    default_branch: Option<String>,
    #[serde(default)]
    bind_repository: bool,
    workflow: Option<Workflow>,
    extra: Option<Value>,
    reason: Option<String>,
}

pub(super) fn update() -> CapabilityDefinition<Context, UpdateInput, Project, Value> {
    capability::<UpdateInput, Project>("project.update")
        .run(|input, call| async move { finish(update_project(call.context(), input).await) })
}

async fn update_project(context: &Context, input: UpdateInput) -> Result<Project, Refused> {
    if !context.caller.is_human() {
        return Err(Failure::HumanOnly.into());
    }
    let repository = match (
        input.bind_repository,
        &context.caller.working_copy.repository,
    ) {
        (false, _) => None,
        (true, Some(repository)) => Some(repository.clone()),
        (true, None) => return Err(Failure::NoWorkingCopy.into()),
    };
    let reason = input.reason.clone();
    project_written(context, "project.update", reason, move |tx| {
        let Some(current) = tx.project(&input.project)? else {
            return Err(Refused::not_found(Kind::Project, &input.project));
        };
        let current_rules = model::domain_project(&current)?;
        let mut proposed = current_rules.clone();
        if let Some(workflow) = &input.workflow {
            proposed.workflow = workflow.clone();
        }
        if let Some(branch) = &input.default_branch {
            proposed.default_branch = branch.clone();
        }
        if let Some(repository) = &repository {
            proposed.repository = Some(repository.clone());
        }
        let filter = IssueFilter {
            project: Some(current.key.clone()),
            ..IssueFilter::default()
        };
        let issues = all_issues(tx, &filter)?
            .iter()
            .map(model::domain_issue)
            .collect::<Result<Vec<_>, _>>()?;
        check_project_update(&current_rules, &proposed, &issues)?;
        tx.update_project(
            &current.key,
            &ProjectChanges {
                name: input.name,
                default_branch: input.default_branch,
                repository: repository.map(Some),
                extra: input.extra.map(|extra| extra.to_string()),
            },
        )?;
        if let Some(workflow) = &input.workflow {
            let before = Board::load(tx)?.out_of_order(&current.key);
            tx.replace_workflow(&current.key, &model::stored_workflow(workflow)?)?;
            keep_stacks_in_order(tx, &current.key, &before)?;
        }
        Ok(current.key)
    })
    .await
}

/// A new workflow may not put a stacked issue at the `merge` status or later ahead of a base
/// where the current workflow had them in order; a pair already out of order stays allowed. A
/// held landing whose bases now count as merged moves on.
fn keep_stacks_in_order(
    tx: &mut Tx<'_>,
    project: &str,
    before: &BTreeSet<(String, String)>,
) -> Result<(), Refused> {
    let after = Board::load(tx)?.out_of_order(project);
    let mut issues: Vec<String> = after
        .difference(before)
        .map(|(issue, _)| issue.clone())
        .collect();
    issues.dedup();
    if !issues.is_empty() {
        return Err(Failure::MergedBeforeBase { issues }.into());
    }
    let held = tx.held_merges()?;
    release_held(tx, held)
}

async fn project_written<F>(
    context: &Context,
    operation: &str,
    reason: Option<String>,
    work: F,
) -> Result<Project, Refused>
where
    F: for<'c> FnOnce(&mut Tx<'c>) -> Result<String, Refused> + Send + 'static,
{
    context
        .write(operation, reason, move |tx| {
            let key = work(tx)?;
            match tx.project(&key)? {
                Some(project) => Project::of(&project),
                None => Err(Refused::not_found(Kind::Project, &key)),
            }
        })
        .await
        .map(|written| written.value)
}
