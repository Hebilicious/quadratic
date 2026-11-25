use acid_cat_core::CapabilityDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ancestry::{Scope, verified};
use super::board::{self, Board};
use super::catalog::capability;
use super::human_word::HumanWord;
use super::issues::whole_issue;
use super::model::{self, Issue, IssueAnswer, IssueSummary};
use super::{Context, IssueReader, Refused, finish};
use crate::domain::{
    Actor, Caller, Change, Claim, ClaimAfter, Confidence, Event, Failure, Landing, Outcome,
    WorkingCopy, decide, decide_on_word,
};
use crate::git::repository::{GIT_LIMIT, Repository};
use crate::store::{AsOf, ClaimStart, IssueFields, Tx};

pub(super) fn apply(
    tx: &mut Tx<'_>,
    id: &str,
    change: &Change,
    evidence: Option<&str>,
) -> Result<(), Refused> {
    tx.set_status(id, &change.status)?;
    if matches!(change.claim, ClaimAfter::Started(_) | ClaimAfter::Ended) {
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
        ClaimAfter::Ended | ClaimAfter::Freed(_) => {
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
            Ok(Outcome::Unchanged | Outcome::LeftForHuman) => {
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
    Claim {
        worktree: Option<String>,
    },
    Release {
        reason: Option<String>,
        word: Option<HumanWord>,
    },
    Close {
        evidence: String,
        word: Option<HumanWord>,
    },
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
            Request::Close { evidence, .. } => Some(evidence),
            Request::Claim { .. } | Request::Release { .. } => None,
        }
    }

    fn reason(&self) -> Option<&str> {
        match self {
            Request::Release { reason, .. } => reason.as_deref(),
            Request::Claim { .. } | Request::Close { .. } => None,
        }
    }

    fn word(&self) -> Option<&HumanWord> {
        match self {
            Request::Release { word, .. } | Request::Close { word, .. } => word.as_ref(),
            Request::Claim { .. } => None,
        }
    }

    /// The reason of the history's change: the human's words when the call gives them.
    fn history_reason(&self) -> Option<String> {
        match self.word() {
            Some(word) => Some(word.history_reason()),
            None => self.reason().map(str::to_owned),
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
    let ((started, left_for_human), answered) = context
        .write_then_read(
            request.operation(),
            request.history_reason(),
            move |tx| {
                let record = board::issue(tx, &id)?;
                let project = model::domain_project(&board::project(tx, &record.project)?)?;
                let board = Board::load(tx)?;
                let issue = board.issue(&record)?;
                let event = request.event(tx, &board, &id);
                let word = request.word();
                let reason = request.reason();
                let confidence = HumanWord::confidence(word);
                let mut started = None;
                match decide_on_word(&event, &issue, &caller, &project, confidence)? {
                    Outcome::Unchanged => {}
                    Outcome::LeftForHuman => {
                        if let Some(word) = word {
                            word.note_left(tx, &id, &caller, reason)?;
                        }
                        return Ok(((None, true), id));
                    }
                    Outcome::Changed(change) => {
                        apply(tx, &id, &change, request.evidence())?;
                        if let ClaimAfter::Freed(former) = &change.claim {
                            note_freed(tx, &id, &caller, former, &change.status, reason)?;
                        }
                        if let Some(word) = word {
                            word.note_moved(tx, &id, &caller, &change.status, reason)?;
                        }
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
                }
                Ok(((started, false), id))
            },
            answer,
        )
        .await?;
    if let Some(started) = started {
        record_start(context, started).await;
    }
    Ok(IssueAnswer {
        issue: answered,
        left_for_human,
    })
}

/// The note an issue keeps when an agent that did not hold its claim released it.
fn note_freed(
    tx: &mut Tx<'_>,
    id: &str,
    caller: &Caller,
    former: &Claim,
    status: &str,
    reason: Option<&str>,
) -> Result<(), Refused> {
    let author = serde_json::to_string(&caller.actor).map_err(Refused::defect)?;
    let mut text = format!(
        "{} released the claim of {} (branch {}, worktree {}); the issue stays in {}.",
        caller.actor.name, former.holder.name, former.branch, former.worktree, status
    );
    if let Some(reason) = reason {
        text.push_str(&format!(" Reason: {reason}"));
    }
    tx.add_note(id, &text, &author)?;
    Ok(())
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
#[serde(rename_all = "camelCase")]
pub(super) struct ReleaseInput {
    issue: String,
    reason: Option<String>,
    human_words: Option<String>,
    confidence: Option<Confidence>,
}

pub(super) fn release()
-> CapabilityDefinition<Context, ReleaseInput, IssueAnswer<IssueSummary>, Value> {
    capability::<ReleaseInput, IssueAnswer<IssueSummary>>("issue.release").run(
        |input, call| async move {
            let caller = &call.context().caller;
            let request = Request::Release {
                reason: input.reason,
                word: HumanWord::given(caller, input.human_words, input.confidence),
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
#[serde(rename_all = "camelCase")]
pub(super) struct CloseInput {
    issue: String,
    evidence: String,
    human_words: Option<String>,
    confidence: Option<Confidence>,
}

pub(super) fn close() -> CapabilityDefinition<Context, CloseInput, IssueAnswer<Issue>, Value> {
    capability::<CloseInput, IssueAnswer<Issue>>("issue.close").run(|input, call| async move {
        let caller = &call.context().caller;
        let request = Request::Close {
            evidence: input.evidence,
            word: HumanWord::given(caller, input.human_words, input.confidence),
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CloseSeveralInput {
    issue: Vec<String>,
    evidence: String,
    human_words: Option<String>,
    confidence: Option<Confidence>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClosedSeveral {
    items: Vec<IssueSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    left_for_human: Vec<String>,
}

pub(super) fn close_several()
-> CapabilityDefinition<Context, CloseSeveralInput, ClosedSeveral, Value> {
    capability::<CloseSeveralInput, ClosedSeveral>("issues.close").run(|input, call| async move {
        let context = call.context();
        let word = HumanWord::given(&context.caller, input.human_words, input.confidence);
        finish(close_issues(context, input.issue, input.evidence, word).await)
    })
}

/// Closes each issue in order inside one write, so that a refusal of any of them changes
/// nothing.
async fn close_issues(
    context: &Context,
    ids: Vec<String>,
    evidence: String,
    word: Option<HumanWord>,
) -> Result<ClosedSeveral, Refused> {
    let caller = context.caller.clone();
    let reason = word.as_ref().map(HumanWord::history_reason);
    let written = context
        .write("issue.close", reason, {
            let ids = ids.clone();
            move |tx| {
                let mut left_for_human = Vec::new();
                for id in &ids {
                    let left = close_in(tx, id, &caller, &evidence, word.as_ref())
                        .map_err(|refused| not_closed(id, refused))?;
                    if left {
                        left_for_human.push(id.clone());
                    }
                }
                let items = ids
                    .iter()
                    .map(|id| board::summary(tx, id, &caller))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((items, left_for_human))
            }
        })
        .await?;
    let (items, left_for_human) = written.value;
    let Some(commit) = written.commit else {
        return Ok(ClosedSeveral {
            items,
            left_for_human,
        });
    };
    let caller = context.caller.clone();
    let items = context
        .read(AsOf::Commit(commit.hash), move |view| {
            ids.iter()
                .map(|id| board::summary(view, id, &caller))
                .collect::<Result<Vec<_>, _>>()
        })
        .await?;
    Ok(ClosedSeveral {
        items,
        left_for_human,
    })
}

/// Closes one issue; true when it was left for the human.
fn close_in(
    tx: &mut Tx<'_>,
    id: &str,
    caller: &Caller,
    evidence: &str,
    word: Option<&HumanWord>,
) -> Result<bool, Refused> {
    let record = board::issue(tx, id)?;
    let project = model::domain_project(&board::project(tx, &record.project)?)?;
    let issue = Board::load(tx)?.issue(&record)?;
    let confidence = HumanWord::confidence(word);
    match decide_on_word(&Event::Close, &issue, caller, &project, confidence)? {
        Outcome::Unchanged => Ok(false),
        Outcome::LeftForHuman => {
            if let Some(word) = word {
                word.note_left(tx, id, caller, None)?;
            }
            Ok(true)
        }
        Outcome::Changed(change) => {
            apply(tx, id, &change, Some(evidence))?;
            if let Some(word) = word {
                word.note_moved(tx, id, caller, &change.status, None)?;
            }
            Ok(false)
        }
    }
}

fn not_closed(id: &str, refused: Refused) -> Refused {
    match refused {
        Refused::Failure(failure) => Refused::Failure(json!({
            "_tag": "NotClosed",
            "issue": id,
            "failure": failure,
        })),
        other => other,
    }
}
