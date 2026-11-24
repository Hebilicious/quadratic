use quadratique_server::domain::{Category, Failure, Move, MoveProblem, WorkflowStatus};

use super::support::{code_workflow, done_at_merge_workflow};

fn invalid(r#move: Move, status: &str, problem: MoveProblem) -> Result<(), Failure> {
    Err(Failure::InvalidMove {
        r#move,
        status: status.into(),
        problem,
    })
}

#[test]
fn the_workflow_of_a_code_project_is_valid() {
    assert_eq!(code_workflow().validate(), Ok(()));
}

#[test]
fn a_workflow_whose_merge_move_names_a_completed_status_is_valid() {
    assert_eq!(done_at_merge_workflow().validate(), Ok(()));
}

#[test]
fn a_key_twice_is_a_duplicate_status() {
    let mut workflow = code_workflow();
    workflow.statuses.push(WorkflowStatus {
        key: "todo".into(),
        name: "Todo again".into(),
        category: Category::Unstarted,
    });
    assert_eq!(
        workflow.validate(),
        Err(Failure::DuplicateStatus {
            status: "todo".into()
        })
    );
}

#[test]
fn a_move_that_names_a_missing_status_is_invalid() {
    let mut workflow = code_workflow();
    workflow.moves.review = "reviewing".into();
    assert_eq!(
        workflow.validate(),
        invalid(Move::Review, "reviewing", MoveProblem::Missing)
    );
}

#[test]
fn a_move_that_names_a_status_of_the_wrong_category_is_invalid() {
    let cases = [
        (Move::Claim, "todo"),
        (Move::Review, "done"),
        (Move::Merge, "canceled"),
        (Move::Merge, "todo"),
        (Move::Release, "backlog"),
        (Move::Release, "in-progress"),
    ];
    for (r#move, status) in cases {
        let mut workflow = code_workflow();
        match r#move {
            Move::Claim => workflow.moves.claim = status.into(),
            Move::Review => workflow.moves.review = status.into(),
            Move::Merge => workflow.moves.merge = status.into(),
            Move::Release => workflow.moves.release = status.into(),
        }
        assert_eq!(
            workflow.validate(),
            invalid(r#move, status, MoveProblem::Category),
            "{move:?} -> {status}"
        );
    }
}

#[test]
fn a_claim_or_review_move_that_names_the_merge_status_is_invalid() {
    let mut claim = code_workflow();
    claim.moves.claim = "merged".into();
    assert_eq!(
        claim.validate(),
        invalid(Move::Claim, "merged", MoveProblem::Merge)
    );

    let mut review = code_workflow();
    review.moves.review = "merged".into();
    assert_eq!(
        review.validate(),
        invalid(Move::Review, "merged", MoveProblem::Merge)
    );
}
