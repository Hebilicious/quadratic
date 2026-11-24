use acid_cat_core::Outcome;
use serde_json::json;

use crate::support::start;

#[tokio::test(flavor = "multi_thread")]
async fn a_repeated_client_key_gives_the_first_issue_and_a_different_input_conflicts() {
    let board = start().await;
    board.project("DEM").await;
    let worker = board.agent("finder");
    let origin = board.issue("Port the tokenizer").await;
    let input = json!({
        "key": "finder-7f3a",
        "acceptance": "It is done.",
        "title": "Tokenizer drops tabs",
        "description": "Found while porting.",
        "labels": ["bug", "parser"],
        "priority": 2,
        "origin": origin,
    });

    let first = board.ok(&worker, "issue.create", input.clone()).await;
    assert_eq!(first["created"], true);
    let id = first["issue"]["id"].as_str().expect("an id").to_owned();
    let relations = board.get(&id).await["relations"].clone();
    assert_eq!(relations[0]["kind"], "discovered-from");
    assert_eq!(relations[0]["issue"], origin);

    board
        .ok(&worker, "issue.claim", json!({ "issue": id }))
        .await;

    let repeated = board.ok(&worker, "issue.create", input.clone()).await;
    assert_eq!(repeated["created"], false);
    assert_eq!(repeated["issue"]["id"], id);
    assert_eq!(repeated["issue"]["status"], "in-progress");

    let mut changed = input.clone();
    changed["title"] = json!("Tokenizer drops spaces");
    let failure = board.failure(&worker, "issue.create", changed).await;
    assert_eq!(failure, json!({ "_tag": "KeyConflict", "issue": id }));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repeated_keyed_create_gives_the_first_issue_after_the_workflow_changed() {
    let board = start().await;
    let project = board.project("DEM").await;
    let input =
        json!({ "key": "finder-1", "acceptance": "It is done.", "title": "Tokenizer drops tabs" });
    let first = board
        .ok(&board.agent("finder"), "issue.create", input.clone())
        .await;

    let mut workflow = project["workflow"].clone();
    let statuses = workflow["statuses"].as_array_mut().expect("statuses");
    statuses.insert(
        1,
        json!({ "key": "triage", "name": "Triage", "category": "unstarted" }),
    );
    board
        .ok(
            &board.human(),
            "project.update",
            json!({ "project": "DEM", "workflow": workflow }),
        )
        .await;

    let repeated = board
        .ok(&board.agent("finder"), "issue.create", input)
        .await;
    assert_eq!(repeated["created"], false);
    assert_eq!(repeated["issue"]["id"], first["issue"]["id"]);
}

fn refused_as_invalid_input(answer: &Outcome) -> bool {
    matches!(answer, Outcome::CallFailure(failure) if failure.kind.as_str() == "invalid-input")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_create_without_acceptance_criteria_is_refused_and_creates_nothing() {
    let board = start().await;
    board.project("DEM").await;
    let human = board.client(&board.human());

    for input in [
        json!({ "title": "No criteria" }),
        json!({ "title": "Empty criteria", "acceptance": "" }),
        json!({ "title": "Blank criteria", "acceptance": " \n\t " }),
    ] {
        let answer = human.invoke("issue.create", &input).await;
        assert!(refused_as_invalid_input(&answer), "{input}: {answer:?}");
    }
    let listed = board.ok(&board.human(), "issue.list", json!({})).await;
    let created = board
        .ok(
            &board.human(),
            "issue.create",
            json!({ "title": "With criteria", "acceptance": "`cargo test` passes." }),
        )
        .await;

    assert_eq!(listed["items"], json!([]), "{listed}");
    assert_eq!(created["issue"]["id"], "DEM-1");
    assert_eq!(
        board.get("DEM-1").await["acceptance"],
        "`cargo test` passes."
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_can_replace_the_acceptance_criteria_but_not_clear_or_blank_them() {
    let board = start().await;
    board.project("DEM").await;
    let issue = board.issue("Keep the criteria").await;
    let revision = board.get(&issue).await["revision"].clone();
    let human = board.client(&board.human());

    let cleared = human
        .invoke(
            "issue.update",
            &json!({ "issue": issue, "revision": revision, "clear": ["acceptance"] }),
        )
        .await;
    let blanked = human
        .invoke(
            "issue.update",
            &json!({ "issue": issue, "revision": revision, "acceptance": "   " }),
        )
        .await;
    let unchanged = board.get(&issue).await["acceptance"].clone();
    board
        .ok(
            &board.human(),
            "issue.update",
            json!({ "issue": issue, "revision": revision, "acceptance": "Both tests pass." }),
        )
        .await;

    assert!(refused_as_invalid_input(&cleared), "{cleared:?}");
    assert!(refused_as_invalid_input(&blanked), "{blanked:?}");
    assert_eq!(unchanged, "It is done.");
    assert_eq!(board.get(&issue).await["acceptance"], "Both tests pass.");
}
