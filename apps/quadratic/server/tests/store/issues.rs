use crate::support::{actor, call, open};
use quadratic_server::store::{AsOf, IssueCreated, IssueFilter, NewIssue, StoreError};
use tokio::task::JoinSet;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn issue_numbers_are_consecutive_per_project_under_concurrent_creates() {
    let board = open();
    board.project("DEM").await;
    board.project("OPS").await;
    let mut creators = JoinSet::new();
    for creator in 0..60 {
        let store = board.store.clone();
        let project = if creator % 2 == 0 { "DEM" } else { "OPS" };
        creators.spawn(async move {
            let issue = NewIssue {
                project: project.into(),
                title: format!("Issue from creator {creator}"),
                status: "todo".into(),
                created_by: actor(&format!("agent-{creator}")),
                ..NewIssue::default()
            };
            store
                .write(
                    call(&format!("agent-{creator}"), "issue.create"),
                    move |tx| tx.create_issue(&issue),
                )
                .await
        });
    }
    while let Some(joined) = creators.join_next().await {
        joined
            .expect("the task ends")
            .expect("the issue is created");
    }

    for project in ["DEM", "OPS"] {
        let filter = IssueFilter {
            project: Some(project.into()),
            ..IssueFilter::default()
        };
        let page = board
            .store
            .read(AsOf::Now, move |view| view.issues(&filter, None, 200))
            .await
            .expect("the issues are listed");
        let mut numbers: Vec<i64> = page.items.iter().map(|issue| issue.number).collect();
        numbers.sort_unstable();
        assert_eq!(numbers, (1..=30).collect::<Vec<_>>(), "{project}");
        assert!(
            page.items
                .iter()
                .all(|issue| issue.id == format!("{project}-{}", issue.number))
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_same_client_key_creates_one_issue() {
    let board = open();
    board.project("DEM").await;
    let mut creators = JoinSet::new();
    for creator in 0..12 {
        let store = board.store.clone();
        creators.spawn(async move {
            let issue = NewIssue {
                project: "DEM".into(),
                title: "Retried create".into(),
                status: "todo".into(),
                client_key: Some("client-key-1".into()),
                created_by: actor(&format!("agent-{creator}")),
                ..NewIssue::default()
            };
            store
                .write(call("agent", "issue.create"), move |tx| {
                    tx.create_issue(&issue)
                })
                .await
        });
    }
    let mut outcomes = Vec::new();
    while let Some(joined) = creators.join_next().await {
        let written = joined.expect("the task ends").expect("the write succeeds");
        outcomes.push((written.value, written.commit.is_some()));
    }
    let created: Vec<_> = outcomes
        .iter()
        .filter(|(outcome, _)| matches!(outcome, IssueCreated::Created(_)))
        .collect();
    assert_eq!(
        created,
        vec![&(IssueCreated::Created("DEM-1".into()), true)]
    );
    assert!(
        outcomes
            .iter()
            .filter(|(outcome, _)| matches!(outcome, IssueCreated::Existing(_)))
            .all(|outcome| outcome == &(IssueCreated::Existing("DEM-1".into()), false))
    );
    let count = board
        .store
        .read(AsOf::Now, |view| {
            Ok::<_, StoreError>(view.issues(&IssueFilter::default(), None, 200)?.items.len())
        })
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn listing_pages_give_every_matching_issue_once_in_order() {
    let board = open();
    board.project("DEM").await;
    for number in 1..=7 {
        let id = board.issue("DEM", &format!("Issue {number}")).await.value;
        if number % 2 == 1 {
            board
                .store
                .write(call("human", "issue.update"), move |tx| {
                    tx.set_labels(&id, &["odd".into()])
                })
                .await
                .expect("the label is set");
        }
    }
    board
        .store
        .write(call("human", "issue.update"), |tx| {
            tx.set_order("DEM-7", 0.5)
        })
        .await
        .expect("the order is set");

    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let filter = IssueFilter {
            labels: vec!["odd".into()],
            ..IssueFilter::default()
        };
        let from = cursor.clone();
        let page = board
            .store
            .read(AsOf::Now, move |view| {
                view.issues(&filter, from.as_deref(), 2)
            })
            .await
            .expect("a page is listed");
        let before = seen.len();
        seen.extend(page.items.into_iter().map(|issue| issue.id));
        let Some(next) = page.next_cursor else {
            break;
        };
        assert!(
            seen.len() > before && cursor.as_ref() != Some(&next) && seen.len() <= 4,
            "the listing makes no progress after {seen:?} at {next}"
        );
        cursor = Some(next);
    }
    assert_eq!(seen, ["DEM-7", "DEM-1", "DEM-3", "DEM-5"]);

    let refused = board
        .store
        .read(AsOf::Now, |view| {
            view.issues(&IssueFilter::default(), Some("h:1:DEM:"), 2)
        })
        .await;
    assert!(matches!(refused, Err(StoreError::BadCursor)), "{refused:?}");
}

#[tokio::test]
async fn an_assignee_filter_matches_whatever_the_order_of_its_keys() {
    let board = open();
    board.project("DEM").await;
    let issue = NewIssue {
        project: "DEM".into(),
        title: "Audit the tokens".into(),
        status: "todo".into(),
        assignee: Some(r#"{"role":"security","kind":"agent"}"#.into()),
        created_by: actor("human"),
        ..NewIssue::default()
    };
    board
        .store
        .write(call("human", "issue.create"), move |tx| {
            tx.create_issue(&issue)
        })
        .await
        .expect("the issue is created");
    let filter = IssueFilter {
        assignee: Some(r#"{"kind":"agent","role":"security"}"#.into()),
        ..IssueFilter::default()
    };
    let page = board
        .store
        .read(AsOf::Now, move |view| view.issues(&filter, None, 10))
        .await
        .expect("the issues are listed");
    assert_eq!(page.items.len(), 1);
}
