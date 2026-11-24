use serde_json::json;

use crate::support::start;

#[tokio::test(flavor = "multi_thread")]
async fn issues_close_puts_the_evidence_on_every_issue_and_moves_each_in_one_commit() {
    let board = start().await;
    board.project("DEM").await;
    let first = board.issue("First").await;
    let second = board.issue("Second").await;
    let worker = board.agent("parser");
    for issue in [&first, &second] {
        board
            .ok(&worker, "issue.claim", json!({ "issue": issue }))
            .await;
    }

    let closed = board
        .ok(
            &worker,
            "issues.close",
            json!({ "issue": [first, second], "evidence": "cargo test: 12 passed" }),
        )
        .await;

    let items = closed["items"].as_array().expect("a list");
    assert_eq!(items.len(), 2, "{closed}");
    for (item, id) in items.iter().zip([&first, &second]) {
        assert_eq!(item["id"], id.as_str());
        assert_eq!(item["status"], "in-review");
        assert_eq!(board.get(id).await["evidence"], "cargo test: 12 passed");
    }
    let history = board.ok(&board.human(), "history.query", json!({})).await;
    let closes: Vec<_> = history["items"]
        .as_array()
        .expect("a list")
        .iter()
        .filter(|change| change["operation"] == "issue.close")
        .collect();
    assert_eq!(closes.len(), 2, "{history}");
    assert_eq!(closes[0]["commit"], closes[1]["commit"], "{history}");
}

#[tokio::test(flavor = "multi_thread")]
async fn issues_close_changes_nothing_when_one_issue_cannot_be_closed_and_names_it() {
    let board = start().await;
    board.project("DEM").await;
    let mine = board.issue("Mine").await;
    let theirs = board.issue("Theirs").await;
    let worker = board.agent("parser");
    board
        .ok(&worker, "issue.claim", json!({ "issue": mine }))
        .await;
    board
        .ok(
            &board.agent("other"),
            "issue.claim",
            json!({ "issue": theirs }),
        )
        .await;

    let refused = board
        .failure(
            &worker,
            "issues.close",
            json!({ "issue": [mine, theirs], "evidence": "cargo test: 12 passed" }),
        )
        .await;

    assert_eq!(refused["_tag"], "NotClosed", "{refused}");
    assert_eq!(refused["issue"], theirs.as_str());
    assert_eq!(refused["failure"]["_tag"], "Held");
    let untouched = board.get(&mine).await;
    assert_eq!(untouched["status"], "in-progress");
    assert!(untouched.get("evidence").is_none(), "{untouched}");
}

#[tokio::test(flavor = "multi_thread")]
async fn issues_close_names_an_issue_that_does_not_exist() {
    let board = start().await;
    board.project("DEM").await;
    let mine = board.issue("Mine").await;
    let worker = board.agent("parser");
    board
        .ok(&worker, "issue.claim", json!({ "issue": mine }))
        .await;

    let refused = board
        .failure(
            &worker,
            "issues.close",
            json!({ "issue": [mine, "DEM-99"], "evidence": "checked" }),
        )
        .await;

    assert_eq!(refused["_tag"], "NotClosed", "{refused}");
    assert_eq!(refused["issue"], "DEM-99");
    assert_eq!(refused["failure"]["_tag"], "NotFound");
    assert_eq!(board.get(&mine).await["status"], "in-progress");
}
