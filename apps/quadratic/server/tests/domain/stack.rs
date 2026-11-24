//! `B stacked-on A`, in the domain: each test is named after the rule it proves
//! (`docs/decisions/0001-stacked-on.md`).

use quadratic_server::domain::{
    Base, Caller, Event, Failure, Issue, Landing, NotReadyReason, Outcome, Readiness, Stage,
    decide, readiness,
};

use super::support::{Board, DEMO_REPOSITORY, agent, boards, claim_of, claimed, human};

fn base(stage: Stage, branch: Option<&str>) -> Base {
    Base {
        on: "DEM-1".into(),
        stage,
        branch: branch.map(Into::into),
        repository: Some(DEMO_REPOSITORY.into()),
    }
}

fn stacked(mut issue: Issue, bases: Vec<Base>) -> Issue {
    issue.bases = bases;
    issue
}

fn containing(mut caller: Caller, branches: &[&str]) -> Caller {
    caller.working_copy.contains = Some(branches.iter().map(|b| b.to_string()).collect());
    caller
}

fn held_back_by_dem_1() -> Readiness {
    Readiness::NotReady {
        reasons: vec![NotReadyReason::Stacked],
        blockers: vec!["DEM-1".into()],
    }
}

fn stage_of(board: &Board, status: &str) -> Stage {
    let category = board.project.workflow.category_of(status);
    Stage::of(&board.project.workflow, status, category)
}

#[test]
fn in_review_or_later_means_the_review_status_or_after_it_and_not_canceled() {
    for board in boards() {
        let stages = [
            (board.backlog, Stage::BeforeReview),
            (board.todo, Stage::BeforeReview),
            (board.in_progress, Stage::BeforeReview),
            (board.in_review, Stage::InReview),
            (board.merged, Stage::Merged),
            (board.done, Stage::Merged),
            (board.canceled, Stage::Canceled),
        ];
        for (status, stage) in stages {
            assert_eq!(stage_of(&board, status), stage, "{} {status}", board.label);
        }
        assert_eq!(
            Stage::of(&board.project.workflow, "gone", None),
            Stage::BeforeReview,
            "a status the workflow lacks"
        );
    }
}

#[test]
fn b_is_ready_once_a_is_in_review_and_not_before() {
    let board = Board::default_workflow();
    let caller = containing(agent(), &["fix-login", "parse"]);
    let before = stacked(
        board.issue("todo"),
        vec![base(Stage::BeforeReview, Some("parse"))],
    );
    let in_review = stacked(
        board.issue("todo"),
        vec![base(Stage::InReview, Some("parse"))],
    );

    assert_eq!(readiness(&before, &[], &caller), held_back_by_dem_1());
    assert_eq!(readiness(&in_review, &[], &caller), Readiness::Ready);
}

#[test]
fn a_canceled_base_keeps_b_not_ready() {
    let board = Board::default_workflow();
    let issue = stacked(
        board.issue("todo"),
        vec![base(Stage::Canceled, Some("parse"))],
    );
    let caller = containing(agent(), &["parse"]);
    assert_eq!(readiness(&issue, &[], &caller), held_back_by_dem_1());
}

#[test]
fn b_is_ready_only_for_a_caller_whose_branch_contains_a_branch() {
    let board = Board::default_workflow();
    let issue = stacked(
        board.issue("todo"),
        vec![base(Stage::InReview, Some("parse"))],
    );

    let on_top = containing(agent(), &["fix-login", "parse", "main"]);
    let unrelated = containing(agent(), &["fix-login", "main"]);
    assert_eq!(readiness(&issue, &[], &on_top), Readiness::Ready);
    assert_eq!(readiness(&issue, &[], &unrelated), held_back_by_dem_1());
}

