use super::failure::NotReadyReason;
use super::issue::{Caller, Issue, IssueId};
use super::workflow::Category;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitKind {
    Blocker,
    Child,
    AncestorBlocker,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wait {
    pub on: IssueId,
    pub kind: WaitKind,
    pub category: Option<Category>,
}

impl Wait {
    fn is_unresolved(&self) -> bool {
        !self.category.is_some_and(Category::is_resolved)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    NotReady {
        reasons: Vec<NotReadyReason>,
        blockers: Vec<IssueId>,
    },
}

impl Readiness {
    pub fn is_ready(&self) -> bool {
        *self == Readiness::Ready
    }
}

pub fn readiness(issue: &Issue, waits: &[Wait], caller: &Caller) -> Readiness {
    let unresolved: Vec<&Wait> = waits.iter().filter(|wait| wait.is_unresolved()).collect();
    let mut reasons = Vec::new();
    if issue.category != Category::Unstarted {
        reasons.push(NotReadyReason::Category);
    }
    if unresolved
        .iter()
        .any(|wait| matches!(wait.kind, WaitKind::Blocker | WaitKind::AncestorBlocker))
    {
        reasons.push(NotReadyReason::Blocked);
    }
    if unresolved.iter().any(|wait| wait.kind == WaitKind::Child) {
        reasons.push(NotReadyReason::Children);
    }
    if issue
        .assignee
        .as_ref()
        .is_some_and(|assignee| !assignee.is(&caller.actor))
    {
        reasons.push(NotReadyReason::Assigned);
    }
    match &issue.claim {
        Some(claim) if caller.holds(claim) => reasons.push(NotReadyReason::Mine),
        Some(_) => reasons.push(NotReadyReason::Claimed),
        None => {}
    }
    if reasons.is_empty() {
        return Readiness::Ready;
    }
    let mut blockers: Vec<IssueId> = Vec::new();
    for wait in unresolved {
        if !blockers.contains(&wait.on) {
            blockers.push(wait.on.clone());
        }
    }
    Readiness::NotReady { reasons, blockers }
}
