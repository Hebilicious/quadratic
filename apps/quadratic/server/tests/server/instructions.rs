use serde_json::json;

use crate::support::{Who, start};

#[tokio::test(flavor = "multi_thread")]
async fn instructions_tell_the_caller_who_and_where_it_is_from_its_headers() {
    let board = start().await;
    board.project("DEM").await;
    let worker = board.agent("parser");
    let instructions = board.ok(&worker, "workspace.instructions", json!({})).await;
    assert_eq!(instructions["actor"], worker.actor.clone().unwrap());
    assert_eq!(instructions["project"], "DEM");
    assert_eq!(instructions["branch"], "parser");
    assert_eq!(instructions["worktree"], worker.worktree());
    let text = instructions["text"].as_str().expect("a text");
    assert!(text.contains(&worker.name()), "{text}");

    let anonymous = Who {
        actor: None,
        repository: None,
        worktree: None,
        branch: None,
    };
    let instructions = board
        .ok(&anonymous, "workspace.instructions", json!({}))
        .await;
    assert_eq!(
        instructions["actor"],
        json!({ "kind": "agent", "name": "agent" })
    );
    assert!(instructions.get("project").is_none());
}
