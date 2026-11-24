use serde_json::json;

use crate::support::start;

#[tokio::test(flavor = "multi_thread")]
async fn ready_work_comes_most_urgent_first_then_in_board_order_with_no_priority_last() {
    let board = start().await;
    board.project("DEM").await;
    let mut created = Vec::new();
    for (title, priority) in [("A", 0), ("B", 2), ("C", 1), ("D", 2), ("E", 0)] {
        created.push(
            board
                .issue_with(
                    json!({ "acceptance": "It is done.", "title": title, "priority": priority }),
                )
                .await,
        );
    }
    let [a, b, c, d, e] = created.try_into().expect("five issues");
    let revision = board.get(&d).await["revision"].clone();
    board
        .ok(
            &board.human(),
            "issue.update",
            json!({ "issue": d, "revision": revision, "order": 1.5 }),
        )
        .await;
    let worker = board.agent("worker");

    assert_eq!(
        board.ready_ids(&worker, 50).await,
        [c.as_str(), d.as_str(), b.as_str(), a.as_str(), e.as_str()]
    );
    assert_eq!(board.ready_ids(&worker, 2).await, [c.as_str(), d.as_str()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_blocks_relation_makes_its_target_ready_while_the_blocker_stays_open() {
    let board = start().await;
    board.project("DEM").await;
    let human = board.human();
    let worker = board.agent("worker");
    let blocker = board.issue("Write the parser").await;
    let target = board.issue("Ship the parser").await;
    let parent = board.issue("Parser epic").await;
    let child = board
        .issue_with(
            json!({ "acceptance": "It is done.", "title": "Parse numbers", "parent": parent }),
        )
        .await;
    board
        .ok(
            &human,
            "issue.link",
            json!({ "issue": blocker, "kind": "blocks", "target": target }),
        )
        .await;

    let waiting = board.get(&target).await;
    assert_eq!(waiting["ready"], false);
    assert_eq!(waiting["blockedBy"], json!([blocker]));
    assert_eq!(
        board.ready_ids(&worker, 50).await,
        [blocker.as_str(), child.as_str()]
    );

    board
        .ok(
            &human,
            "issue.link",
            json!({ "issue": blocker, "kind": "blocks", "target": target, "remove": true }),
        )
        .await;

    let freed = board.get(&target).await;
    assert_eq!(freed["ready"], true);
    assert_eq!(freed["blockedBy"], json!([]));
    assert_eq!(board.get(&blocker).await["status"], "todo");
    assert_eq!(
        board.ready_ids(&worker, 50).await,
        [blocker.as_str(), target.as_str(), child.as_str()]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ready_work_assigned_to_the_callers_role_comes_before_more_urgent_unassigned_work() {
    let board = start().await;
    board.project("DEM").await;
    let unassigned = board
        .issue_with(json!({ "acceptance": "It is done.", "title": "Urgent", "priority": 1 }))
        .await;
    let docs = board
        .issue_with(json!({
            "acceptance": "It is done.",
            "title": "Write the README",
            "priority": 3,
            "assignee": { "kind": "agent", "role": "docs" },
        }))
        .await;
    let mut writer = board.agent("writer");
    if let Some(actor) = writer.actor.as_mut() {
        actor["role"] = json!("docs");
        actor["name"] = json!("[docs]claude-code(t3)@writer");
    }

    assert_eq!(
        board.ready_ids(&writer, 50).await,
        [docs.as_str(), unassigned.as_str()]
    );
}
