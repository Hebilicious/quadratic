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
    use futures::poll;
    use jiff::{SignedDuration, Timestamp};
    use quadratique_server::app::{Context, application};
    use quadratique_server::domain::{Caller, WorkingCopy};
    use quadratique_server::store::{Author, Call, NewIssue, Store, StoreError};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use crate::support::{HANG_GUARD, within};

    const MAX_AGE_MS: i64 = 1500;
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

    fn create(store: &Store, title: &str) -> impl Future<Output = Result<(), StoreError>> {
        create_many(store, vec![new_issue(title)])
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

    #[tokio::test]
    async fn a_live_occurrence_older_than_max_age_is_skipped_and_the_next_one_is_truncated() {
        let home = crate::support::private_home();
        let store = Store::open(home.path().join("workspace.db")).expect("the store opens");
        let app = application().expect("the application");
        let human = Caller {
            actor: serde_json::from_str(HUMAN).expect("an actor"),
            working_copy: WorkingCopy::default(),
        };
        let session = Session::open(&app, "cli", Arc::new(Context::new(store.clone(), human)));
        let cancel = CancellationToken::new();
        let created = session
            .invoke(
                "project.create",
                json!({ "key": "DEM", "name": "Demo" }),
                &cancel,
            )
            .await;
        assert!(matches!(created, Outcome::Value(_)), "{created:?}");
        let mut live = session
            .subscribe(
                "issue.changed",
                json!({}),
                SubscribeOptions {
                    max_age_ms: Some(MAX_AGE_MS as f64),
                    ..SubscribeOptions::default()
                },
            )
            .expect("the subscription starts");

        let mut commits = store.subscribe();
        let mut aged = Box::pin(create(&store, "Aged"));
        assert!(poll!(&mut aged).is_pending());
        let guard = std::time::Instant::now() + HANG_GUARD;
        let aged_commit = loop {
            if let Ok(commit) = commits.try_recv() {
                break commit;
            }
            assert!(
                std::time::Instant::now() < guard,
                "the aged commit was not made"
            );
            std::thread::yield_now();
        };
        let at: Timestamp = aged_commit.at.parse().expect("a commit time");
        let old = at + SignedDuration::from_millis(MAX_AGE_MS + 1);
        while Timestamp::now() <= old {
            std::thread::yield_now();
        }
        aged.await.expect("the aged write ends");
        create(&store, "Fresh").await.expect("the fresh write ends");

        let first = tokio::time::timeout(HANG_GUARD, live.next())
            .await
            .expect("an occurrence within the hang guard")
            .expect("the subscription goes on")
            .expect("an occurrence");
        assert_eq!(first.data["issues"][0]["issue"]["title"], "Fresh");
        assert!(first.truncated, "{first:?}");
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