#[test]
fn b_is_offered_to_nobody_when_the_caller_branches_or_a_branch_are_unknown() {
    let board = Board::default_workflow();
    let with_branch = stacked(
        board.issue("todo"),
        vec![base(Stage::InReview, Some("parse"))],
    );
    let without_branch = stacked(board.issue("todo"), vec![base(Stage::InReview, None)]);
    let everything = containing(agent(), &["parse", "fix-login", "main"]);

    assert_eq!(readiness(&with_branch, &[], &agent()), held_back_by_dem_1());
    assert_eq!(readiness(&with_branch, &[], &human()), held_back_by_dem_1());
    assert_eq!(
        readiness(&without_branch, &[], &everything),
        held_back_by_dem_1()
    );
}

#[test]
fn b_is_ready_for_any_caller_once_a_is_merged() {
    let board = Board::default_workflow();
    let issue = stacked(board.issue("todo"), vec![base(Stage::Merged, None)]);
    for caller in [agent(), human()] {
        assert_eq!(readiness(&issue, &[], &caller), Readiness::Ready);
    }
}

#[test]
fn a_claimed_b_shows_a_reason_naming_a_only_while_a_is_before_review() {
    let board = Board::default_workflow();
    let holder = agent();
    let claim = claim_of(&holder);
    let back = claimed(
        stacked(
            board.issue("in-progress"),
            vec![base(Stage::BeforeReview, Some("parse"))],
        ),
        claim.clone(),
    );
    let in_review = claimed(
        stacked(
            board.issue("in-progress"),
            vec![base(Stage::InReview, Some("parse"))],
        ),
        claim,
    );

    assert_eq!(
        readiness(&back, &[], &holder),
        Readiness::NotReady {
            reasons: vec![
                NotReadyReason::Category,
                NotReadyReason::Stacked,
                NotReadyReason::Mine
            ],
            blockers: vec!["DEM-1".into()],
        }
    );
    assert_eq!(
        readiness(&in_review, &[], &holder),
        Readiness::NotReady {
            reasons: vec![NotReadyReason::Category, NotReadyReason::Mine],
            blockers: vec![],
        }
    );
}

#[test]
fn a_resolved_b_has_no_stacked_reason() {
    let board = Board::default_workflow();
    let issue = stacked(board.issue("done"), vec![base(Stage::BeforeReview, None)]);
    let Readiness::NotReady { reasons, .. } = readiness(&issue, &[], &agent()) else {
        panic!("a done issue is not ready");
    };
    assert_eq!(reasons, vec![NotReadyReason::Category]);
}

#[test]
fn an_issue_stacked_on_two_issues_waits_for_both() {
    let board = Board::default_workflow();
    let bases = vec![
        base(Stage::InReview, Some("parse")),
        Base {
            on: "DEM-2".into(),
            stage: Stage::BeforeReview,
            branch: None,
            repository: Some(DEMO_REPOSITORY.into()),
        },
    ];
    let issue = stacked(board.issue("todo"), bases);
    let caller = containing(agent(), &["parse"]);
    assert_eq!(
        readiness(&issue, &[], &caller),
        Readiness::NotReady {
            reasons: vec![NotReadyReason::Stacked],
            blockers: vec!["DEM-2".into()],
        }
    );
}

fn base_not_merged(bases: &[&str]) -> Failure {
    Failure::BaseNotMerged {
        issue: "DEM-12".into(),
        bases: bases.iter().map(|id| id.to_string()).collect(),
    }
}

#[test]
fn b_cannot_move_to_the_merge_status_or_later_before_a_has() {
    for board in boards() {
        for stage in [Stage::BeforeReview, Stage::InReview, Stage::Canceled] {
            let issue = stacked(board.issue(board.in_review), vec![base(stage, None)]);
            for status in [board.merged, board.done] {
                let event = Event::Update {
                    revision: issue.revision,
                    status: Some(status.into()),
                    changes_assignee: false,
                };
                assert_eq!(
                    decide(&event, &issue, &human(), &board.project),
                    Err(base_not_merged(&["DEM-1"])),
                    "{} {stage:?} {status}",
                    board.label
                );
            }
            assert_eq!(
                decide(&Event::Close, &issue, &human(), &board.project),
                Err(base_not_merged(&["DEM-1"])),
                "{} {stage:?} close",
                board.label
            );
        }
    }
}

