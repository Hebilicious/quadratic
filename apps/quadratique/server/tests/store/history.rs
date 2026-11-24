use crate::support::{actor, call, open};
use quadratique_server::store::{FieldChange, IssueFields, Subject};
use serde_json::{Value, json};

fn field<'c>(fields: &'c [FieldChange], name: &str) -> &'c FieldChange {
    fields
        .iter()
        .find(|change| change.field == name)
        .unwrap_or_else(|| panic!("no change to {name} in {fields:?}"))
}

type Parsed = (String, Option<Value>, Option<Value>);

fn parsed(change: &FieldChange) -> Parsed {
    let value = |text: &Option<String>| {
        text.as_deref()
            .map(|text| serde_json::from_str(text).expect("a field change holds JSON text"))
    };
    (
        change.field.clone(),
        value(&change.before),
        value(&change.after),
    )
}

fn change(field: &str, before: Option<Value>, after: Option<Value>) -> Parsed {
    (field.into(), before, after)
}

#[tokio::test]
async fn the_author_operation_reason_branch_and_worktree_of_a_write_come_back() {
    let board = open();
    board.project("DEM").await;
    let id = board.issue("DEM", "Fix the claim").await.value;

    let written = board
        .store
        .write(
            call("[security]claude-code(t3)@fix-login", "issue.update")
                .reason("The plan changed")
                .working_copy("dem-1-fix", "/home/sam/work/fix-login"),
            {
                let id = id.clone();
                move |tx| tx.set_status(&id, "in-progress")
            },
        )
        .await
        .expect("the write succeeds");

    let latest = board.history(Subject::Issue(id.clone())).await.remove(0);
    assert_eq!(latest.commit, written.commit.expect("a commit").hash);
    assert_eq!(latest.author, actor("[security]claude-code(t3)@fix-login"));
    assert_eq!(latest.operation, "issue.update");
    assert_eq!(latest.reason.as_deref(), Some("The plan changed"));
    assert_eq!(latest.branch.as_deref(), Some("dem-1-fix"));
    assert_eq!(latest.worktree.as_deref(), Some("/home/sam/work/fix-login"));
    assert_eq!(
        (latest.project.as_str(), latest.issue.as_deref()),
        ("DEM", Some(id.as_str()))
    );
}

#[tokio::test]
async fn the_field_changes_of_an_update_come_back_exactly() {
    let board = open();
    board.project("DEM").await;
    let first = board.issue("DEM", "Old title").await.value;
    let second = board.issue("DEM", "Blocked work").await.value;
    board
        .store
        .write(call("human", "issue.update"), {
            let first = first.clone();
            move |tx| tx.set_labels(&first, &["store".into()])
        })
        .await
        .expect("the labels are set");

    board
        .store
        .write(call("agent-1", "issue.update"), {
            let (first, second) = (first.clone(), second.clone());
            move |tx| {
                tx.update_fields(
                    &first,
                    &IssueFields {
                        title: Some("New \"title\"".into()),
                        description: Some(Some("Line one\nLine two".into())),
                        ..IssueFields::default()
                    },
                )?;
                tx.set_labels(&first, &["store".into(), "history".into()])?;
                tx.add_relation(&first, "blocks", &second)
            }
        })
        .await
        .expect("the update succeeds");

    let first_change = board.history(Subject::Issue(first.clone())).await.remove(0);
    let fields = &first_change.fields;
    assert_eq!(
        parsed(field(fields, "title")),
        change(
            "title",
            Some(json!("Old title")),
            Some(json!("New \"title\""))
        )
    );
    assert_eq!(
        parsed(field(fields, "description")),
        change("description", None, Some(json!("Line one\nLine two")))
    );
    assert_eq!(
        parsed(field(fields, "labels")),
        change(
            "labels",
            Some(json!(["store"])),
            Some(json!(["history", "store"]))
        )
    );
    assert_eq!(
        parsed(field(fields, "relations")),
        change(
            "relations",
            None,
            Some(json!({ "kind": "blocks", "direction": "outgoing", "issue": "DEM-2" }))
        )
    );
    assert_eq!(
        parsed(field(fields, "revision")),
        change("revision", Some(json!(2)), Some(json!(3)))
    );
    let mut names: Vec<&str> = fields.iter().map(|change| change.field.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "description",
            "labels",
            "relations",
            "revision",
            "title",
            "updatedAt"
        ]
    );

    let second_change = board.history(Subject::Issue(second)).await.remove(0);
    assert_eq!(second_change.commit, first_change.commit);
    assert_eq!(
        second_change.fields.iter().map(parsed).collect::<Vec<_>>(),
        vec![change(
            "relations",
            None,
            Some(json!({ "kind": "blocks", "direction": "incoming", "issue": "DEM-1" }))
        )]
    );
}

