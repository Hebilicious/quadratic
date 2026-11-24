use serde::{Deserialize, Serialize};

use super::issue::{Claim, IssueId};
use super::workflow::{Category, Move, Status};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MoveProblem {
    Missing,
    Category,
    Merge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotReadyReason {
    Category,
    Blocked,
    Children,
    Stacked,
    Assigned,
    Claimed,
    Mine,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_tag")]
pub enum Failure {
    NoWorkingCopy,
    HumanOnly,
    Held {
        issue: IssueId,
        claim: Box<Claim>,
    },
    NotReady {
        issue: IssueId,
        status: Status,
        category: Category,
        reasons: Vec<NotReadyReason>,
        blockers: Vec<IssueId>,
    },
    NotClaimed {
        issue: IssueId,
    },
    Resolved {
        issue: IssueId,
        status: Status,
    },
    Stale {
        revision: u64,
    },
    UnknownStatus {
        status: Status,
        statuses: Vec<Status>,
    },
    NotAfterMerge {
        status: Status,
        statuses: Vec<Status>,
    },
    Cycle {
        path: Vec<IssueId>,
    },
    BaseNotMerged {
        issue: IssueId,
        bases: Vec<IssueId>,
    },
    DuplicateStatus {
        status: Status,
    },
    InvalidMove {
        r#move: Move,
        status: Status,
        problem: MoveProblem,
    },
    StatusInUse {
        statuses: Vec<Status>,
        issues: Vec<IssueId>,
    },
    ClaimsHeld {
        issues: Vec<IssueId>,
    },
}