#[test]
fn b_may_move_to_merged_once_a_has_and_may_always_be_canceled() {
    for board in boards() {
        let merged = stacked(
            board.issue(board.in_review),
            vec![base(Stage::Merged, None)],
        );
        let unmerged = stacked(
            board.issue(board.in_review),
            vec![base(Stage::BeforeReview, None)],
        );
        let to = |status: &str| Event::Update {
            revision: 3,
            status: Some(status.into()),
            changes_assignee: false,
        };
        assert!(matches!(
            decide(&to(board.merged), &merged, &human(), &board.project),
            Ok(Outcome::Changed(_))
        ));
        assert!(matches!(
            decide(&to(board.canceled), &unmerged, &human(), &board.project),
            Ok(Outcome::Changed(_))
        ));
        assert!(matches!(
            decide(&to(board.in_progress), &unmerged, &human(), &board.project),
            Ok(Outcome::Changed(_))
        ));
    }
}

#[test]
fn a_landing_of_b_before_a_merges_is_held_and_lands_when_released() {
    let board = Board::default_workflow();
    let holder = agent();
    let issue = claimed(
        stacked(board.issue("in-review"), vec![base(Stage::InReview, None)]),
        claim_of(&holder),
    );
    let landing = Event::Merge {
        landing: Landing::Trailer,
    };
    assert_eq!(
        decide(&landing, &issue, &human(), &board.project),
        Err(base_not_merged(&["DEM-1"]))
    );

    let released = stacked(issue.clone(), vec![base(Stage::Merged, None)]);
    let held = Event::Merge {
        landing: Landing::Held,
    };
    let Ok(Outcome::Changed(change)) = decide(&held, &released, &human(), &board.project) else {
        panic!("a released landing moves the issue");
    };
    assert_eq!(change.status, "merged");
    assert_eq!(
        decide(&held, &issue, &human(), &board.project),
        Err(base_not_merged(&["DEM-1"]))
    );
}

#[test]
fn every_stacked_reason_names_its_base_in_blockers() {
    let board = Board::default_workflow();
    let caller = containing(agent(), &[]);
    for stage in [Stage::BeforeReview, Stage::InReview, Stage::Canceled] {
        let issue = stacked(board.issue("todo"), vec![base(stage, Some("parse"))]);
        assert_eq!(
            readiness(&issue, &[], &caller),
            held_back_by_dem_1(),
            "{stage:?}"
        );
    }
}

fn claim_after_merge() -> Board {
    let mut board = Board::default_workflow();
    let statuses = &mut board.project.workflow.statuses;
    let merged = statuses.remove(4);
    statuses.insert(2, merged);
    board
}

#[test]
fn every_move_that_ends_at_or_after_the_merge_status_waits_for_the_bases() {
    let board = claim_after_merge();
    assert_eq!(board.project.workflow.validate(), Ok(()));
    let holder = containing(agent(), &["parse"]);
    let unclaimed = stacked(
        board.issue("todo"),
        vec![base(Stage::InReview, Some("parse"))],
    );
    let claim = Event::Claim {
        waits: vec![],
        at: "2026-11-24T10:00:00Z".into(),
        worktree: None,
    };
    assert_eq!(
        decide(&claim, &unclaimed, &holder, &board.project),
        Err(base_not_merged(&["DEM-1"])),
        "a claim into a status after the merge status"
    );
    let held = claimed(
        stacked(
            board.issue("in-progress"),
            vec![base(Stage::InReview, Some("parse"))],
        ),
        claim_of(&holder),
    );
    assert_eq!(
        decide(&Event::Close, &held, &holder, &board.project),
        Err(base_not_merged(&["DEM-1"])),
        "a close from a feature branch into a review status after the merge status"
    );
}
