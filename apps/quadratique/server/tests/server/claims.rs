use std::collections::BTreeSet;

use serde_json::json;
use tokio::task::JoinSet;

use crate::support::{Who, call, ids, reasons, start};

const CONTENDERS: usize = 8;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn every_agent_that_loses_a_claim_over_http_is_told_the_committed_holder() {
    let board = start().await;
    board.project("DEM").await;
    let issue = board.issue("Contested").await;
    let mut racers = JoinSet::new();
    for index in 0..CONTENDERS {
        let who = board.agent(&format!("racer-{index}"));
        let (name, client) = (who.name(), board.client(&who));
        let issue = issue.clone();
        racers.spawn(async move {
            let outcome = call(&client, "issue.claim", json!({ "issue": issue })).await;
            (name, outcome)
        });
    }
    let mut winners = Vec::new();
    let mut losers = Vec::new();
    while let Some(joined) = racers.join_next().await {
        match joined.expect("the racer ends") {
            (name, Ok(summary)) => winners.push((name, summary)),
            (name, Err(failure)) => losers.push((name, failure)),
        }
    }

    let stored = board.get(&issue).await;
    let holder = &stored["claim"]["holder"]["name"];
    assert_eq!(winners.len(), 1, "winners {winners:?}");
    assert_eq!(*holder, winners[0].0);
    assert_eq!(losers.len(), CONTENDERS - 1);
    for (name, failure) in &losers {
        assert_eq!(failure["_tag"], "Held", "{name} got {failure}");
        assert_eq!(failure["issue"], issue, "{name} got {failure}");
        assert_eq!(failure["claim"]["holder"]["name"], *holder, "{name}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn claim_mine_lists_what_the_caller_holds_in_its_own_worktree() {
    let board = start().await;
    board.project("DEM").await;
    let ours = board.issue("Ours").await;
    let theirs = board.issue("Theirs").await;
    let me = board.agent("mine");
    board.ok(&me, "issue.claim", json!({ "issue": ours })).await;
    board
        .ok(
            &board.agent("other"),
            "issue.claim",
            json!({ "issue": theirs }),
        )
        .await;

    let mine = board
        .ok(&me, "issue.list", json!({ "claim": "mine" }))
        .await;
    assert_eq!(ids(&mine["items"]), vec![ours]);
    let mut elsewhere = me.clone();
    elsewhere.worktree = board.agent("copy").worktree;
    let none = board
        .ok(&elsewhere, "issue.list", json!({ "claim": "mine" }))
        .await;
    assert_eq!(none["items"], json!([]));
}

fn in_session(mut who: Who, harness: &str, session: &str) -> Who {
    let actor = who.actor.as_mut().expect("an actor");
    actor["harness"] = json!(harness);
    actor["session"] = json!(session);
    who
}

#[tokio::test(flavor = "multi_thread")]
async fn claim_mine_lists_the_claims_of_the_callers_session_in_every_worktree() {
    let board = start().await;
    board.project("DEM").await;
    let here = board.issue("Here").await;
    let there = board.issue("There").await;
    let theirs = board.issue("Theirs").await;
    let lead = in_session(board.agent("lead"), "claude-code", "session-1");
    let lead_elsewhere = in_session(board.agent("audit"), "claude-code", "session-1");
    let other = in_session(board.agent("other"), "claude-code", "session-2");
    board
        .ok(&lead, "issue.claim", json!({ "issue": here }))
        .await;
    board
        .ok(&lead_elsewhere, "issue.claim", json!({ "issue": there }))
        .await;
    board
        .ok(&other, "issue.claim", json!({ "issue": theirs }))
        .await;

    let mine = board
        .ok(&lead, "issue.list", json!({ "claim": "mine" }))
        .await;

    assert_eq!(ids(&mine["items"]), vec![here.clone(), there.clone()]);
    let worktrees: Vec<_> = mine["items"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|item| {
            item["claim"]["worktree"]
                .as_str()
                .expect("a worktree")
                .to_owned()
        })
        .collect();
    assert_eq!(worktrees, vec![lead.worktree(), lead_elsewhere.worktree()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn claim_mine_without_a_session_is_the_same_name_in_the_same_worktree() {
    let board = start().await;
    board.project("DEM").await;
    let here = board.issue("Here").await;
    let there = board.issue("There").await;
    let earlier = in_session(board.agent("lead"), "claude-code", "session-1");
    let elsewhere = in_session(board.agent("audit"), "claude-code", "session-1");
    board
        .ok(&earlier, "issue.claim", json!({ "issue": here }))
        .await;
    board
        .ok(&elsewhere, "issue.claim", json!({ "issue": there }))
        .await;

    let mine = board
        .ok(
            &board.agent("lead"),
            "issue.list",
            json!({ "claim": "mine" }),
        )
        .await;

    assert_eq!(ids(&mine["items"]), vec![here]);
}

#[tokio::test(flavor = "multi_thread")]
async fn claim_mine_keeps_the_claims_of_the_same_name_in_the_same_worktree_from_another_session() {
    let board = start().await;
    board.project("DEM").await;
    let earlier = board.issue("Earlier").await;
    board
        .ok(
            &in_session(board.agent("lead"), "claude-code", "session-1"),
            "issue.claim",
            json!({ "issue": earlier }),
        )
        .await;

    let mine = board
        .ok(
            &in_session(board.agent("lead"), "claude-code", "session-2"),
            "issue.list",
            json!({ "claim": "mine" }),
        )
        .await;

    assert_eq!(ids(&mine["items"]), vec![earlier]);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_sessions_with_the_same_actor_name_in_two_worktrees_do_not_see_each_others_claims() {
    let board = start().await;
    board.project("DEM").await;
    let first = board.issue("First").await;
    let second = board.issue("Second").await;
    let one = in_session(board.agent("lead"), "claude-code", "session-1");
    let mut two = in_session(board.agent("lead"), "claude-code", "session-2");
    two.worktree = Some(format!("{}/copy/lead", board.agent("x").worktree()));
    assert_eq!(one.name(), two.name());
    board
        .ok(&one, "issue.claim", json!({ "issue": first }))
        .await;
    board
        .ok(&two, "issue.claim", json!({ "issue": second }))
        .await;

    let seen_by_one = board
        .ok(&one, "issue.list", json!({ "claim": "mine" }))
        .await;
    let seen_by_two = board
        .ok(&two, "issue.list", json!({ "claim": "mine" }))
        .await;

    assert_eq!(ids(&seen_by_one["items"]), vec![first]);
    assert_eq!(ids(&seen_by_two["items"]), vec![second]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_session_id_from_another_harness_is_another_session() {
    let board = start().await;
    board.project("DEM").await;
    let theirs = board.issue("Theirs").await;
    board
        .ok(
            &in_session(board.agent("audit"), "codex", "shared-id"),
            "issue.claim",
            json!({ "issue": theirs }),
        )
        .await;

    let mine = board
        .ok(
            &in_session(board.agent("lead"), "claude-code", "shared-id"),
            "issue.list",
            json!({ "claim": "mine" }),
        )
        .await;

    assert_eq!(mine["items"], json!([]));
}

fn without_harness(mut who: Who, session: &str) -> Who {
    let actor = who
        .actor
        .as_mut()
        .expect("an actor")
        .as_object_mut()
        .expect("an object");
    actor.remove("harness");
    actor.insert("session".into(), json!(session));
    who
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_id_without_a_harness_is_no_session() {
    let board = start().await;
    board.project("DEM").await;
    let here = board.issue("Here").await;
    let there = board.issue("There").await;
    let lead = without_harness(board.agent("lead"), "session-1");
    board
        .ok(&lead, "issue.claim", json!({ "issue": here }))
        .await;
    board
        .ok(
            &without_harness(board.agent("audit"), "session-1"),
            "issue.claim",
            json!({ "issue": there }),
        )
        .await;

    let mine = board
        .ok(&lead, "issue.list", json!({ "claim": "mine" }))
        .await;

    assert_eq!(ids(&mine["items"]), vec![here]);
}

#[tokio::test(flavor = "multi_thread")]
async fn claim_mine_of_a_human_is_its_own_name_in_this_worktree_whatever_session_it_declares() {
    let board = start().await;
    board.project("DEM").await;
    let theirs = board.issue("Theirs").await;
    board
        .ok(
            &in_session(board.agent("audit"), "claude-code", "session-1"),
            "issue.claim",
            json!({ "issue": theirs }),
        )
        .await;

    let mine = board
        .ok(
            &in_session(board.human(), "claude-code", "session-1"),
            "issue.list",
            json!({ "claim": "mine" }),
        )
        .await;

    assert_eq!(mine["items"], json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_gives_the_issues_description_and_acceptance() {
    let board = start().await;
    board.project("DEM").await;
    let issue = board
        .issue_with(json!({
            "acceptance": "It is done.",
            "title": "Count words",
            "description": "Return a dict of word counts.",
            "acceptance": "The tests pass.",
        }))
        .await;
    let me = board.agent("mine");

    let claimed = board
        .ok(&me, "issue.claim", json!({ "issue": issue }))
        .await;

    assert_eq!(claimed["issue"]["id"], issue);
    assert_eq!(
        claimed["issue"]["description"],
        "Return a dict of word counts."
    );
    assert_eq!(claimed["issue"]["acceptance"], "The tests pass.");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_release_gives_the_issue_back_unclaimed_in_the_release_status() {
    let board = start().await;
    board.project("DEM").await;
    let issue = board.issue("Count words").await;
    let me = board.agent("mine");
    board
        .ok(&me, "issue.claim", json!({ "issue": issue }))
        .await;

    let released = board
        .ok(&me, "issue.release", json!({ "issue": issue }))
        .await;

    assert_eq!(released["issue"]["id"], issue);
    assert_eq!(released["issue"]["status"], "todo");
    assert!(released["issue"].get("claim").is_none(), "{released}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_issue_says_why_it_is_not_ready_and_that_the_caller_holds_it() {
    let board = start().await;
    board.project("DEM").await;
    let held = board.issue("Held").await;
    let free = board.issue("Free").await;
    let me = board.agent("mine");
    board.ok(&me, "issue.claim", json!({ "issue": held })).await;

    let listed = board.ok(&me, "issue.list", json!({})).await;

    let summary = |id: &str| {
        listed["items"]
            .as_array()
            .expect("a list")
            .iter()
            .find(|summary| summary["id"] == id)
            .unwrap_or_else(|| panic!("{id} is not in {listed}"))
            .clone()
    };
    assert_eq!(
        reasons(&summary(&held)["notReady"]),
        BTreeSet::from(["category", "mine"])
    );
    let free_summary = summary(&free);
    assert!(free_summary.get("notReady").is_none(), "{free_summary}");
}

mod in_process {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::mpsc;

    use acid_cat_core::{Outcome, Session};
    use futures::poll;
    use quadratique_server::app::{Context, application};
    use quadratique_server::domain::{Caller, WorkingCopy};
    use quadratique_server::store::{Author, Call, Store, StoreError};
    use serde_json::{Value, json};
    use tokio_util::sync::CancellationToken;

    use crate::support::HANG_GUARD;

    fn caller(code: &Path, actor: Value, worktree: &str) -> Caller {
        let text = |path: PathBuf| Some(path.display().to_string());
        Caller {
            actor: serde_json::from_value(actor).expect("an actor"),
            working_copy: WorkingCopy {
                repository: text(code.join(".git")),
                worktree: text(code.join(worktree)),
                branch: Some(worktree.into()),
                contains: None,
            },
        }
    }

    fn agent(code: &Path, worktree: &str) -> Caller {
        let name = format!("claude-code@{worktree}");
        caller(
            code,
            json!({ "kind": "agent", "name": name, "harness": "claude-code" }),
            worktree,
        )
    }

    fn value(outcome: Outcome) -> Value {
        match outcome {
            Outcome::Value(value) => value,
            other => panic!("the call failed: {other:?}"),
        }
    }

    fn quiet_write() -> Call {
        Call::new(
            Author::new(r#"{"kind":"agent","name":"test"}"#).expect("an author"),
            "test.gate",
        )
    }

    #[tokio::test]
    async fn a_repeated_claim_answers_with_its_own_claim_when_others_write_right_after_it() {
        let home = crate::support::private_home();
        let store = Store::open(home.path().join("workspace.db")).expect("the store opens");
        let app = application().expect("the application");
        let open =
            |who: Caller| Session::open(&app, "cli", Arc::new(Context::new(store.clone(), who)));
        let code = home.path().join("code");
        let human = open(caller(
            &code,
            json!({ "kind": "human", "name": "sam" }),
            "main",
        ));
        let first = open(agent(&code, "first"));
        let second = open(agent(&code, "second"));
        let cancel = CancellationToken::new();

        value(
            human
                .invoke(
                    "project.create",
                    json!({ "key": "DEM", "name": "Demo" }),
                    &cancel,
                )
                .await,
        );
        value(
            human
                .invoke(
                    "issue.create",
                    json!({ "acceptance": "It is done.", "title": "Contested" }),
                    &cancel,
                )
                .await,
        );
        let issue = json!({ "issue": "DEM-1" });
        value(first.invoke("issue.claim", issue.clone(), &cancel).await);

        let (open_gate, gate) = mpsc::channel::<()>();
        let mut gated = Box::pin(store.write(quiet_write(), move |_| {
            let _ = gate.recv_timeout(HANG_GUARD);
            Ok::<(), StoreError>(())
        }));
        assert!(poll!(&mut gated).is_pending());
        let mut repeated = Box::pin(first.invoke("issue.claim", issue.clone(), &cancel));
        assert!(poll!(&mut repeated).is_pending());
        let release = json!({ "issue": "DEM-1", "reason": "Reassigned" });
        let mut released = Box::pin(human.invoke("issue.release", release, &cancel));
        assert!(poll!(&mut released).is_pending());
        let mut taken = Box::pin(second.invoke("issue.claim", issue, &cancel));
        assert!(poll!(&mut taken).is_pending());
        let (reached, reaching) = mpsc::channel::<()>();
        let mut marker = Box::pin(store.write(quiet_write(), move |_| {
            let _ = reached.send(());
            Ok::<(), StoreError>(())
        }));
        assert!(poll!(&mut marker).is_pending());
        open_gate.send(()).expect("the gate is waiting");
        reaching
            .recv_timeout(HANG_GUARD)
            .expect("the writer served every queued write");

        let repeated = value(repeated.await);
        assert_eq!(
            repeated["issue"]["claim"]["holder"]["name"],
            "claude-code@first"
        );
        value(released.await);
        assert_eq!(
            value(taken.await)["issue"]["claim"]["holder"]["name"],
            "claude-code@second"
        );
        gated.await.expect("the gate write ends");
        marker.await.expect("the marker write ends");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn close_answers_the_whole_issue_with_the_evidence_it_saved() {
    let board = start().await;
    board.project("DEM").await;
    let issue = board.issue("Ship the parser").await;
    let worker = board.agent("parser");
    board
        .ok(&worker, "issue.claim", json!({ "issue": issue }))
        .await;

    let closed = board
        .ok(
            &worker,
            "issue.close",
            json!({ "issue": issue, "evidence": "cargo test: 12 passed at 1a2b3c4" }),
        )
        .await;

    assert_eq!(
        closed["issue"]["evidence"], "cargo test: 12 passed at 1a2b3c4",
        "{closed}"
    );
    let mut shown = board
        .ok(&worker, "issue.get", json!({ "issue": issue }))
        .await;
    let work = shown["issue"]
        .as_object_mut()
        .expect("an issue")
        .remove("work");
    assert_eq!(work, Some(json!({ "branch": "parser", "commits": [] })));
    assert_eq!(closed["issue"], shown["issue"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_that_names_a_worktree_other_than_the_callers_is_refused_and_records_nothing() {
    let board = start().await;
    board.project("DEM").await;
    let issue = board.issue("Named").await;
    let me = board.agent("dem-12");
    let before = board.get(&issue).await;

    for named in [board.agent("other").worktree(), board.human().worktree()] {
        let refused = board
            .failure(
                &me,
                "issue.claim",
                json!({ "issue": issue, "worktree": named }),
            )
            .await;
        assert_eq!(refused["_tag"], "NoWorkingCopy", "{named}: {refused}");
    }
    let mut in_another_repository = me.clone();
    in_another_repository.repository = Some(board.other_repository());
    let refused = board
        .failure(
            &in_another_repository,
            "issue.claim",
            json!({ "issue": issue, "worktree": me.worktree() }),
        )
        .await;
    assert_eq!(refused["_tag"], "NoWorkingCopy", "{refused}");
    assert_eq!(board.get(&issue).await, before);

    let claimed = board
        .ok(
            &me,
            "issue.claim",
            json!({ "issue": issue, "worktree": me.worktree() }),
        )
        .await;
    assert_eq!(claimed["issue"]["claim"]["worktree"], me.worktree());
    assert_eq!(claimed["issue"]["claim"]["branch"], "dem-12");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_that_names_a_relative_worktree_is_refused_before_anything_is_recorded() {
    let board = start().await;
    board.project("DEM").await;
    let issue = board.issue("Relative").await;
    let me = board.agent("dem-12");
    let before = board.get(&issue).await;

    let answer = board
        .client(&me)
        .invoke(
            "issue.claim",
            &json!({ "issue": issue, "worktree": ".worktrees/dem-12" }),
        )
        .await;

    match answer {
        acid_cat_core::Outcome::CallFailure(failure) => {
            assert_eq!(failure.kind.as_str(), "invalid-input", "{failure:?}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(board.get(&issue).await, before);
}
