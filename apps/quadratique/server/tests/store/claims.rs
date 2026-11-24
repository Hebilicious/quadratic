use crate::support::{actor, call, open};
use quadratique_server::store::{AsOf, StoreError, Subject};
use tokio::task::JoinSet;

const ROUNDS: usize = 10;
const CONTENDERS: usize = 16;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn exactly_one_of_many_racing_claimers_wins_and_authors_the_commit() {
    let board = open();
    board.project("DEM").await;
    for round in 0..ROUNDS {
        let id = board.issue("DEM", &format!("Round {round}")).await.value;
        let mut racers = JoinSet::new();
        for contender in 0..CONTENDERS {
            let store = board.store.clone();
            let id = id.clone();
            let name = format!("agent-{contender}");
            racers.spawn(async move {
                let worktree = format!("/work/{name}");
                let written = store
                    .write(
                        call(&name, "issue.claim").working_copy("feature", &worktree),
                        {
                            let holder = actor(&name);
                            let key = name.clone();
                            move |tx| tx.take_claim(&id, &holder, &key, "feature", &worktree)
                        },
                    )
                    .await?;
                Ok::<_, StoreError>((name, written))
            });
        }
        let mut winners = Vec::new();
        while let Some(joined) = racers.join_next().await {
            let (name, written) = joined.expect("the task ends").expect("the write succeeds");
            assert_eq!(
                written.value,
                written.commit.is_some(),
                "round {round}: {name}"
            );
            if let Some(commit) = written.commit {
                winners.push((name, commit));
            }
        }
        assert_eq!(winners.len(), 1, "round {round}: winners {winners:?}");
        let (winner, commit) = &winners[0];
        assert_eq!(commit.author, actor(winner), "round {round}");
        assert_eq!(commit.issues, vec![id.clone()], "round {round}");

        let holder = board
            .store
            .read(AsOf::Now, {
                let id = id.clone();
                move |view| Ok::<_, StoreError>(view.issue(&id)?.and_then(|issue| issue.claim))
            })
            .await
            .expect("the issue is readable")
            .expect("the issue is claimed");
        assert_eq!(holder.holder, actor(winner), "round {round}");

        let latest = board.history(Subject::Issue(id.clone())).await.remove(0);
        assert_eq!(latest.commit, commit.hash, "round {round}");
        assert_eq!(latest.author, actor(winner), "round {round}");
    }
}
