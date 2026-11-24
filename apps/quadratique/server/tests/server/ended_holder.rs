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

/// An issue that `holder` claimed and still works on.
async fn left_in_progress(board: &Running, holder: &Who) -> String {
    board.project("DEM").await;
    let issue = board.issue("Audit the parser").await;
    board
        .ok(holder, "issue.claim", json!({ "issue": issue }))
        .await;
    issue
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_does_not_hold_the_claim_cannot_move_or_close_it() {
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
    let closed = board
        .failure(
            &other,
            "issue.close",
            json!({ "issue": issue, "evidence": "-" }),
        )
        .await;

    for failure in [&moved, &closed] {
        assert_eq!(failure["_tag"], "Held", "{failure}");
        assert_eq!(failure["claim"]["holder"]["name"], holder.name());
    }
    assert_eq!(board.get(&issue).await["status"], "in-review");
}

#[tokio::test(flavor = "multi_thread")]
async fn any_agent_releases_a_claim_in_review_and_the_issue_keeps_its_status_and_evidence() {
    let board = start().await;
    let holder = board.agent("audit");
    let issue = left_in_review(&board, &holder).await;
    let lead = board.agent("lead");

    let released = board
        .ok(
            &lead,
            "issue.release",
            json!({ "issue": issue, "reason": "the audit ended without a merge" }),
        )
        .await;

    assert_eq!(released["issue"]["status"], "in-review", "{released}");
    assert!(released["issue"].get("claim").is_none(), "{released}");
    let shown = board.get(&issue).await;
    assert_eq!(shown["evidence"], "read every branch of the parser");
    assert!(shown.get("claim").is_none(), "{shown}");
    let note = &shown["notes"][0];
    assert_eq!(note["author"], lead.name(), "{note}");
    let text = note["text"].as_str().expect("a note's text");
    assert!(text.contains(&lead.name()), "{text}");
    assert!(text.contains(&holder.name()), "{text}");
    assert!(text.contains("the audit ended without a merge"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_does_not_hold_the_claim_cannot_release_it_before_review() {
    let board = start().await;
    let holder = board.agent("audit");
    let issue = left_in_progress(&board, &holder).await;

    let released = board
        .failure(
            &board.agent("lead"),
            "issue.release",
            json!({ "issue": issue }),
        )
        .await;

    assert_eq!(released["_tag"], "Held", "{released}");
    assert_eq!(released["claim"]["holder"]["name"], holder.name());
    assert_eq!(board.get(&issue).await["status"], "in-progress");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_releasing_a_claim_in_review_on_an_issue_assigned_to_the_human_gets_human_only() {
    let board = start().await;
    let issue = left_in_review(&board, &board.agent("audit")).await;
    board
        .ok(
            &board.human(),
            "issue.update",
            json!({
                "issue": issue,
                "revision": revision(&board, &issue).await,
                "assignee": { "kind": "human" },
            }),
        )
        .await;

    let released = board
        .failure(
            &board.agent("lead"),
            "issue.release",
            json!({ "issue": issue }),
        )
        .await;

    assert_eq!(released["_tag"], "HumanOnly", "{released}");
}

#[tokio::test(flavor = "multi_thread")]
async fn once_freed_an_issue_in_review_is_moved_before_it_is_claimed_again() {
    let board = start().await;
    let issue = left_in_review(&board, &board.agent("audit")).await;
    let lead = board.agent("lead");
    board
        .ok(&lead, "issue.release", json!({ "issue": issue }))
        .await;

    let claimed_in_review = board
        .failure(&lead, "issue.claim", json!({ "issue": issue }))
        .await;
    let closed_unclaimed = board
        .failure(
            &lead,
            "issue.close",
            json!({ "issue": issue, "evidence": "-" }),
        )
        .await;
    let moved = board
        .ok(
            &lead,
            "issue.update",
            json!({
                "issue": issue,
                "revision": revision(&board, &issue).await,
                "status": "todo",
            }),
        )
        .await;
    let claimed = board
        .ok(&lead, "issue.claim", json!({ "issue": issue }))
        .await;

    assert_eq!(claimed_in_review["_tag"], "NotReady", "{claimed_in_review}");
    assert_eq!(closed_unclaimed["_tag"], "NotClaimed", "{closed_unclaimed}");
    assert_eq!(moved["issue"]["status"], "todo", "{moved}");
    assert_eq!(claimed["issue"]["claim"]["holder"]["name"], lead.name());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_session_of_the_holder_in_another_worktree_cannot_release_the_claim() {
    let board = start().await;
    let holder = in_session(board.agent("audit"), "session-1");
    let issue = left_in_progress(&board, &holder).await;

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

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_holds_an_issue_assigned_to_the_human_gets_human_only_for_a_move() {
    let board = start().await;
    let holder = board.agent("audit");
    let issue = left_in_review(&board, &holder).await;
    board
        .ok(
            &board.human(),
            "issue.update",
            json!({
                "issue": issue,
                "revision": revision(&board, &issue).await,
                "assignee": { "kind": "human" },
            }),
        )
        .await;

    let released = board
        .failure(&holder, "issue.release", json!({ "issue": issue }))
        .await;
    let moved = board
        .failure(
            &holder,
            "issue.update",
            json!({
                "issue": issue,
                "revision": revision(&board, &issue).await,
                "status": "in-progress",
            }),
        )
        .await;

    assert_eq!(released["_tag"], "HumanOnly", "{released}");
    assert_eq!(moved["_tag"], "HumanOnly", "{moved}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_human_closing_a_stacked_issue_before_its_base_merges_gets_base_not_merged() {
    let board = start().await;
    board.project("DEM").await;
    let base = board.issue("Base").await;
    let stacked = board.issue("Stacked").await;
    board
        .ok(
            &board.human(),
            "issue.link",
            json!({ "issue": stacked, "kind": "stacked-on", "target": base }),
        )
        .await;

    let closed = board
        .failure(
            &board.human(),
            "issue.close",
            json!({ "issue": stacked, "evidence": "checked" }),
        )
        .await;

    assert_eq!(closed["_tag"], "BaseNotMerged", "{closed}");
}
