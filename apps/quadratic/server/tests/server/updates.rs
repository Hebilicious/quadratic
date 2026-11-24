use serde_json::json;

use crate::support::start;

#[tokio::test(flavor = "multi_thread")]
async fn an_update_from_a_stale_revision_fails_and_changes_nothing() {
    let board = start().await;
    board.project("DEM").await;
    let issue = board.issue("Draft the plan").await;
    let first = board.agent("first");
    let second = board.agent("second");
    let read = board.get(&issue).await["revision"]
        .as_u64()
        .expect("a revision");

    let updated = board
        .ok(
            &first,
            "issue.update",
            json!({ "issue": issue, "revision": read, "acceptance": "Criteria from the first agent." }),
        )
        .await;
    assert_eq!(updated["issue"]["revision"], read + 1);

    let failure = board
        .failure(
            &second,
            "issue.update",
            json!({ "issue": issue, "revision": read, "acceptance": "Criteria from the second agent.", "title": "Renamed" }),
        )
        .await;
    assert_eq!(failure, json!({ "_tag": "Stale", "revision": read + 1 }));

    let stored = board.get(&issue).await;
    assert_eq!(stored["acceptance"], "Criteria from the first agent.");
    assert_eq!(stored["title"], "Draft the plan");
    assert_eq!(stored["revision"], read + 1);
}
