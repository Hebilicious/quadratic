use crate::support::{HANG_GUARD, call, open};
use quadratique_server::store::{AsOf, IssueFields, StoreError, Subject};

#[derive(Debug, PartialEq)]
enum Refused {
    Store(String),
    Rule,
}

impl From<StoreError> for Refused {
    fn from(error: StoreError) -> Self {
        Refused::Store(error.to_string())
    }
}

#[tokio::test]
async fn a_write_that_fails_or_changes_nothing_leaves_no_commit_and_no_row_change() {
    let board = open();
    board.project("DEM").await;
    let id = board.issue("DEM", "Original").await.value;
    let history_before = board.history(Subject::Board).await;
    let mut commits = board.store.subscribe();

    let failed = board
        .store
        .write(call("agent-1", "issue.update"), {
            let id = id.clone();
            move |tx| {
                tx.update_fields(
                    &id,
                    &IssueFields {
                        title: Some("Changed, then refused".into()),
                        ..IssueFields::default()
                    },
                )?;
                tx.set_status(&id, "in-progress")?;
                Err::<(), _>(Refused::Rule)
            }
        })
        .await;
    assert_eq!(failed.unwrap_err(), Refused::Rule);

    let unchanged = board
        .store
        .write(call("agent-1", "issue.update"), {
            let id = id.clone();
            move |tx| {
                tx.update_fields(
                    &id,
                    &IssueFields {
                        title: Some("Original".into()),
                        ..IssueFields::default()
                    },
                )?;
                tx.set_status(&id, "todo")
            }
        })
        .await
        .expect("the write succeeds");
    assert!(!unchanged.value);
    assert_eq!(unchanged.commit, None);

    let issue = board
        .store
        .read(AsOf::Now, {
            let id = id.clone();
            move |view| view.issue(&id)
        })
        .await
        .expect("the issue is readable")
        .expect("the issue exists");
    assert_eq!(
        (issue.title.as_str(), issue.status.as_str(), issue.revision),
        ("Original", "todo", 1)
    );
    assert_eq!(board.history(Subject::Board).await, history_before);
    assert!(commits.try_recv().is_err(), "no commit reached subscribers");
}

#[tokio::test]
async fn each_commit_reaches_subscribers_with_what_it_changed() {
    let board = open();
    let mut commits = board.store.subscribe();
    board.project("DEM").await;
    let created = board.issue("DEM", "Watched").await;

    let mut next_commit = async || {
        tokio::time::timeout(HANG_GUARD, commits.recv())
            .await
            .expect("a commit within the hang guard")
            .expect("the commit arrives")
    };
    let project_commit = next_commit().await;
    assert_eq!(
        (
            project_commit.operation.as_str(),
            project_commit.projects.as_slice()
        ),
        ("project.create", ["DEM".to_owned()].as_slice())
    );
    assert!(project_commit.issues.is_empty());

    let issue_commit = next_commit().await;
    assert_eq!(Some(&issue_commit), created.commit.as_ref());
    assert_eq!(issue_commit.issues, vec!["DEM-1".to_owned()]);
    assert!(
        issue_commit.projects.is_empty(),
        "a new issue number is not a change to the project"
    );
    assert_eq!(issue_commit.author, crate::support::actor("human"));
}

#[tokio::test]
async fn a_write_that_panics_leaves_no_change_and_the_store_keeps_serving() {
    let board = open();
    board.project("DEM").await;
    let id = board.issue("DEM", "Before the panic").await.value;

    let panicked = board
        .store
        .write(call("agent-1", "issue.update"), {
            let id = id.clone();
            move |tx| -> Result<(), StoreError> {
                tx.set_status(&id, "in-progress")?;
                panic!("the work panics inside the transaction")
            }
        })
        .await;
    assert!(matches!(panicked, Err(StoreError::Closed)), "{panicked:?}");

    let next = board.issue("DEM", "After the panic").await;
    assert_eq!(next.value, "DEM-2");
    let issue = board
        .store
        .read(AsOf::Now, move |view| view.issue(&id))
        .await
        .expect("the issue is readable")
        .expect("the issue exists");
    assert_eq!(issue.status, "todo");
}
