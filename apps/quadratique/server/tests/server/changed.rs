use std::collections::BTreeSet;

use acid_cat_core::{CallFailure, Occurrence};
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};

use crate::support::{HANG_GUARD, reasons, start, within};

type Occurrences = BoxStream<'static, Result<Occurrence, CallFailure>>;

async fn next(occurrences: &mut Occurrences) -> Occurrence {
    match tokio::time::timeout(HANG_GUARD, occurrences.next()).await {
        Ok(Some(Ok(occurrence))) => occurrence,
        Ok(other) => panic!("the subscription ended: {other:?}"),
        Err(_) => panic!("no occurrence came within the hang guard of {HANG_GUARD:?}"),
    }
}

fn changed<'a>(occurrence: &'a Occurrence, id: &str) -> &'a Value {
    occurrence.data["issues"]
        .as_array()
        .expect("issues")
        .iter()
        .find(|changed| changed["issue"]["id"] == id)
        .unwrap_or_else(|| panic!("{id} is not in {}", occurrence.data))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subscriber_sees_each_commit_with_the_issues_it_made_ready_and_resumes_from_a_cursor() {
    let board = start().await;
    board.project("DEM").await;
    let blocker = board.issue("Write the parser").await;
    let blocked = board.issue("Ship the parser").await;
    board
        .ok(
            &board.human(),
            "issue.link",
            json!({ "issue": blocker, "kind": "blocks", "target": blocked }),
        )
        .await;
    let watcher = board.client(&board.agent("watcher"));
    let mut live = within(
        "the subscription",
        watcher.subscribe("issue.changed", &json!({}), None, None),
    )
    .await
    .expect("the subscription starts");

    let revision = board.get(&blocker).await["revision"].clone();
    board
        .ok(
            &board.human(),
            "issue.update",
            json!({ "issue": blocker, "revision": revision, "status": "done" }),
        )
        .await;
    let resolved = next(&mut live).await;
    assert_eq!(resolved.data["operation"], "issue.update");
    assert_eq!(resolved.data["author"]["name"], "sam");
    assert_eq!(resolved.data["projects"], json!([]));
    assert_eq!(resolved.cursor.as_deref(), resolved.data["commit"].as_str());
    let done = changed(&resolved, &blocker);
    assert_eq!(done["issue"]["status"], "done");
    assert!(
        done["fields"]
            .as_array()
            .unwrap()
            .contains(&json!("status"))
    );
    let now_ready = changed(&resolved, &blocked);
    assert_eq!(now_ready["issue"]["ready"], true);
    assert!(
        now_ready["fields"]
            .as_array()
            .unwrap()
            .contains(&json!("ready"))
    );

    let cursor = resolved.cursor.clone().expect("a cursor");
    let claimed = board
        .ok(
            &board.agent("worker"),
            "issue.claim",
            json!({ "issue": blocked }),
        )
        .await;
    let created = board.issue("Document the parser").await;
    let claim_commit = next(&mut live).await;
    let create_commit = next(&mut live).await;
    drop(live);
    assert_eq!(
        changed(&claim_commit, &blocked)["issue"]["revision"],
        claimed["issue"]["revision"]
    );
    changed(&create_commit, &created);

    let mut resumed = within(
        "the resumed subscription",
        watcher.subscribe("issue.changed", &json!({}), Some(&cursor), None),
    )
    .await
    .expect("the subscription resumes");
    let first = next(&mut resumed).await;
    let second = next(&mut resumed).await;
    assert_eq!(first.data["commit"], claim_commit.data["commit"]);
    assert_eq!(first.data["operation"], "issue.claim");
    assert_eq!(second.data["commit"], create_commit.data["commit"]);
    assert!(!first.truncated && !second.truncated);

    let mut one_issue = within(
        "the filtered subscription",
        watcher.subscribe(
            "issue.changed",
            &json!({ "issue": created }),
            Some(&cursor),
            None,
        ),
    )
    .await
    .expect("the filtered subscription resumes");
    let only = next(&mut one_issue).await;
    assert_eq!(only.data["commit"], create_commit.data["commit"]);
    assert_eq!(only.data["issues"].as_array().map(Vec::len), Some(1));
}

mod in_process {
    use std::sync::Arc;

    use acid_cat_core::{Occurrence, Occurrences, Outcome, Session, SubscribeOptions};
    use quadratique_server::app::{Context, application};
    use quadratique_server::domain::{Caller, WorkingCopy};
    use quadratique_server::store::{Author, Call, NewIssue, Store, StoreError};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use crate::support::within;

    const HUMAN: &str = r#"{"kind":"human","name":"sam"}"#;

