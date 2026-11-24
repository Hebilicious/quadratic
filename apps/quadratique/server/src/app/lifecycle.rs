use acid_cat_core::CapabilityDefinition;
use serde::Deserialize;
use serde_json::Value;

use super::ancestry::{Scope, verified};
use super::board::{self, Board};
use super::catalog::capability;
use super::issues::whole_issue;
use super::model::{self, Issue, IssueAnswer, IssueSummary};
use super::{Context, IssueReader, Refused, finish};
use crate::domain::{
    Actor, Caller, Change, ClaimAfter, Event, Failure, Landing, Outcome, WorkingCopy, decide,
};
use crate::git::repository::{GIT_LIMIT, Repository};
use crate::store::{ClaimStart, IssueFields, Tx};

pub(super) fn apply(
    tx: &mut Tx<'_>,
    id: &str,
    change: &Change,
    evidence: Option<&str>,
) -> Result<(), Refused> {
    tx.set_status(id, &change.status)?;
    if change.claim != ClaimAfter::Kept {
        tx.end_held_merge(id)?;
    }
    match &change.claim {
        ClaimAfter::Kept => {}
        ClaimAfter::Started(claim) => {
            let holder = serde_json::to_string(&claim.holder).map_err(Refused::defect)?;
            let key = claim.holder_key();
            if !tx.take_claim(id, &holder, key.as_str(), &claim.branch, &claim.worktree)? {
                return Err(Refused::defect(format!(
                    "{id} was claimed inside the write that claims it"
                )));
            }
        }
        ClaimAfter::Ended => {
            tx.clear_claim(id)?;
        }
    }
    if change.takes_evidence {
        tx.update_fields(
            id,
            &IssueFields {
                evidence: Some(evidence.map(str::to_owned)),
                ..IssueFields::default()
            },
        )?;
    }
    release_held_on(tx, id)
}

/// Releases the held issues stacked on `base`, once it moved.
pub(super) fn release_held_on(tx: &mut Tx<'_>, base: &str) -> Result<(), Refused> {
    if tx.held_merges()?.is_empty() {
        return Ok(());
    }
    let stacked = Board::load(tx)?.stacked_on(base);
    release_held(tx, stacked)
}

/// An issue whose landing its bases held back moves to the workflow's `merge` status once
/// nothing holds it back any more.
pub(super) fn release_held(tx: &mut Tx<'_>, issues: Vec<String>) -> Result<(), Refused> {
    let held = tx.held_merges()?;
    let issues: Vec<String> = issues.into_iter().filter(|id| held.contains(id)).collect();
    if issues.is_empty() {
        return Ok(());
    }
    let board = Board::load(tx)?;
    let server = Caller {
        actor: Actor::server(),
        working_copy: WorkingCopy::default(),
    };
    let event = Event::Merge {
        landing: Landing::Held,
    };
    for id in issues {
        let record = board::issue(tx, &id)?;
        let project = model::domain_project(&board::project(tx, &record.project)?)?;
        match decide(&event, &board.issue(&record)?, &server, &project) {
            Ok(Outcome::Changed(change)) => {
                tx.end_held_merge(&id)?;
                apply(tx, &id, &change, None)?;
            }
            Ok(Outcome::Unchanged) => {
                tx.end_held_merge(&id)?;
            }
            Err(Failure::BaseNotMerged { .. }) => {}
            Err(failure) => return Err(failure.into()),
        }
    }
    Ok(())
}

#[derive(Clone)]
enum Request {
    Claim { worktree: Option<String> },
    Release { reason: Option<String> },
    Close { evidence: String },
}

impl Request {
    fn operation(&self) -> &'static str {
        match self {
            Request::Claim { .. } => "issue.claim",
            Request::Release { .. } => "issue.release",
            Request::Close { .. } => "issue.close",
        }
    }

    fn event(&self, tx: &Tx<'_>, board: &Board, id: &str) -> Event {
        match self {
            Request::Claim { worktree } => Event::Claim {
                waits: board.waits(id),
                at: tx.now().to_owned(),
                worktree: worktree.clone(),
            },
            Request::Release { .. } => Event::Release,
            Request::Close { .. } => Event::Close,
        }
    }

    fn evidence(&self) -> Option<&str> {
        match self {
            Request::Close { evidence } => Some(evidence),
            Request::Claim { .. } | Request::Release { .. } => None,
        }
    }
}

