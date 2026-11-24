use crate::support::{call, open};
use quadratic_server::store::{AsOf, IssueFields, IssueFilter, StoreError};

#[tokio::test]
async fn a_read_as_of_an_earlier_commit_or_time_gives_the_earlier_state() {
    let board = open();
    board.project("DEM").await;
    let created = board.issue("DEM", "First title").await;
    let id = created.value;
    let earlier = created.commit.expect("a commit");
    board
        .store
        .write(call("agent-1", "issue.update"), {
            let id = id.clone();
            move |tx| {
                tx.update_fields(
                    &id,
                    &IssueFields {
                        title: Some("Second title".into()),
                        ..IssueFields::default()
                    },
                )
            }
        })
        .await
        .expect("the update succeeds");

    let first = Some("First title".to_owned());
    assert_eq!(
        board.title(AsOf::Now, &id).await.unwrap().as_deref(),
        Some("Second title")
    );
    assert_eq!(
        board
            .title(AsOf::Commit(earlier.hash.clone()), &id)
            .await
            .unwrap(),
        first
    );
    assert_eq!(
        board
            .title(AsOf::Commit(earlier.hash[..7].into()), &id)
            .await
            .unwrap(),
        first
    );
    assert_eq!(
        board
            .title(AsOf::Time(earlier.at.clone()), &id)
            .await
            .unwrap(),
        first
    );

    let before_everything = board
        .store
        .read(AsOf::Time("2000-01-01T00:00:00Z".into()), |view| {
            let page = view.issues(&IssueFilter::default(), None, 50)?;
            Ok::<_, StoreError>((view.projects()?.len(), page.items.len()))
        })
        .await
        .unwrap();
    assert_eq!(before_everything, (0, 0));
}

#[tokio::test]
async fn a_read_as_of_an_unknown_commit_is_refused() {
    let board = open();
    board.project("DEM").await;
    let refused = board
        .title(AsOf::Commit("0123456789abcdef".into()), "DEM-1")
        .await;
    assert!(
        matches!(&refused, Err(StoreError::UnknownCommit(hash)) if hash == "0123456789abcdef"),
        "{refused:?}"
    );
}
