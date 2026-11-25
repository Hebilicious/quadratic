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

fn without_harness(mut who: Who) -> Who {
    let actor = who.actor.as_mut().expect("an actor");
    actor.as_object_mut().expect("an object").remove("harness");
    who
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
    assert!(text.contains("branch audit"), "{text}");
    assert!(text.contains(&holder.worktree()), "{text}");
    assert!(text.contains("the audit ended without a merge"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn any_agent_frees_a_claim_at_a_status_after_review_and_the_issue_keeps_its_assignee() {
    let board = start().await;
    board.project("DEM").await;
    let statuses: Vec<Value> = [
        ("backlog", "backlog"),
        ("todo", "unstarted"),
        ("in-progress", "started"),
        ("in-review", "started"),
        ("qa", "started"),
        ("merged", "started"),
        ("done", "completed"),
        ("canceled", "canceled"),
    ]
    .iter()
    .map(|(key, category)| json!({ "key": key, "name": key, "category": category }))
    .collect();
    board
        .ok(
            &board.human(),
            "project.update",
            json!({
                "project": "DEM",
                "workflow": {
                    "statuses": statuses,
                    "moves": { "claim": "in-progress", "review": "in-review", "merge": "merged", "release": "todo" },
                },
            }),
        )
        .await;
    let assignee = json!({ "kind": "agent", "role": "audit" });
    let issue = board
        .issue_with(json!({ "title": "Audit the parser", "acceptance": "-", "assignee": assignee }))
        .await;
    let mut holder = board.agent("audit");
    holder.actor.as_mut().expect("an actor")["role"] = json!("audit");
    board
        .ok(&holder, "issue.claim", json!({ "issue": issue }))
        .await;
    board
        .ok(
            &holder,
            "issue.close",
            json!({ "issue": issue, "evidence": "read every branch of the parser" }),
        )
        .await;
    board
        .ok(
            &holder,
            "issue.update",
            json!({ "issue": issue, "revision": revision(&board, &issue).await, "status": "qa" }),
        )
        .await;
    let lead = board.agent("lead");

    let released = board
        .ok(&lead, "issue.release", json!({ "issue": issue }))
        .await;

    assert_eq!(released["issue"]["status"], "qa", "{released}");
    let shown = board.get(&issue).await;
    assert_eq!(shown["assignee"], assignee, "{shown}");
    assert_eq!(shown["evidence"], "read every branch of the parser");
    assert!(shown.get("claim").is_none(), "{shown}");
    let text = shown["notes"][0]["text"].as_str().expect("a note's text");
    assert!(text.contains(&holder.name()), "{text}");
    assert!(text.contains("branch audit"), "{text}");
    assert!(text.contains(&holder.worktree()), "{text}");
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

/// Every move that `rule 2` names, by `who`, on an issue in progress: the answers of a status
/// change, a release and a close, in that order.
async fn every_move(board: &Running, who: &Who, issue: &str) -> [Result<Value, Value>; 3] {
    let moved = board
        .call(
            who,
            "issue.update",
            json!({ "issue": issue, "revision": revision(board, issue).await, "status": "todo" }),
        )
        .await;
    let released = board
        .call(who, "issue.release", json!({ "issue": issue }))
        .await;
    let closed = board
        .call(
            who,
            "issue.close",
            json!({ "issue": issue, "evidence": "-" }),
        )
        .await;
    [moved, released, closed]
}

fn assert_held_for_every_move(answers: &[Result<Value, Value>; 3], holder: &Who) {
    for answer in answers {
        let failure = answer.as_ref().expect_err("a refusal");
        assert_eq!(failure["_tag"], "Held", "{failure}");
        assert_eq!(failure["claim"]["holder"]["name"], holder.name());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_holders_session_closes_moves_and_releases_its_claim_from_another_worktree() {
    let board = start().await;
    let holder = in_session(board.agent("audit"), "session-1");
    let issue = left_in_progress(&board, &holder).await;
    let elsewhere = in_session(board.agent("lead"), "session-1");

    let closed = board
        .ok(
            &elsewhere,
            "issue.close",
            json!({ "issue": issue, "evidence": "checked from lead" }),
        )
        .await;
    let moved = board
        .ok(
            &elsewhere,
            "issue.update",
            json!({
                "issue": issue,
                "revision": revision(&board, &issue).await,
                "status": "in-progress",
            }),
        )
        .await;
    let released = board
        .ok(&elsewhere, "issue.release", json!({ "issue": issue }))
        .await;

    assert_eq!(closed["issue"]["status"], "in-review", "{closed}");
    let claim = &closed["issue"]["claim"];
    assert_eq!(claim["holder"]["name"], holder.name(), "{claim}");
    assert_eq!(claim["branch"], "audit", "{claim}");
    assert_eq!(claim["worktree"], holder.worktree(), "{claim}");
    assert_eq!(moved["issue"]["status"], "in-progress", "{moved}");
    assert_eq!(moved["issue"]["claim"]["branch"], "audit", "{moved}");
    assert_eq!(released["issue"]["status"], "todo", "{released}");
    assert!(released["issue"].get("claim").is_none(), "{released}");
    assert!(board.get(&issue).await["notes"] == json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn another_session_in_another_worktree_gets_held() {
    let board = start().await;
    let holder = in_session(board.agent("audit"), "session-1");
    let issue = left_in_progress(&board, &holder).await;

    let answers = every_move(
        &board,
        &in_session(board.agent("lead"), "session-2"),
        &issue,
    )
    .await;

    assert_held_for_every_move(&answers, &holder);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_holders_session_id_from_another_harness_gets_held() {
    let board = start().await;
    let holder = in_session(board.agent("audit"), "session-1");
    let issue = left_in_progress(&board, &holder).await;
    let mut other_harness = in_session(board.agent("lead"), "session-1");
    other_harness.actor.as_mut().expect("an actor")["harness"] = json!("codex");

    let answers = every_move(&board, &other_harness, &issue).await;

    assert_held_for_every_move(&answers, &holder);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_with_no_session_in_another_worktree_gets_held() {
    let board = start().await;
    let holder = in_session(board.agent("audit"), "session-1");
    let issue = left_in_progress(&board, &holder).await;

    let answers = every_move(&board, &board.agent("lead"), &issue).await;

    assert_held_for_every_move(&answers, &holder);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_id_without_a_harness_in_another_worktree_gets_held() {
    let board = start().await;
    let holder = without_harness(in_session(board.agent("audit"), "session-1"));
    let issue = left_in_progress(&board, &holder).await;
    let elsewhere = without_harness(in_session(board.agent("lead"), "session-1"));

    let answers = every_move(&board, &elsewhere, &issue).await;

    assert_held_for_every_move(&answers, &holder);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_holders_name_without_a_session_in_another_worktree_gets_held() {
    let board = start().await;
    let holder = board.agent("audit");
    let issue = left_in_progress(&board, &holder).await;
    let mut moved = board.agent("audit");
    moved.worktree = board.agent("audit-moved").worktree;
    assert_eq!(moved.name(), holder.name());

    let answers = every_move(&board, &moved, &issue).await;

    assert_held_for_every_move(&answers, &holder);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_human_with_the_holders_harness_and_session_is_the_human_not_the_holder() {
    let board = start().await;
    let holder = in_session(board.agent("audit"), "session-1");
    let issue = left_in_progress(&board, &holder).await;
    let mut human = board.human();
    let actor = human.actor.as_mut().expect("an actor");
    actor["harness"] = json!("claude-code");
    actor["session"] = json!("session-1");

    let closed = board
        .ok(
            &human,
            "issue.close",
            json!({ "issue": issue, "evidence": "checked by the human" }),
        )
        .await;

    assert_eq!(closed["issue"]["status"], "merged", "{closed}");
    assert!(closed["issue"].get("claim").is_none(), "{closed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blank_session_in_another_worktree_gets_held() {
    let board = start().await;
    let holder = in_session(board.agent("audit"), " ");
    let issue = left_in_progress(&board, &holder).await;

    let answers = every_move(&board, &in_session(board.agent("lead"), " "), &issue).await;

    assert_held_for_every_move(&answers, &holder);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_holders_session_in_another_worktree_gets_human_only_on_an_issue_assigned_to_the_human()
{
    let board = start().await;
    let holder = in_session(board.agent("audit"), "session-1");
    let issue = left_in_progress(&board, &holder).await;
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

    let answers = every_move(
        &board,
        &in_session(board.agent("lead"), "session-1"),
        &issue,
    )
    .await;

    for answer in &answers {
        let failure = answer.as_ref().expect_err("a refusal");
        assert_eq!(failure["_tag"], "HumanOnly", "{failure}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_holders_session_in_another_worktree_gets_base_not_merged_for_a_stacked_issue() {
    let board = start().await;
    let holder = in_session(board.agent("audit"), "session-1");
    let stacked = left_in_progress(&board, &holder).await;
    let base = board.issue("Base").await;
    board
        .ok(
            &board.human(),
            "issue.link",
            json!({ "issue": stacked, "kind": "stacked-on", "target": base }),
        )
        .await;

    let merged = board
        .failure(
            &in_session(board.agent("lead"), "session-1"),
            "issue.update",
            json!({
                "issue": stacked,
                "revision": revision(&board, &stacked).await,
                "status": "merged",
            }),
        )
        .await;

    assert_eq!(merged["_tag"], "BaseNotMerged", "{merged}");
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