async fn move_issue<R: Send + 'static>(
    context: &Context,
    caller: Caller,
    id: String,
    request: Request,
    answer: IssueReader<R>,
) -> Result<IssueAnswer<R>, Refused> {
    let reason = match &request {
        Request::Release { reason } => reason.clone(),
        Request::Claim { .. } | Request::Close { .. } => None,
    };
    let (started, answered) = context
        .write_then_read(
            request.operation(),
            reason,
            move |tx| {
                let record = board::issue(tx, &id)?;
                let project = model::domain_project(&board::project(tx, &record.project)?)?;
                let board = Board::load(tx)?;
                let issue = board.issue(&record)?;
                let event = request.event(tx, &board, &id);
                let mut started = None;
                if let Outcome::Changed(change) = decide(&event, &issue, &caller, &project)? {
                    apply(tx, &id, &change, request.evidence())?;
                    let in_bound_repository = project.repository.is_some()
                        && project.repository == caller.working_copy.repository;
                    if let ClaimAfter::Started(claim) = &change.claim
                        && in_bound_repository
                    {
                        started = Some(Started {
                            issue: id.clone(),
                            claim_key: claim.holder_key().as_str().to_owned(),
                            since: tx.now().to_owned(),
                        });
                    }
                }
                Ok((started, id))
            },
            answer,
        )
        .await?;
    if let Some(started) = started {
        record_start(context, started).await;
    }
    Ok(IssueAnswer { issue: answered })
}

struct Started {
    issue: String,
    claim_key: String,
    since: String,
}

async fn record_start(context: &Context, started: Started) {
    let working_copy = &context.caller.working_copy;
    let (Some(repository), Some(branch)) = (&working_copy.repository, &working_copy.branch) else {
        return;
    };
    let Ok(Some(tip)) = Repository::at(repository, GIT_LIMIT).tip(branch).await else {
        return;
    };
    let start = ClaimStart {
        issue: started.issue,
        claim_key: started.claim_key,
        since: started.since,
        repository: repository.clone(),
        tip,
    };
    let _ = context
        .write("claim.start", None, move |tx| {
            Ok(tx.record_claim_start(&start)?)
        })
        .await;
}

#[derive(Deserialize)]
pub(super) struct ClaimInput {
    issue: String,
    worktree: Option<String>,
}

pub(super) fn claim() -> CapabilityDefinition<Context, ClaimInput, IssueAnswer<Issue>, Value> {
    capability::<ClaimInput, IssueAnswer<Issue>>("issue.claim").run(|input, call| async move {
        finish(claim_issue(call.context(), input.issue, input.worktree).await)
    })
}

/// A claim is decided first without git. Only when that refuses it for `stacked`, and a base
/// in review asks git about the caller's branch, is it decided again with git's answer.
async fn claim_issue(
    context: &Context,
    id: String,
    worktree: Option<String>,
) -> Result<IssueAnswer<Issue>, Refused> {
    let request = Request::Claim { worktree };
    let first = move_issue(
        context,
        context.caller.clone(),
        id.clone(),
        request.clone(),
        whole_issue,
    )
    .await;
    match first {
        Err(Refused::Failure(failure)) if refused_for_stacked(&failure) => {
            let caller = verified(context, Scope::Issue(id.clone())).await?;
            if caller.working_copy.contains.is_none() {
                return Err(Refused::Failure(failure));
            }
            move_issue(context, caller, id, request, whole_issue).await
        }
        other => other,
    }
}

fn refused_for_stacked(failure: &Value) -> bool {
    failure["_tag"] == "NotReady"
        && failure["reasons"]
            .as_array()
            .is_some_and(|reasons| reasons.iter().any(|reason| reason == "stacked"))
}

#[derive(Deserialize)]
pub(super) struct ReleaseInput {
    issue: String,
    reason: Option<String>,
}

pub(super) fn release()
-> CapabilityDefinition<Context, ReleaseInput, IssueAnswer<IssueSummary>, Value> {
    capability::<ReleaseInput, IssueAnswer<IssueSummary>>("issue.release").run(
        |input, call| async move {
            let request = Request::Release {
                reason: input.reason,
            };
            finish(
                move_issue(
                    call.context(),
                    call.context().caller.clone(),
                    input.issue,
                    request,
                    board::summary,
                )
                .await,
            )
        },
    )
}

#[derive(Deserialize)]
pub(super) struct CloseInput {
    issue: String,
    evidence: String,
}

pub(super) fn close() -> CapabilityDefinition<Context, CloseInput, IssueAnswer<Issue>, Value> {
    capability::<CloseInput, IssueAnswer<Issue>>("issue.close").run(|input, call| async move {
        let request = Request::Close {
            evidence: input.evidence,
        };
        finish(
            move_issue(
                call.context(),
                call.context().caller.clone(),
                input.issue,
                request,
                whole_issue,
            )
            .await,
        )
    })
}
