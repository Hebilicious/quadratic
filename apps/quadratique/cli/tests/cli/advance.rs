use serde_json::json;

use crate::support::{Home, Repository, git, git_stdout};

fn head(repository: &Repository) -> String {
    git_stdout(&repository.root, &["rev-parse", "HEAD"])
}

#[test]
fn a_deploy_script_moves_the_landed_issue_and_a_refused_deploy_exits_with_its_failure() {
    let home = Home::new();
    let repository = Repository::new();
    let quadratique = || home.quadratique(&repository.root);
    quadratique()
        .args(["project", "create", "--key", "DEM", "--name", "Demo"])
        .run()
        .ok();
    let workflow = json!({
        "statuses": [
            { "key": "todo", "name": "Todo", "category": "unstarted" },
            { "key": "in-progress", "name": "In progress", "category": "started" },
            { "key": "in-review", "name": "In review", "category": "started" },
            { "key": "merged", "name": "Merged", "category": "started" },
            { "key": "deployed", "name": "Deployed", "category": "started" },
            { "key": "done", "name": "Done", "category": "completed" },
        ],
        "moves": { "claim": "in-progress", "review": "in-review", "merge": "merged", "release": "todo" },
    });
    quadratique()
        .args(["project", "update", "--project", "DEM", "--workflow"])
        .args([workflow.to_string()])
        .run()
        .ok();
    quadratique()
        .args([
            "create",
            "--title",
            "Parse the headers",
            "--acceptance",
            "It is done.",
        ])
        .run()
        .ok();
    let before = head(&repository);
    let first_look = quadratique()
        .args(["issue", "advance", "--project", "DEM", "--commit", &before])
        .args(["--status", "deployed"])
        .run()
        .ok();
    assert_eq!(first_look["moved"]["count"], 0, "{first_look}");
    git(
        &repository.root,
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "--message",
            "Parse the headers\n\nRefs: DEM-1",
        ],
    );
    let landed = head(&repository);

    let advanced = quadratique()
        .args(["issue", "advance", "--project", "DEM", "--commit", &landed])
        .args(["--status", "deployed"])
        .run()
        .ok();

    assert_eq!(
        advanced["moved"],
        json!({ "count": 1, "issues": ["DEM-1"] })
    );
    assert_eq!(advanced["left"], json!({ "count": 0, "issues": [] }));
    let shown = quadratique().args(["show", "--issue", "DEM-1"]).run().ok();
    assert_eq!(shown["issue"]["status"], "deployed");
    let refused = quadratique()
        .args(["issue", "advance", "--project", "DEM", "--commit", &landed])
        .args(["--status", "in-review"])
        .run();
    assert_eq!(refused.code, 1, "{refused:?}");
    assert_eq!(refused.json()["_tag"], "NotAfterMerge", "{refused:?}");
}
