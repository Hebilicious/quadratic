//! `issue.close` and `issues.close`, the names of `issue.finish` and `issues.finish` before 0.2, still answer until
//! the release after it; history written before the rename names the finish `issue.close`.

use serde_json::{Value, json};

use crate::support::start;

fn operations(history: &Value) -> Vec<&str> {
    history["items"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|change| change["operation"].as_str().expect("an operation"))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn issue_close_answers_as_issue_finish_does_and_its_history_names_the_finish() {
    let board = start().await;
    board.project("DEM").await;
    let by_old_name = board.issue("By the old name").await;
    let by_new_name = board.issue("By the new name").await;
    let worker = board.agent("parser");
    for issue in [&by_old_name, &by_new_name] {
        board
            .ok(&worker, "issue.claim", json!({ "issue": issue }))
            .await;
    }

    let closed = board
        .ok(
            &worker,
            "issue.close",
            json!({ "issue": by_old_name, "evidence": "cargo test: 3 passed" }),
        )
        .await;
    let finished = board
        .ok(
            &worker,
            "issue.finish",
            json!({ "issue": by_new_name, "evidence": "cargo test: 3 passed" }),
        )
        .await;

    for answer in [&closed, &finished] {
        assert_eq!(answer["issue"]["status"], "in-review", "{answer}");
        assert_eq!(
            answer["issue"]["evidence"], "cargo test: 3 passed",
            "{answer}"
        );
        assert_eq!(
            answer["issue"]["claim"]["holder"]["name"],
            worker.name(),
            "{answer}"
        );
    }
    for issue in [&by_old_name, &by_new_name] {
        let history = board
            .ok(&board.human(), "history.query", json!({ "issue": issue }))
            .await;
        assert_eq!(
            operations(&history),
            ["issue.finish", "issue.claim", "issue.create"],
            "{history}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn issues_close_finishes_several_and_refuses_with_its_old_tag_not_closed() {
    let board = start().await;
    board.project("DEM").await;
    let mine = board.issue("Mine").await;
    let theirs = board.issue("Theirs").await;
    let worker = board.agent("parser");
    board
        .ok(&worker, "issue.claim", json!({ "issue": mine }))
        .await;
    board
        .ok(
            &board.agent("other"),
            "issue.claim",
            json!({ "issue": theirs }),
        )
        .await;

    let refused = board
        .failure(
            &worker,
            "issues.close",
            json!({ "issue": [mine, theirs], "evidence": "checked" }),
        )
        .await;
    assert_eq!(refused["_tag"], "NotClosed", "{refused}");
    assert_eq!(refused["issue"], theirs.as_str(), "{refused}");
    assert_eq!(refused["failure"]["_tag"], "Held", "{refused}");
    assert_eq!(board.get(&mine).await["status"], "in-progress");

    let closed = board
        .ok(
            &worker,
            "issues.close",
            json!({ "issue": [mine], "evidence": "checked" }),
        )
        .await;
    assert_eq!(closed["items"][0]["status"], "in-review", "{closed}");
}

mod in_process {
    use std::sync::Arc;

    use acid_cat_core::{Outcome, Session};
    use quadratique_server::app::{Context, application};
    use quadratique_server::domain::{Caller, WorkingCopy};
    use quadratique_server::store::{Author, Call, IssueFields, Store, StoreError};
    use serde_json::{Value, json};
    use tokio_util::sync::CancellationToken;

    use super::operations;
    use crate::support::within;

    const HUMAN: &str = r#"{"kind":"human","name":"sam"}"#;

    async fn invoke(session: &Session<Context>, id: &str, input: Value) -> Value {
        match within(id, session.invoke(id, input, &CancellationToken::new())).await {
            Outcome::Value(value) => value,
            other => panic!("{id} did not answer a value: {other:?}"),
        }
    }

    /// What a build before 0.2 wrote when the human closed an issue: the operation `issue.close`.
    async fn closed_by_an_older_build(store: &Store, issue: &str) {
        let call = Call::new(Author::new(HUMAN).expect("an author"), "issue.close");
        let issue = issue.to_owned();
        store
            .write(call, move |tx| {
                tx.set_status(&issue, "merged")?;
                tx.update_fields(
                    &issue,
                    &IssueFields {
                        evidence: Some(Some("checked by hand".into())),
                        ..IssueFields::default()
                    },
                )?;
                Ok::<_, StoreError>(())
            })
            .await
            .expect("the older build's commit");
    }

    #[tokio::test]
    async fn history_query_reads_a_finish_written_before_the_rename_beside_one_written_after() {
        let home = crate::support::private_home();
        let store = Store::open(home.path().join("workspace.db")).expect("the store opens");
        let human = Caller {
            actor: serde_json::from_str(HUMAN).expect("an actor"),
            working_copy: WorkingCopy::default(),
        };
        let session = Session::open(
            &application().expect("the application"),
            "cli",
            Arc::new(Context::new(store.clone(), human)),
        );
        invoke(
            &session,
            "project.create",
            json!({ "key": "DEM", "name": "Demo" }),
        )
        .await;
        for title in ["Before the rename", "After the rename"] {
            invoke(
                &session,
                "issue.create",
                json!({ "project": "DEM", "title": title, "acceptance": "It is done." }),
            )
            .await;
        }

        closed_by_an_older_build(&store, "DEM-1").await;
        invoke(
            &session,
            "issue.finish",
            json!({ "issue": "DEM-2", "evidence": "checked by hand" }),
        )
        .await;

        let history = invoke(&session, "history.query", json!({ "project": "DEM" })).await;
        assert_eq!(
            operations(&history)[..2],
            ["issue.finish", "issue.close"],
            "{history}"
        );
        let items = history["items"].as_array().expect("a list");
        for (change, issue) in items[..2].iter().zip(["DEM-2", "DEM-1"]) {
            assert_eq!(change["issue"], issue, "{history}");
            assert!(
                change["fields"]
                    .as_array()
                    .expect("the changed fields")
                    .iter()
                    .any(|field| field["field"] == "status" && field["after"] == "\"merged\""),
                "{history}"
            );
        }
        for issue in ["DEM-1", "DEM-2"] {
            let shown = invoke(&session, "issue.get", json!({ "issue": issue })).await;
            assert_eq!(shown["issue"]["status"], "merged", "{shown}");
            assert_eq!(shown["issue"]["evidence"], "checked by hand", "{shown}");
        }
    }
}
