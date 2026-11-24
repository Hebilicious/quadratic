use serde_json::{Value, json};

use crate::support::{Running, Who, start};

fn default_workflow() -> Value {
    json!({
        "statuses": [
            { "key": "backlog", "name": "Backlog", "category": "backlog" },
            { "key": "todo", "name": "Todo", "category": "unstarted" },
            { "key": "in-progress", "name": "In progress", "category": "started" },
            { "key": "in-review", "name": "In review", "category": "started" },
            { "key": "merged", "name": "Merged", "category": "started" },
            { "key": "done", "name": "Done", "category": "completed" },
            { "key": "canceled", "name": "Canceled", "category": "canceled" },
        ],
        "moves": { "claim": "in-progress", "review": "in-review", "merge": "merged", "release": "todo" },
    })
}

fn other_repository(board: &Running) -> Who {
    Who {
        repository: Some(board.other_repository()),
        ..board.human()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_human_changes_a_project_and_project_update_refuses_other_callers_and_bound_repositories()
 {
    let board = start().await;
    let created = board.project("DEM").await;
    assert_eq!(created["workflow"], default_workflow());
    assert_eq!(created["defaultBranch"], "main");

    let mut deployed = default_workflow();
    deployed["statuses"].as_array_mut().unwrap().insert(
        5,
        json!({ "key": "deployed", "name": "Deployed", "category": "started" }),
    );
    deployed["statuses"][3]["name"] = json!("Review");
    let updated = board
        .ok(
            &board.human(),
            "project.update",
            json!({ "project": "DEM", "workflow": deployed, "extra": { "team": "parser" }, "reason": "A deploy step." }),
        )
        .await;
    assert_eq!(updated["workflow"], deployed);
    assert_eq!(updated["extra"], json!({ "team": "parser" }));
    let listed = board.ok(&board.human(), "project.list", json!({})).await;
    assert_eq!(listed["items"][0]["workflow"], deployed);

    let failure = board
        .failure(
            &board.agent("meddler"),
            "project.update",
            json!({ "project": "DEM", "name": "Mine" }),
        )
        .await;
    assert_eq!(failure, json!({ "_tag": "HumanOnly" }));

    let failure = board
        .failure(
            &board.human(),
            "project.update",
            json!({ "project": "NOPE", "name": "Nope" }),
        )
        .await;
    assert_eq!(
        failure,
        json!({ "_tag": "NotFound", "kind": "project", "id": "NOPE" })
    );

    let web = Who {
        repository: None,
        worktree: None,
        branch: None,
        ..board.human()
    };
    let failure = board
        .failure(
            &web,
            "project.update",
            json!({ "project": "DEM", "bindRepository": true }),
        )
        .await;
    assert_eq!(failure, json!({ "_tag": "NoWorkingCopy" }));

    board
        .ok(
            &other_repository(&board),
            "project.create",
            json!({ "key": "SITE", "name": "Site" }),
        )
        .await;
    let failure = board
        .failure(
            &board.human(),
            "project.update",
            json!({ "project": "SITE", "bindRepository": true }),
        )
        .await;
    assert_eq!(
        failure,
        json!({ "_tag": "RepositoryBound", "project": "DEM" })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn project_create_refuses_a_taken_key_and_a_repository_bound_to_another_project() {
    let board = start().await;
    board.project("DEM").await;
    let failure = board
        .failure(
            &other_repository(&board),
            "project.create",
            json!({ "key": "DEM", "name": "Again" }),
        )
        .await;
    assert_eq!(failure, json!({ "_tag": "KeyTaken", "project": "DEM" }));

    let failure = board
        .failure(
            &board.human(),
            "project.create",
            json!({ "key": "TWIN", "name": "Twin" }),
        )
        .await;
    assert_eq!(
        failure,
        json!({ "_tag": "RepositoryBound", "project": "DEM" })
    );

    let listed = board.ok(&board.human(), "project.list", json!({})).await;
    assert_eq!(listed["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(listed["items"][0]["repository"], board.repository());
}