#[tokio::test]
async fn a_workflow_change_comes_back_as_a_change_to_the_project() {
    let board = open();
    board.project("DEM").await;
    let mut workflow = crate::support::workflow();
    workflow.statuses.retain(|status| status.key != "backlog");
    board
        .store
        .write(
            call("human", "project.update").reason("No backlog"),
            move |tx| tx.replace_workflow("DEM", &workflow),
        )
        .await
        .expect("the workflow is replaced");

    let latest = board
        .history(Subject::Project("DEM".into()))
        .await
        .remove(0);
    assert_eq!(
        (latest.issue, latest.reason.as_deref()),
        (None, Some("No backlog"))
    );
    let [change] = latest.fields.as_slice() else {
        panic!("one field change expected: {:?}", latest.fields);
    };
    assert_eq!(change.field, "workflow");
    let status_keys = |workflow: &Option<String>| -> Vec<String> {
        let workflow: serde_json::Value =
            serde_json::from_str(workflow.as_deref().expect("a workflow")).expect("JSON");
        workflow["statuses"]
            .as_array()
            .expect("statuses")
            .iter()
            .map(|status| status["key"].as_str().expect("a key").to_owned())
            .collect()
    };
    assert_eq!(
        status_keys(&change.before).first().map(String::as_str),
        Some("backlog")
    );
    assert_eq!(
        status_keys(&change.after).first().map(String::as_str),
        Some("todo")
    );
}

#[tokio::test]
async fn a_reopened_store_keeps_the_board_and_its_history() {
    let board = open();
    board.project("DEM").await;
    let id = board.issue("DEM", "Survives a restart").await.value;
    let history = board.history(Subject::Board).await;

    let board = board.reopen();
    assert_eq!(board.history(Subject::Board).await, history);
    assert_eq!(
        board
            .title(quadratique_server::store::AsOf::Now, &id)
            .await
            .unwrap()
            .as_deref(),
        Some("Survives a restart")
    );
    let next = board.issue("DEM", "After the restart").await;
    assert_eq!(next.value, "DEM-2");
    assert!(next.commit.expect("a commit").at > history[0].at);
}

#[tokio::test]
async fn an_actor_name_with_characters_the_engine_cannot_take_authors_its_commit_exactly() {
    let board = open();
    board.project("DEM").await;
    let name = "claude-code@review<draft>\tone";

    let written = board
        .store
        .write(call(name, "project.update"), |tx| {
            tx.update_project(
                "DEM",
                &quadratique_server::store::ProjectChanges {
                    name: Some("Renamed".into()),
                    ..Default::default()
                },
            )
        })
        .await
        .expect("the write succeeds");

    assert!(written.commit.is_some());
    let latest = board
        .history(Subject::Project("DEM".into()))
        .await
        .remove(0);
    assert_eq!(latest.author, actor(name));
}

#[tokio::test]
async fn a_claim_and_its_release_come_back_as_changes_to_the_claim_field() {
    let board = open();
    board.project("DEM").await;
    let id = board.issue("DEM", "Claimed work").await.value;
    let holder = actor("claude-code@feature");
    board
        .store
        .write(call("claude-code@feature", "issue.claim"), {
            let (id, holder) = (id.clone(), holder.clone());
            move |tx| {
                tx.take_claim(
                    &id,
                    &holder,
                    "claude-code@feature",
                    "feature",
                    "/work/feature",
                )
            }
        })
        .await
        .expect("the claim is written");
    let claimed = board.history(Subject::Issue(id.clone())).await.remove(0);
    let (_, before, after) = parsed(field(&claimed.fields, "claim"));
    let after = after.expect("a claim after the write");
    assert_eq!(before, None);
    assert_eq!(
        after["holder"],
        serde_json::from_str::<Value>(&holder).expect("the holder is JSON")
    );
    assert_eq!(after["branch"], "feature");
    assert_eq!(after["worktree"], "/work/feature");
    assert!(after["since"].is_string(), "{after}");

    board
        .store
        .write(call("human", "issue.release"), {
            let id = id.clone();
            move |tx| tx.clear_claim(&id)
        })
        .await
        .expect("the release is written");
    let released = board.history(Subject::Issue(id)).await.remove(0);
    let (_, before, after) = parsed(field(&released.fields, "claim"));
    assert_eq!(before, Some(after_claim(&claimed)));
    assert_eq!(after, None);
}

fn after_claim(change: &quadratique_server::store::Change) -> Value {
    parsed(field(&change.fields, "claim"))
        .2
        .expect("a claim after the write")
}
