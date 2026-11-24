use quadratique_server::domain::{
    Assignee, Category, NotReadyReason, Readiness, Wait, WaitKind, readiness,
};

use super::support::{
    Board, agent, agent_with_role, assigned, claim_of, claimed, human, other_agent,
};

fn wait(on: &str, kind: WaitKind, category: Option<Category>) -> Wait {
    Wait {
        on: on.into(),
        kind,
        category,
    }
}

fn not_ready(reasons: Vec<NotReadyReason>, blockers: &[&str]) -> Readiness {
    Readiness::NotReady {
        reasons,
        blockers: blockers.iter().map(|id| id.to_string()).collect(),
    }
}

#[test]
fn an_unstarted_unclaimed_issue_with_resolved_waits_is_ready_for_everyone() {
    let board = Board::default_workflow();
    let waits = [
        wait("DEM-1", WaitKind::Blocker, Some(Category::Completed)),
        wait("DEM-2", WaitKind::Child, Some(Category::Canceled)),
        wait(
            "DEM-3",
            WaitKind::AncestorBlocker,
            Some(Category::Completed),
        ),
    ];
    for caller in [agent(), agent_with_role("security"), human()] {
        assert_eq!(
            readiness(&board.issue("todo"), &waits, &caller),
            Readiness::Ready
        );
    }
}

#[test]
fn a_status_outside_the_unstarted_category_is_not_ready() {
    let board = Board::default_workflow();
    for status in ["backlog", "in-progress", "merged", "done", "canceled"] {
        assert_eq!(
            readiness(&board.issue(status), &[], &agent()),
            not_ready(vec![NotReadyReason::Category], &[]),
            "{status}"
        );
    }
}

#[test]
fn an_unresolved_blocker_blocks() {
    let board = Board::default_workflow();
    let waits = [wait("DEM-1", WaitKind::Blocker, Some(Category::Started))];
    assert_eq!(
        readiness(&board.issue("todo"), &waits, &agent()),
        not_ready(vec![NotReadyReason::Blocked], &["DEM-1"])
    );
}

#[test]
fn a_blocker_whose_state_is_unknown_blocks() {
    let board = Board::default_workflow();
    let waits = [wait("DEM-1", WaitKind::Blocker, None)];
    assert_eq!(
        readiness(&board.issue("todo"), &waits, &agent()),
        not_ready(vec![NotReadyReason::Blocked], &["DEM-1"])
    );
}

#[test]
fn an_unresolved_child_keeps_its_parent_waiting() {
    let board = Board::default_workflow();
    let waits = [wait("DEM-13", WaitKind::Child, Some(Category::Backlog))];
    assert_eq!(
        readiness(&board.issue("todo"), &waits, &agent()),
        not_ready(vec![NotReadyReason::Children], &["DEM-13"])
    );
}

#[test]
fn an_unresolved_blocker_of_an_ancestor_blocks() {
    let board = Board::default_workflow();
    let waits = [wait(
        "DEM-5",
        WaitKind::AncestorBlocker,
        Some(Category::Unstarted),
    )];
    assert_eq!(
        readiness(&board.issue("todo"), &waits, &agent()),
        not_ready(vec![NotReadyReason::Blocked], &["DEM-5"])
    );
}

#[test]
fn an_issue_assigned_to_the_human_is_ready_only_for_the_human() {
    let board = Board::default_workflow();
    let issue = assigned(board.issue("todo"), Assignee::Human);
    assert_eq!(readiness(&issue, &[], &human()), Readiness::Ready);
    for caller in [agent(), agent_with_role("security")] {
        assert_eq!(
            readiness(&issue, &[], &caller),
            not_ready(vec![NotReadyReason::Assigned], &[])
        );
    }
}

#[test]
fn an_issue_assigned_to_a_role_is_ready_only_for_agents_with_that_role() {
    let board = Board::default_workflow();
    let issue = assigned(
        board.issue("todo"),
        Assignee::Agent {
            role: "security".into(),
        },
    );
    assert_eq!(
        readiness(&issue, &[], &agent_with_role("security")),
        Readiness::Ready
    );
    for caller in [agent(), agent_with_role("web-design"), human()] {
        assert_eq!(
            readiness(&issue, &[], &caller),
            not_ready(vec![NotReadyReason::Assigned], &[])
        );
    }
}

#[test]
fn every_reason_and_each_unresolved_wait_once() {
    let board = Board::default_workflow();
    let issue = assigned(board.issue("backlog"), Assignee::Human);
    let waits = [
        wait("DEM-1", WaitKind::Blocker, Some(Category::Started)),
        wait("DEM-2", WaitKind::Child, None),
        wait("DEM-1", WaitKind::AncestorBlocker, Some(Category::Started)),
        wait("DEM-3", WaitKind::Blocker, Some(Category::Completed)),
    ];
    assert_eq!(
        readiness(&issue, &waits, &agent()),
        not_ready(
            vec![
                NotReadyReason::Category,
                NotReadyReason::Blocked,
                NotReadyReason::Children,
                NotReadyReason::Assigned,
            ],
            &["DEM-1", "DEM-2"]
        )
    );
}

#[test]
fn a_claimed_issue_is_never_ready_and_says_whether_the_caller_holds_it() {
    let board = Board::default_workflow();
    let issue = claimed(board.issue("todo"), claim_of(&agent()));
    assert_eq!(
        readiness(&issue, &[], &agent()),
        not_ready(vec![NotReadyReason::Mine], &[])
    );
    for caller in [other_agent(), human()] {
        assert_eq!(
            readiness(&issue, &[], &caller),
            not_ready(vec![NotReadyReason::Claimed], &[])
        );
    }
}
