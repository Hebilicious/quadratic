use quadratique_server::domain::{Category, Failure, check_project_update};

use super::support::{Board, agent, claim_of, claimed};

#[test]
fn an_invalid_workflow_is_refused() {
    let board = Board::default_workflow();
    let mut proposed = board.project.clone();
    proposed.workflow.moves.claim = "todo".into();
    assert!(matches!(
        check_project_update(&board.project, &proposed, &[]),
        Err(Failure::InvalidMove { .. })
    ));
}

#[test]
fn a_workflow_that_drops_or_recategorises_a_status_in_use_is_refused() {
    let board = Board::default_workflow();
    let mut proposed = board.project.clone();
    proposed
        .workflow
        .statuses
        .retain(|status| status.key != "backlog");
    proposed
        .workflow
        .statuses
        .iter_mut()
        .filter(|status| status.key == "merged")
        .for_each(|status| status.category = Category::Completed);
    let mut first = board.issue("backlog");
    first.id = "DEM-1".into();
    let mut second = board.issue("merged");
    second.id = "DEM-2".into();
    let mut third = board.issue("todo");
    third.id = "DEM-3".into();
    let mut fourth = board.issue("backlog");
    fourth.id = "DEM-4".into();
    assert_eq!(
        check_project_update(&board.project, &proposed, &[first, second, third, fourth]),
        Err(Failure::StatusInUse {
            statuses: vec!["backlog".into(), "merged".into()],
            issues: vec!["DEM-1".into(), "DEM-2".into(), "DEM-4".into()],
        })
    );
}

#[test]
fn renaming_reordering_and_changing_moves_are_allowed() {
    let board = Board::default_workflow();
    let mut proposed = board.project.clone();
    proposed.workflow.statuses.reverse();
    proposed.workflow.statuses[0].name = "Abandoned".into();
    proposed.workflow.moves.review = "in-progress".into();
    let issues = [
        claimed(board.issue("in-progress"), claim_of(&agent())),
        board.issue("todo"),
    ];
    assert_eq!(
        check_project_update(&board.project, &proposed, &issues),
        Ok(())
    );
}

#[test]
fn a_new_repository_or_default_branch_is_refused_while_claims_are_held() {
    let board = Board::default_workflow();
    let mut held = claimed(board.issue("in-progress"), claim_of(&agent()));
    held.id = "DEM-7".into();
    let issues = [held, board.issue("todo")];

    let mut new_branch = board.project.clone();
    new_branch.default_branch = "trunk".into();
    let mut new_repository = board.project.clone();
    new_repository.repository = Some("/home/sam/elsewhere/demo/.git".into());
    for proposed in [new_branch, new_repository] {
        assert_eq!(
            check_project_update(&board.project, &proposed, &issues),
            Err(Failure::ClaimsHeld {
                issues: vec!["DEM-7".into()]
            })
        );
        assert_eq!(
            check_project_update(&board.project, &proposed, &[board.issue("todo")]),
            Ok(())
        );
    }
    assert_eq!(
        check_project_update(&board.project, &board.project.clone(), &issues),
        Ok(())
    );
}

#[test]
fn a_merge_move_onto_a_status_where_claims_are_held_is_refused() {
    let board = Board::default_workflow();
    let mut held = claimed(board.issue("in-progress"), claim_of(&agent()));
    held.id = "DEM-3".into();
    let mut proposed = board.project.clone();
    proposed.workflow.moves.merge = "in-progress".into();
    proposed.workflow.moves.claim = "in-review".into();
    proposed.workflow.moves.review = "merged".into();

    assert_eq!(
        check_project_update(&board.project, &proposed, &[held, board.issue("todo")]),
        Err(Failure::ClaimsHeld {
            issues: vec!["DEM-3".into()]
        })
    );
    assert_eq!(
        check_project_update(&board.project, &proposed, &[board.issue("in-progress")]),
        Ok(())
    );
}
