use serde_json::json;

use crate::support::start;

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_cannot_move_an_issue_that_the_human_was_assigned() {
    let board = start().await;
    board.project("DEM").await;
    let decision = board
        .issue_with(json!({ "acceptance": "It is done.", "title": "Choose the license", "assignee": { "kind": "human" } }))
        .await;
    let revision = board.get(&decision).await["revision"].clone();

    let failure = board
        .failure(
            &board.agent("worker"),
            "issue.update",
            json!({ "issue": decision, "revision": revision, "status": "done" }),
        )
        .await;

    assert_eq!(failure, json!({ "_tag": "HumanOnly" }));
    assert_eq!(board.get(&decision).await["status"], "todo");
}