    fn new_issue(title: &str) -> NewIssue {
        NewIssue {
            project: "DEM".into(),
            title: title.into(),
            status: "todo".into(),
            created_by: HUMAN.into(),
            ..NewIssue::default()
        }
    }

    fn create_many(
        store: &Store,
        issues: Vec<NewIssue>,
    ) -> impl Future<Output = Result<(), StoreError>> {
        let call = Call::new(Author::new(HUMAN).expect("an author"), "issue.create");
        let written = store.write(call, move |tx| {
            issues
                .iter()
                .try_for_each(|issue| tx.create_issue(issue).map(|_| ()))
        });
        async move { written.await.map(|_| ()) }
    }

    struct Board {
        store: Store,
        session: Session<Context>,
        cancel: CancellationToken,
        _home: tempfile::TempDir,
    }

    async fn board() -> Board {
        let home = crate::support::private_home();
        let store = Store::open(home.path().join("workspace.db")).expect("the store opens");
        let app = application().expect("the application");
        let human = Caller {
            actor: serde_json::from_str(HUMAN).expect("an actor"),
            working_copy: WorkingCopy::default(),
        };
        let session = Session::open(&app, "cli", Arc::new(Context::new(store.clone(), human)));
        let cancel = CancellationToken::new();
        let created = within(
            "project.create",
            session.invoke(
                "project.create",
                json!({ "key": "DEM", "name": "Demo" }),
                &cancel,
            ),
        )
        .await;
        assert!(matches!(created, Outcome::Value(_)), "{created:?}");
        Board {
            store,
            session,
            cancel,
            _home: home,
        }
    }

    async fn next_occurrence(live: &mut Occurrences) -> Occurrence {
        within("the next occurrence", live.next())
            .await
            .expect("the subscription goes on")
            .expect("an occurrence")
    }

    #[tokio::test]
    async fn a_change_to_a_project_alone_gives_an_occurrence_that_names_the_project() {
        let board = board().await;
        let mut live = board
            .session
            .subscribe("issue.changed", json!({}), SubscribeOptions::default())
            .expect("the subscription starts");

        let renamed = within(
            "project.update",
            board.session.invoke(
                "project.update",
                json!({ "project": "DEM", "name": "Demo renamed" }),
                &board.cancel,
            ),
        )
        .await;
        assert!(matches!(renamed, Outcome::Value(_)), "{renamed:?}");

        let occurrence = next_occurrence(&mut live).await;
        assert_eq!(occurrence.data["operation"], "project.update");
        assert_eq!(occurrence.data["projects"], json!(["DEM"]));
        assert_eq!(occurrence.data["issues"], json!([]));
    }

    #[tokio::test]
    async fn a_commit_that_changes_more_than_500_issues_gives_500_and_says_it_was_cut() {
        let board = board().await;
        let mut live = board
            .session
            .subscribe("issue.changed", json!({}), SubscribeOptions::default())
            .expect("the subscription starts");

        let issues = (1..=501)
            .map(|n| new_issue(&format!("Issue {n}")))
            .collect();
        within("the large write", create_many(&board.store, issues))
            .await
            .expect("the large write ends");

        let occurrence = next_occurrence(&mut live).await;
        assert_eq!(occurrence.data["cut"], true);
        assert_eq!(
            occurrence.data["issues"].as_array().map(Vec::len),
            Some(500)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_of_why_an_issue_is_not_ready_names_not_ready_in_its_fields() {
    let board = start().await;
    board.project("DEM").await;
    let blocker = board.issue("Write the parser").await;
    let blocked = board.issue("Ship the parser").await;
    board
        .ok(
            &board.human(),
            "issue.link",
            json!({ "issue": blocker, "kind": "blocks", "target": blocked }),
        )
        .await;
    let watcher = board.client(&board.agent("watcher"));
    let mut live = within(
        "the subscription",
        watcher.subscribe("issue.changed", &json!({ "issue": blocked }), None, None),
    )
    .await
    .expect("the subscription starts");

    let revision = board.get(&blocked).await["revision"].clone();
    board
        .ok(
            &board.human(),
            "issue.update",
            json!({ "issue": blocked, "revision": revision, "status": "backlog" }),
        )
        .await;
    let moved = next(&mut live).await;
    let shelved = changed(&moved, &blocked);
    assert_eq!(
        reasons(&shelved["issue"]["notReady"]),
        BTreeSet::from(["category", "blocked"])
    );
    assert!(
        shelved["fields"]
            .as_array()
            .unwrap()
            .contains(&json!("notReady")),
        "{shelved}"
    );
}
