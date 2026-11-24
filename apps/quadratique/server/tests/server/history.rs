use serde_json::json;

use crate::support::{Who, ids, start};

#[tokio::test(flavor = "multi_thread")]
async fn history_tells_who_changed_an_issue_how_and_where_and_as_of_reads_show_the_past() {
    let board = start().await;
    board.project("DEM").await;
    let issue = board.issue("Old title").await;
    let worker = board.agent("historian");

    let claimed = board
        .ok(&worker, "issue.claim", json!({ "issue": issue }))
        .await;
    let before_rename = board
        .ok(
            &board.human(),
            "history.query",
            json!({ "issue": issue, "limit": 1 }),
        )
        .await["items"][0]["commit"]
        .as_str()
        .expect("a commit")
        .to_owned();
    board
        .ok(
            &worker,
            "issue.update",
            json!({
                "issue": issue,
                "revision": claimed["issue"]["revision"],
                "title": "New title",
                "reason": "The old title was vague.",
            }),
        )
        .await;

    let history = board
        .ok(&board.human(), "history.query", json!({ "issue": issue }))
        .await;
    let changes = history["items"].as_array().expect("changes");
    let operations: Vec<&str> = changes
        .iter()
        .map(|change| change["operation"].as_str().expect("an operation"))
        .collect();
    assert_eq!(operations, ["issue.update", "issue.claim", "issue.create"]);

    let rename = &changes[0];
    assert_eq!(rename["author"]["name"], worker.name());
    assert_eq!(rename["author"]["host"], "t3");
    assert_eq!(rename["branch"], "historian");
    assert_eq!(rename["worktree"], worker.worktree());
    assert_eq!(rename["reason"], "The old title was vague.");
    assert_eq!(rename["issue"], issue);
    assert_eq!(rename["project"], "DEM");
    assert!(rename["at"].as_str().is_some_and(|at| at.ends_with('Z')));
    assert_eq!(changes[2]["author"]["name"], "sam");

    let then = board
        .ok(
            &board.human(),
            "issue.get",
            json!({ "issue": issue, "asOf": before_rename }),
        )
        .await;
    assert_eq!(then["issue"]["title"], "Old title");
    assert_eq!(then["issue"]["status"], "in-progress");
    let now = board.get(&issue).await;
    assert_eq!(now["title"], "New title");
    assert!(
        now.get("history").is_none(),
        "issue.get leaves the changes to history.query: {now}"
    );

    let listed_then = board
        .ok(
            &board.human(),
            "issue.list",
            json!({ "asOf": before_rename, "text": "Old" }),
        )
        .await;
    assert_eq!(ids(&listed_then["items"]), std::slice::from_ref(&issue));
    let listed_now = board
        .ok(&board.human(), "issue.list", json!({ "text": "Old" }))
        .await;
    assert_eq!(listed_now["items"], json!([]));

    let failure = board
        .failure(
            &board.human(),
            "issue.get",
            json!({ "issue": issue, "asOf": "abcdef0" }),
        )
        .await;
    assert_eq!(
        failure,
        json!({ "_tag": "UnknownCommit", "commit": "abcdef0" })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn history_of_an_issue_filtered_by_another_project_has_no_changes() {
    let board = start().await;
    board.project("DEM").await;
    let unbound = Who {
        repository: None,
        ..board.human()
    };
    board
        .ok(
            &unbound,
            "project.create",
            json!({ "key": "OTH", "name": "Other" }),
        )
        .await;
    let issue = board
        .issue_with(json!({ "project": "DEM", "acceptance": "It is done.", "title": "Ours" }))
        .await;

    let own = board
        .ok(
            &board.human(),
            "history.query",
            json!({ "issue": issue, "project": "DEM" }),
        )
        .await;
    assert_eq!(own["items"].as_array().map(Vec::len), Some(1));
    let other = board
        .ok(
            &board.human(),
            "history.query",
            json!({ "issue": issue, "project": "OTH" }),
        )
        .await;
    assert_eq!(other["items"], json!([]));
}
