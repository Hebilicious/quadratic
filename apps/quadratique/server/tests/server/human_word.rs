//! Rule 4 of `docs/decisions/0002-claims-of-agents-that-ended.md`, through the server: an agent
//! moves an issue assigned to the human only with the human's words and high confidence.

use serde_json::{Value, json};

use crate::support::{Running, start};

const WORDS: &str = "Yes, MIT. Close it.";

async fn decision(board: &Running) -> String {
    board.project("DEM").await;
    board
        .issue_with(json!({
            "title": "Choose the license",
            "acceptance": "The human chose a license.",
            "assignee": { "kind": "human" },
        }))
        .await
}

async fn revision(board: &Running, issue: &str) -> Value {
    board.get(issue).await["revision"].clone()
}

async fn last_change(board: &Running, issue: &str) -> Value {
    board
        .ok(
            &board.human(),
            "history.query",
            json!({ "issue": issue, "limit": 1 }),
        )
        .await["items"][0]
        .clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn rule_4_with_high_confidence_an_agent_moves_the_issue_and_the_note_and_history_quote_the_words()
 {
    let board = start().await;
    let issue = decision(&board).await;
    let lead = board.agent("lead");

    let moved = board
        .ok(
            &lead,
            "issue.update",
            json!({
                "issue": issue,
                "revision": revision(&board, &issue).await,
                "status": "done",
                "humanWords": WORDS,
                "confidence": "high",
            }),
        )
        .await;

    assert_eq!(moved["issue"]["status"], "done", "{moved}");
    assert!(moved.get("leftForHuman").is_none(), "{moved}");
    let shown = board.get(&issue).await;
    let note = &shown["notes"][0];
    assert_eq!(note["author"], lead.name(), "{note}");
    let text = note["text"].as_str().expect("a note's text");
    assert!(text.contains(&lead.name()), "{text}");
    assert!(text.contains(&format!("\"{WORDS}\"")), "{text}");
    assert!(text.contains("high confidence"), "{text}");
    let change = last_change(&board, &issue).await;
    assert_eq!(change["operation"], "issue.update", "{change}");
    assert_eq!(change["author"]["name"], lead.name(), "{change}");
    assert_eq!(
        change["reason"],
        format!("The human's words, high confidence: \"{WORDS}\""),
        "{change}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rule_4_with_medium_or_low_confidence_nothing_moves_and_a_note_leaves_the_issue_for_the_human()
 {
    let board = start().await;
    let issue = decision(&board).await;
    let lead = board.agent("lead");

    for (index, confidence) in ["medium", "low"].into_iter().enumerate() {
        let left = board
            .ok(
                &lead,
                "issue.update",
                json!({
                    "issue": issue,
                    "revision": revision(&board, &issue).await,
                    "status": "done",
                    "title": "Renamed by the agent",
                    "humanWords": WORDS,
                    "confidence": confidence,
                }),
            )
            .await;

        assert_eq!(left["leftForHuman"], true, "{left}");
        assert_eq!(left["issue"]["status"], "todo", "{left}");
        assert_eq!(left["issue"]["title"], "Choose the license", "{left}");
        let shown = board.get(&issue).await;
        let note = &shown["notes"][index];
        assert_eq!(note["author"], lead.name(), "{note}");
        let text = note["text"].as_str().expect("a note's text");
        assert!(text.contains("left the issue for the human"), "{text}");
        assert!(text.contains(confidence), "{text}");
        assert!(text.contains(&format!("\"{WORDS}\"")), "{text}");
    }

    let released = board
        .ok(
            &lead,
            "issue.release",
            json!({ "issue": issue, "humanWords": WORDS, "confidence": "low" }),
        )
        .await;
    assert!(
        released.get("leftForHuman").is_none(),
        "unclaimed: {released}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rule_4_without_the_words_or_without_the_confidence_an_agent_gets_human_only() {
    let board = start().await;
    let issue = decision(&board).await;
    let lead = board.agent("lead");

    for partial in [
        json!({ "humanWords": WORDS }),
        json!({ "confidence": "high" }),
        json!({}),
    ] {
        let mut input = json!({
            "issue": issue,
            "revision": revision(&board, &issue).await,
            "status": "done",
        });
        input
            .as_object_mut()
            .expect("an object")
            .extend(partial.as_object().expect("an object").clone());
        let failure = board.failure(&lead, "issue.update", input).await;
        assert_eq!(failure, json!({ "_tag": "HumanOnly" }));
    }
    assert_eq!(board.get(&issue).await["status"], "todo");
    assert!(
        board
            .get(&issue)
            .await
            .get("notes")
            .is_none_or(|notes| { notes.as_array().is_some_and(Vec::is_empty) })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rule_4_with_high_confidence_base_not_merged_and_stale_keep_their_place() {
    let board = start().await;
    let issue = decision(&board).await;
    let base = board.issue("Base").await;
    board
        .ok(
            &board.human(),
            "issue.link",
            json!({ "issue": issue, "kind": "stacked-on", "target": base }),
        )
        .await;
    let lead = board.agent("lead");
    let current = revision(&board, &issue).await;

    let merged = board
        .failure(
            &lead,
            "issue.update",
            json!({
                "issue": issue,
                "revision": current,
                "status": "merged",
                "humanWords": WORDS,
                "confidence": "high",
            }),
        )
        .await;
    let stale = board
        .failure(
            &lead,
            "issue.update",
            json!({
                "issue": issue,
                "revision": current.as_u64().expect("a revision") + 1,
                "status": "done",
                "humanWords": WORDS,
                "confidence": "medium",
            }),
        )
        .await;

    assert_eq!(merged["_tag"], "BaseNotMerged", "{merged}");
    assert_eq!(stale["_tag"], "Stale", "{stale}");
    assert_eq!(board.get(&issue).await["status"], "todo");
}

#[tokio::test(flavor = "multi_thread")]
async fn rule_4_the_humans_own_moves_ignore_the_words() {
    let board = start().await;
    let issue = decision(&board).await;

    let moved = board
        .ok(
            &board.human(),
            "issue.update",
            json!({
                "issue": issue,
                "revision": revision(&board, &issue).await,
                "status": "done",
                "reason": "decided",
                "humanWords": WORDS,
                "confidence": "low",
            }),
        )
        .await;

    assert_eq!(moved["issue"]["status"], "done", "{moved}");
    assert!(moved.get("leftForHuman").is_none(), "{moved}");
    assert!(
        board
            .get(&issue)
            .await
            .get("notes")
            .is_none_or(|notes| { notes.as_array().is_some_and(Vec::is_empty) })
    );
    assert_eq!(last_change(&board, &issue).await["reason"], "decided");
}

#[tokio::test(flavor = "multi_thread")]
async fn rule_4_an_agent_that_holds_an_issue_assigned_to_the_human_closes_it_on_the_humans_words() {
    let board = start().await;
    board.project("DEM").await;
    let holder = board.agent("audit");
    let issue = board.issue("Audit the parser").await;
    board
        .ok(&holder, "issue.claim", json!({ "issue": issue }))
        .await;
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

    let left = board
        .ok(
            &holder,
            "issues.close",
            json!({
                "issue": [issue],
                "evidence": "read every branch",
                "humanWords": WORDS,
                "confidence": "medium",
            }),
        )
        .await;
    assert_eq!(left["leftForHuman"], json!([issue]), "{left}");
    assert_eq!(left["items"][0]["status"], "in-progress", "{left}");

    let closed = board
        .ok(
            &holder,
            "issue.close",
            json!({
                "issue": issue,
                "evidence": "read every branch",
                "humanWords": WORDS,
                "confidence": "high",
            }),
        )
        .await;
    assert_eq!(closed["issue"]["status"], "in-review", "{closed}");
    let notes = closed["issue"]["notes"].as_array().expect("notes");
    assert_eq!(notes.len(), 2, "{closed}");
    assert!(
        notes[1]["text"]
            .as_str()
            .is_some_and(|text| text.contains("high confidence") && text.contains("in-review")),
        "{closed}"
    );
}
