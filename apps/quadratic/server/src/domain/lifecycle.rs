use super::failure::Failure;
use super::issue::{Caller, Claim, Issue, Project};
use super::readiness::{Readiness, Wait, readiness};
use super::stack::may_reach;
use super::workflow::{Category, Status, Workflow};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Claim {
        waits: Vec<Wait>,
        at: String,
        worktree: Option<String>,
    },
    Release,
    Close,
    Update {
        revision: u64,
        status: Option<Status>,
        changes_assignee: bool,
    },
    Merge {
        landing: Landing,
    },
    Advance {
        status: Status,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Landing {
    Branch {
        branch: String,
        since_claim: bool,
    },
    Trailer,
    /// A landing that an earlier `Merge` confirmed and its bases held back.
    Held,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Unchanged,
    Changed(Change),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub status: Status,
    pub claim: ClaimAfter,
    pub takes_evidence: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimAfter {
    Kept,
    Started(Box<Claim>),
    Ended,
}

pub fn decide(
    event: &Event,
    issue: &Issue,
    caller: &Caller,
    project: &Project,
) -> Result<Outcome, Failure> {
    let rules = Rules {
        issue,
        caller,
        project,
    };
    let outcome = match event {
        Event::Claim {
            waits,
            at,
            worktree,
        } => rules.claim(waits, at, worktree.as_deref()),
        Event::Release => rules.release(),
        Event::Close => rules.close(),
        Event::Update {
            revision,
            status,
            changes_assignee,
        } => rules.update(*revision, status.as_deref(), *changes_assignee),
        Event::Merge { landing } => rules.merge(landing),
        Event::Advance { status } => rules.advance(status),
    }?;
    if let Outcome::Changed(change) = &outcome {
        may_reach(issue, &project.workflow, &change.status)?;
    }
    Ok(outcome)
}

pub fn create_status(workflow: &Workflow, requested: Option<&str>) -> Result<Status, Failure> {
    match requested {
        Some(status) if workflow.category_of(status).is_some() => Ok(status.to_string()),
        Some(status) => Err(unknown_status(workflow, status)),
        None => Ok(workflow
            .first_of(Category::Unstarted)
            .unwrap_or(&workflow.moves.release)
            .clone()),
    }
}

fn unknown_status(workflow: &Workflow, status: &str) -> Failure {
    Failure::UnknownStatus {
        status: status.to_string(),
        statuses: workflow.keys(),
    }
}

struct Rules<'a> {
    issue: &'a Issue,
    caller: &'a Caller,
    project: &'a Project,
}

impl Rules<'_> {
    fn moves_to(&self, status: &Status, claim: ClaimAfter, takes_evidence: bool) -> Outcome {
        Outcome::Changed(Change {
            status: status.clone(),
            claim,
            takes_evidence,
        })
    }

    fn in_merge_status(&self) -> bool {
        self.issue.status == self.project.workflow.moves.merge
    }

    fn agent_on_human_issue(&self) -> bool {
        !self.caller.is_human() && self.issue.assigned_to_human()
    }

    fn caller_holds(&self) -> bool {
        self.issue
            .claim
            .as_ref()
            .is_some_and(|claim| self.caller.holds(claim))
    }

    fn held(&self, claim: &Claim) -> Failure {
        Failure::Held {
            issue: self.issue.id.clone(),
            claim: Box::new(claim.clone()),
        }
    }

    fn claim(&self, waits: &[Wait], at: &str, named: Option<&str>) -> Result<Outcome, Failure> {
        let working_copy = &self.caller.working_copy;
        let (Some(worktree), Some(branch)) = (&working_copy.worktree, &working_copy.branch) else {
            return Err(Failure::NoWorkingCopy);
        };
        let in_named_worktree = named.is_none_or(|named| {
            named == worktree
                && self.project.repository.is_some()
                && self.project.repository == working_copy.repository
        });
        if !in_named_worktree {
            return Err(Failure::NoWorkingCopy);
        }
        if let Some(claim) = &self.issue.claim {
            return if self.caller.holds(claim) {
                Ok(Outcome::Unchanged)
            } else {
                Err(self.held(claim))
            };
        }
        match readiness(self.issue, waits, self.caller) {
            Readiness::Ready => {
                let claim = Claim {
                    holder: self.caller.actor.clone(),
                    branch: branch.clone(),
                    worktree: worktree.clone(),
                    since: at.to_string(),
                };
                Ok(self.moves_to(
                    &self.project.workflow.moves.claim,
                    ClaimAfter::Started(Box::new(claim)),
                    false,
                ))
            }
            Readiness::NotReady { reasons, blockers } => Err(Failure::NotReady {
                issue: self.issue.id.clone(),
                status: self.issue.status.clone(),
                category: self.issue.category,
                reasons,
                blockers,
            }),
        }
    }

    fn update(
        &self,
        revision: u64,
        status: Option<&str>,
        changes_assignee: bool,
    ) -> Result<Outcome, Failure> {
        if revision != self.issue.revision {
            return Err(Failure::Stale {
                revision: self.issue.revision,
            });
        }
        let Some(status) = status else {
            return if changes_assignee && self.agent_on_human_issue() {
                Err(Failure::HumanOnly)
            } else {
                Ok(Outcome::Unchanged)
            };
        };
        let workflow = &self.project.workflow;
        let Some(category) = workflow.category_of(status) else {
            return Err(unknown_status(workflow, status));
        };
        if self.agent_on_human_issue() {
            return Err(Failure::HumanOnly);
        }
        if let Some(claim) = &self.issue.claim
            && !self.caller.holds(claim)
            && !self.caller.is_human()
        {
            return Err(self.held(claim));
        }
        let status = status.to_string();
        let claim = if status == workflow.moves.merge || category != Category::Started {
            ClaimAfter::Ended
        } else {
            ClaimAfter::Kept
        };
        Ok(self.moves_to(&status, claim, false))
    }

    fn advance(&self, status: &str) -> Result<Outcome, Failure> {
        let workflow = &self.project.workflow;
        let waits_before_status = match (
            workflow.position(&workflow.moves.merge),
            workflow.position(&self.issue.status),
            workflow.position(status),
        ) {
            (Some(merge), Some(current), Some(target)) => merge <= current && current < target,
            _ => false,
        };
        if self.issue.category.is_resolved() || !waits_before_status {
            return Ok(Outcome::Unchanged);
        }
        self.update(self.issue.revision, Some(status), false)
    }

    fn release(&self) -> Result<Outcome, Failure> {
        let Some(claim) = &self.issue.claim else {
            return Ok(Outcome::Unchanged);
        };
        if self.agent_on_human_issue() {
            return Err(Failure::HumanOnly);
        }
        if !self.caller.holds(claim) && !self.caller.is_human() {
            return Err(self.held(claim));
        }
        Ok(self.moves_to(
            &self.project.workflow.moves.release,
            ClaimAfter::Ended,
            false,
        ))
    }

    fn close(&self) -> Result<Outcome, Failure> {
        let moves = &self.project.workflow.moves;
        if self.issue.category == Category::Canceled {
            return Err(Failure::Resolved {
                issue: self.issue.id.clone(),
                status: self.issue.status.clone(),
            });
        }
        if self.issue.category == Category::Completed || self.in_merge_status() {
            return Ok(Outcome::Unchanged);
        }
        if self.agent_on_human_issue() {
            return Err(Failure::HumanOnly);
        }
        if !self.caller.is_human() && !self.caller_holds() {
            return Err(match &self.issue.claim {
                Some(claim) => self.held(claim),
                None => Failure::NotClaimed {
                    issue: self.issue.id.clone(),
                },
            });
        }
        match &self.issue.claim {
            Some(claim)
                if self.caller.holds(claim) && claim.branch != self.project.default_branch =>
            {
                Ok(self.moves_to(&moves.review, ClaimAfter::Kept, true))
            }
            _ => Ok(self.moves_to(&moves.merge, ClaimAfter::Ended, true)),
        }
    }

    fn merge(&self, landing: &Landing) -> Result<Outcome, Failure> {
        if self.issue.category.is_resolved() || self.in_merge_status() {
            return Ok(Outcome::Unchanged);
        }
        let lands = match (&self.issue.claim, landing) {
            (None, _) | (_, Landing::Held) => true,
            (
                Some(claim),
                Landing::Branch {
                    branch,
                    since_claim,
                },
            ) => *since_claim && claim.branch == *branch,
            (Some(_), Landing::Trailer) => self.issue.status == self.project.workflow.moves.review,
        };
        if !lands {
            return Ok(Outcome::Unchanged);
        }
        Ok(self.moves_to(&self.project.workflow.moves.merge, ClaimAfter::Ended, false))
    }
}
