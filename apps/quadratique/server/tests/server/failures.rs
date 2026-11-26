use serde_json::json;

use crate::support::start;

#[tokio::test(flavor = "multi_thread")]
async fn each_issue_capability_returns_a_refusal_as_its_catalog_failure() {
    let board = start().await;
    board.project("DEM").await;
    let human = board.human();
    let holder = board.agent("holder");
    let other = board.agent("other");
    let first = board.issue("First").await;
    let second = board.issue("Second").await;
    board
        .ok(&holder, "issue.claim", json!({ "issue": first }))
        .await;

    let failure = board
        .failure(&other, "issue.release", json!({ "issue": first }))
        .await;
    assert_eq!(failure["_tag"], "Held");
    assert_eq!(failure["issue"], first);
    assert_eq!(failure["claim"]["holder"]["name"], holder.name());
    assert_eq!(failure["claim"]["branch"], "holder");
    assert_eq!(failure["claim"]["worktree"], holder.worktree());

    let failure = board
        .failure(
            &other,
            "issue.finish",
            json!({ "issue": second, "evidence": "-" }),
        )
        .await;
    assert_eq!(failure, json!({ "_tag": "NotClaimed", "issue": second }));

    let failure = board
        .failure(
            &human,
            "issue.link",
            json!({ "issue": first, "kind": "related", "target": first }),
        )
        .await;
    assert_eq!(failure, json!({ "_tag": "SelfLink" }));

    board
        .ok(
            &human,
            "issue.link",
            json!({ "issue": first, "kind": "blocks", "target": second }),
        )
        .await;
    let failure = board
        .failure(
            &human,
            "issue.link",
            json!({ "issue": second, "kind": "blocks", "target": first }),
        )
        .await;
    assert_eq!(failure["_tag"], "Cycle");
    let path = failure["path"].as_array().expect("a path");
    assert_eq!(path.first(), path.last(), "{failure}");
    assert!(
        path.contains(&json!(first)) && path.contains(&json!(second)),
        "{failure}"
    );

    let failure = board
        .failure(&human, "issue.get", json!({ "issue": "DEM-99" }))
        .await;
    assert_eq!(
        failure,
        json!({ "_tag": "NotFound", "kind": "issue", "id": "DEM-99" })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cursor_continues_only_the_listing_it_came_from() {
    let board = start().await;
    board.project("DEM").await;
    for title in ["Parse tabs", "Parse spaces", "Lex tabs"] {
        board.issue(title).await;
    }
    let listing = json!({ "text": "Parse", "limit": 1 });
    let first = board
        .ok(&board.human(), "issue.list", listing.clone())
        .await;
    let cursor = first["nextCursor"].clone();
    let mut next = listing.clone();
    next["cursor"] = cursor.clone();
    let second = board.ok(&board.human(), "issue.list", next).await;
    assert_eq!(second["items"][0]["title"], "Parse spaces");

    let other_listing = json!({ "text": "tabs", "limit": 1, "cursor": cursor });
    let refused = board
        .failure(&board.human(), "issue.list", other_listing)
        .await;
    assert_eq!(refused, json!({ "_tag": "BadCursor" }));

    let page = board
        .ok(&board.human(), "history.query", json!({ "limit": 1 }))
        .await;
    let board_cursor = page["nextCursor"].clone();
    let refused = board
        .failure(
            &board.human(),
            "history.query",
            json!({ "issue": "DEM-2", "limit": 1, "cursor": board_cursor }),
        )
        .await;
    assert_eq!(refused, json!({ "_tag": "BadCursor" }));
}
