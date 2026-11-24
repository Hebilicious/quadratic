use std::collections::{BTreeMap, BTreeSet};

use acid_cat_core::CapabilityDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::board::{self, caller_project};
use super::catalog::capability;
use super::lifecycle::apply;
use super::model::{self, IssueIds};
use super::{Context, Refused, finish};
use crate::domain::{Event, Failure, Outcome, decide};
use crate::git::repository::{GIT_LIMIT, GitError, Repository};
use crate::git::watcher::{Confirmed, LookFailed};
use crate::store::AsOf;

const OPERATION: &str = "issue.advance";
const MOST_TEXT: usize = 500;

#[derive(Deserialize)]
pub(super) struct AdvanceInput {
    project: Option<String>,
    commit: String,
    status: String,
    reason: Option<String>,
}

#[derive(Serialize)]
pub(super) struct Advanced {
    moved: IssueIds,
    left: IssueIds,
}

pub(super) fn advance() -> CapabilityDefinition<Context, AdvanceInput, Advanced, Value> {
    capability::<AdvanceInput, Advanced>(OPERATION)
        .run(|input, call| async move { finish(advance_to(call.context(), input).await) })
}

async fn advance_to(context: &Context, input: AdvanceInput) -> Result<Advanced, Refused> {
    let caller = context.caller.clone();
    let (project, status) = (input.project.clone(), input.status.clone());
    let key = context
        .read(AsOf::Now, move |view| {
            let project = caller_project(view, project.as_deref(), &caller)?;
            model::workflow(&project.workflow)?.after_merge(&status)?;
            Ok(project.repository.is_some().then_some(project.key))
        })
        .await?
        .ok_or_else(|| not_in_repository(&input.commit))?;
    let looks = context
        .looks
        .as_ref()
        .ok_or_else(|| Refused::defect("this context has no ref watcher to look with"))?;
    let confirmed = looks.confirm(&key).await.map_err(scan_failed)?;
    let reachable = deployed_history(&confirmed, &input.commit).await?;
    let reason = reason(&input);
    let caller = context.caller.clone();
    let written = context
        .write(OPERATION, Some(reason), move |tx| {
            let project = board::project(tx, &key)?;
            if project.repository.as_deref() != Some(confirmed.repository.as_str())
                || project.default_branch != confirmed.default_branch
            {
                return Err(Refused::Failure(
                    json!({ "_tag": "RepositoryChanged", "project": key }),
                ));
            }
            let domain_project = model::domain_project(&project)?;
            let workflow = &domain_project.workflow;
            workflow.after_merge(&input.status)?;
            let mut landed_up_to: BTreeMap<String, bool> = BTreeMap::new();
            for landing in tx.git_landings(&key)? {
                let in_history = reachable.contains(&landing.git_commit);
                *landed_up_to.entry(landing.issue).or_insert(true) &= in_history;
            }
            let event = Event::Advance {
                status: input.status.clone(),
            };
            let (mut moved, mut left) = (Vec::new(), Vec::new());
            for (id, every_landing_deployed) in landed_up_to {
                let issue = model::domain_issue(&board::issue(tx, &id)?)?;
                if issue.category.is_resolved() || !workflow.is_before(&issue.status, &input.status)
                {
                    continue;
                }
                let outcome = if every_landing_deployed {
                    decide(&event, &issue, &caller, &domain_project)
                } else {
                    Ok(Outcome::Unchanged)
                };
                match outcome {
                    Ok(Outcome::Changed(change)) => {
                        apply(tx, &id, &change, None)?;
                        moved.push(id);
                    }
                    Ok(Outcome::Unchanged) | Err(Failure::HumanOnly | Failure::Held { .. }) => {
                        left.push(id);
                    }
                    Err(failure) => return Err(failure.into()),
                }
            }
            Ok(Advanced {
                moved: IssueIds::of(moved),
                left: IssueIds::of(left),
            })
        })
        .await?;
    Ok(written.value)
}

async fn deployed_history(
    confirmed: &Confirmed,
    commit: &str,
) -> Result<BTreeSet<String>, Refused> {
    let git = Repository::at(&confirmed.repository, GIT_LIMIT);
    if git.commit_of(commit).await.map_err(git_failed)?.as_deref() != Some(commit) {
        return Err(not_in_repository(commit));
    }
    let mut on_default_branch = false;
    for tip in confirmed.tips.local.iter().chain(&confirmed.tips.upstream) {
        if git.is_ancestor(commit, tip).await.map_err(git_failed)? {
            on_default_branch = true;
            break;
        }
    }
    if !on_default_branch {
        return Err(Refused::Failure(
            json!({ "_tag": "NotOnDefaultBranch", "commit": commit }),
        ));
    }
    git.reachable_from(commit).await.map_err(git_failed)
}

fn reason(input: &AdvanceInput) -> String {
    let said = match &input.reason {
        Some(reason) => format!(
            "to {} at git commit {}: {reason}",
            input.status, input.commit
        ),
        None => format!("to {} at git commit {}", input.status, input.commit),
    };
    at_most(said)
}

fn at_most(text: String) -> String {
    match text.char_indices().nth(MOST_TEXT) {
        Some((cut, _)) => text[..cut].to_owned(),
        None => text,
    }
}

fn not_in_repository(commit: &str) -> Refused {
    Refused::Failure(json!({ "_tag": "NotInRepository", "commit": commit }))
}

fn scan_failed(failed: LookFailed) -> Refused {
    Refused::Failure(json!({ "_tag": "ScanFailed", "problem": at_most(failed.to_string()) }))
}

fn git_failed(error: GitError) -> Refused {
    scan_failed(LookFailed::from(error))
}
