//! `B stacked-on A`: B's branch starts from A's branch. A is a base of B.
//!
//! Each rule of the link lives in one function here; `docs/decisions/0001-stacked-on.md`
//! states them.

use super::failure::Failure;
use super::issue::{Caller, Issue, IssueId};
use super::workflow::{Category, Workflow};

pub const STACKED_ON: &str = "stacked-on";

/// Where a base stands, for the issues stacked on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    BeforeReview,
    InReview,
    Merged,
    Canceled,
}

impl Stage {
    /// In review means the workflow's `review` status or a later one, and not canceled.
    /// Merged means the workflow's `merge` status or a later one, and not canceled.
    /// A status the workflow lacks counts as before review.
    pub fn of(workflow: &Workflow, status: &str, category: Option<Category>) -> Stage {
        if category == Some(Category::Canceled) {
            return Stage::Canceled;
        }
        let Some(position) = workflow.position(status) else {
            return Stage::BeforeReview;
        };
        let reached = |of: &str| workflow.position(of).is_some_and(|of| of <= position);
        if reached(&workflow.moves.merge) {
            Stage::Merged
        } else if reached(&workflow.moves.review) {
            Stage::InReview
        } else {
            Stage::BeforeReview
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base {
    pub on: IssueId,
    pub stage: Stage,
    /// The branch of the base's claim, while it has one; for a base in review whose claim was
    /// freed (decision 0002, rule 3), the branch of its last claim.
    pub branch: Option<String>,
    /// The repository that the base's project is bound to.
    pub repository: Option<String>,
}

impl Base {
    /// The caller works from the base's branch: the caller is in the repository that the
    /// base's project is bound to, and git verified there that the caller's branch is the base's
    /// branch or contains its tip. A base with no claim or no repository, or a caller whose
    /// branch was not verified, never counts as such.
    pub fn starts(&self, caller: &Caller) -> bool {
        let copy = &caller.working_copy;
        let (Some(branch), Some(repository), Some(contained)) =
            (&self.branch, &self.repository, &copy.contains)
        else {
            return false;
        };
        copy.repository.as_ref() == Some(repository) && contained.contains(branch)
    }

    /// Whether `caller` can start the issue only once git confirms its branch: the base is in
    /// review, has a branch, and its project's repository is the caller's.
    pub fn asks_git(&self, caller: &Caller) -> bool {
        self.stage == Stage::InReview
            && self.branch.is_some()
            && self.repository.is_some()
            && self.repository == caller.working_copy.repository
    }

    /// Whether this base keeps `issue` from being ready, or, for an issue already held, whether
    /// its holder must see that the base went back before review.
    fn holds_back(&self, issue: &Issue, caller: &Caller) -> bool {
        match self.stage {
            Stage::Merged => false,
            Stage::BeforeReview | Stage::Canceled => true,
            Stage::InReview => issue.claim.is_none() && !self.starts(caller),
        }
    }
}

/// The bases that make `issue` not ready for `caller`, by the `stacked` reason.
/// A resolved issue has none: the link no longer matters to it.
pub fn holding_back(issue: &Issue, caller: &Caller) -> Vec<IssueId> {
    if issue.category.is_resolved() {
        return Vec::new();
    }
    issue
        .bases
        .iter()
        .filter(|base| base.holds_back(issue, caller))
        .map(|base| base.on.clone())
        .collect()
}

/// A `stacked-on` link may not join an issue at the `merge` status or later to a base that is
/// not merged.
pub fn may_stack(
    issue: &IssueId,
    stage: Stage,
    base: &IssueId,
    base_stage: Stage,
) -> Result<(), Failure> {
    if stage == Stage::Merged && base_stage != Stage::Merged {
        Err(Failure::BaseNotMerged {
            issue: issue.clone(),
            bases: vec![base.clone()],
        })
    } else {
        Ok(())
    }
}

/// An issue moves to the workflow's `merge` status, or a later status that is not canceled,
/// only once every base is merged.
pub fn may_reach(issue: &Issue, workflow: &Workflow, status: &str) -> Result<(), Failure> {
    let reaches_merge = workflow.category_of(status) != Some(Category::Canceled)
        && Stage::of(workflow, status, None) == Stage::Merged;
    let bases: Vec<IssueId> = issue
        .bases
        .iter()
        .filter(|base| base.stage != Stage::Merged)
        .map(|base| base.on.clone())
        .collect();
    if reaches_merge && !bases.is_empty() {
        Err(Failure::BaseNotMerged {
            issue: issue.id.clone(),
            bases,
        })
    } else {
        Ok(())
    }
}
