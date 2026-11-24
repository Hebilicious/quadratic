//! What an actor that does not hold a claim may do to the claimed issue: the rules of
//! `docs/decisions/0002-claims-of-agents-that-ended.md`.

use serde_json::{Value, json};

use crate::support::{Running, Who, start};

/// An issue that `holder` claimed and closed into review, as an agent does before it ends.
async fn left_in_review(board: &Running, holder: &Who) -> String {
    board.project("DEM").await;
    let issue = board.issue("Audit the parser").await;
    board
        .ok(holder, "issue.claim", json!({ "issue": issue }))
        .await;
    board
        .ok(
            holder,
            "issue.close",
            json!({ "issue": issue, "evidence": "read every branch of the parser" }),
        )
        .await;
    issue
}

async fn revision(board: &Running, issue: &str) -> Value {
    board.get(issue).await["revision"].clone()
}

fn in_session(mut who: Who, session: &str) -> Who {
    who.actor.as_mut().expect("an actor")["session"] = json!(session);
    who
}

#[tokio::test(flavor = "multi_thread")]
async fn any_agent_adds_a_note_to_an_issue_that_another_agent_holds_in_review() {
    let board = start().await;
    let holder = board.agent("audit");
    let issue = left_in_review(&board, &holder).await;
    let reviewer = board.agent("lead");

    let noted = board
        .ok(
            &reviewer,
            "issue.update",
            json!({
                "issue": issue,
                "revision": revision(&board, &issue).await,
                "notes": "Sent back: the audit skips the error paths.",
            }),
        )
        .await;

    assert_eq!(noted["issue"]["status"], "in-review");
    let shown = board.get(&issue).await;
    assert_eq!(shown["notes"][0]["author"], reviewer.name());
    assert_eq!(shown["claim"]["holder"]["name"], holder.name());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_does_not_hold_the_claim_cannot_move_release_or_close_it() {
    let board = start().await;
    let holder = board.agent("audit");
    let issue = left_in_review(&board, &holder).await;
    let other = board.agent("lead");

    let moved = board
        .failure(
            &other,
            "issue.update",
            json!({
                "issue": issue,
                "revision": revision(&board, &issue).await,
                "status": "in-progress",
            }),
        )
        .await;
    let released = board
        .failure(&other, "issue.release", json!({ "issue": issue }))
        .await;
    let closed = board
        .failure(
            &other,
            "issue.close",
            json!({ "issue": issue, "evidence": "-" }),
        )
        .await;

    for failure in [&moved, &released, &closed] {
        assert_eq!(failure["_tag"], "Held", "{failure}");
        assert_eq!(failure["claim"]["holder"]["name"], holder.name());
    }
    assert_eq!(board.get(&issue).await["status"], "in-review");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_session_of_the_holder_in_another_worktree_cannot_release_the_claim() {
    let board = start().await;
    let holder = in_session(board.agent("audit"), "session-1");
    let issue = left_in_review(&board, &holder).await;

    let released = board
        .failure(
            &in_session(board.agent("lead"), "session-1"),
            "issue.release",
            json!({ "issue": issue }),
        )
        .await;

    assert_eq!(released["_tag"], "Held", "{released}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_human_releases_a_claim_whose_holder_ended() {
    let board = start().await;
    let issue = left_in_review(&board, &board.agent("audit")).await;

    let released = board
        .ok(&board.human(), "issue.release", json!({ "issue": issue }))
        .await;

    assert_eq!(released["issue"]["status"], "todo");
    assert!(released["issue"].get("claim").is_none(), "{released}");
}
