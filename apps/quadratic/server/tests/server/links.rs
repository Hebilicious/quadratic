use serde_json::json;

use crate::support::start;

#[tokio::test(flavor = "multi_thread")]
async fn a_link_answers_with_the_issue_and_its_relations() {
    let board = start().await;
    board.project("DEM").await;
    let blocker = board.issue("Write the parser").await;
    let blocked = board.issue("Ship the parser").await;

    let linked = board
        .ok(
            &board.human(),
            "issue.link",
            json!({ "issue": blocked, "kind": "related", "target": blocker }),
        )
        .await;

    assert_eq!(linked["issue"]["id"], blocked);
    assert_eq!(linked["issue"]["title"], "Ship the parser");
    assert_eq!(linked["relations"][0]["kind"], "related");
    assert_eq!(linked["relations"][0]["issue"], blocker);
}
