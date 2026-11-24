use serde_json::{Value, json};

use crate::support::{Running, start};

fn finding(key: &str, title: &str) -> Value {
    json!({
        "key": key,
        "title": title,
        "description": format!("{title}, as the checker reports it."),
    })
}

fn with(mut finding: Value, members: Value) -> Value {
    for (name, value) in members.as_object().expect("members") {
        finding[name] = value.clone();
    }
    finding
}

fn fact(id: &str) -> Value {
    json!({ "kind": "fact", "id": id })
}

fn file(path: &str) -> Value {
    json!({ "kind": "file", "id": path })
}

async fn import(board: &Running, findings: Vec<Value>) -> Value {
    board
        .ok(
            &board.human(),
            "findings.import",
            json!({ "tool": "linter", "findings": findings }),
        )
        .await
}

async fn import_partial(board: &Running, findings: Vec<Value>) -> Value {
    board
        .ok(
            &board.human(),
            "findings.import",
            json!({ "tool": "linter", "findings": findings, "complete": false }),
        )
        .await
}

fn ids(answer: &Value, group: &str) -> Vec<String> {
    answer[group]["issues"]
        .as_array()
        .unwrap_or_else(|| panic!("no {group} in {answer}"))
        .iter()
        .map(|id| id.as_str().expect("an id").to_owned())
        .collect()
}

async fn issue_of(board: &Running, key: &str) -> Value {
    let listed = board
        .ok(
            &board.human(),
            "issue.list",
            json!({ "tool": "linter", "limit": 200 }),
        )
        .await;
    for item in listed["items"].as_array().expect("issues") {
        let issue = board.get(item["id"].as_str().expect("an id")).await;
        if issue["source"]["key"] == key {
            return issue;
        }
    }
    panic!("no issue has the key {key}: {listed}");
}

async fn last_commit(board: &Running) -> Value {
    board
        .ok(&board.human(), "history.query", json!({ "limit": 1 }))
        .await["items"][0]["commit"]
        .clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn each_new_finding_becomes_an_issue_with_its_source_and_links() {
    let board = start().await;
    board.project("DEM").await;

    let answer = import(
        &board,
        vec![with(
            finding("L204 f:49f3eaf2628a1f35", "Move the age to the baseline"),
            json!({
                "assignee": { "kind": "human" },
                "priority": 2,
                "labels": ["cosmos"],
                "links": [fact("cosmos.universe.age"), { "kind": "file", "id": "cosmos/universe.md" }],
            }),
        )],
    )
    .await;

    assert_eq!(answer["created"]["count"], 1, "{answer}");
    let issue = board.get(&ids(&answer, "created")[0]).await;
    assert_eq!(
        issue["source"],
        json!({ "tool": "linter", "key": "L204 f:49f3eaf2628a1f35" })
    );
    assert_eq!(
        issue["links"],
        json!([fact("cosmos.universe.age"), { "kind": "file", "id": "cosmos/universe.md" }])
    );
    assert_eq!(issue["title"], "Move the age to the baseline");
    assert_eq!(issue.get("acceptance"), None, "{issue}");
    assert_eq!(
        issue["description"],
        "Move the age to the baseline, as the checker reports it."
    );
    assert_eq!(issue["status"], "todo");
    assert_eq!(issue["assignee"], json!({ "kind": "human" }));
    assert_eq!(issue["priority"], 2);
    assert_eq!(issue["labels"], json!(["cosmos"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_finding_changes_only_what_it_gives() {
    let board = start().await;
    board.project("DEM").await;
    import(
        &board,
        vec![with(
            finding("K1", "First title"),
            json!({ "labels": ["law"] }),
        )],
    )
    .await;
    let worker = board.agent("worker");
    let id = issue_of(&board, "K1").await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    board
        .ok(&worker, "issue.claim", json!({ "issue": id }))
        .await;
    let revision = board.get(&id).await["revision"].clone();
    board
        .ok(
            &worker,
            "issue.update",
            json!({ "issue": id, "revision": revision, "acceptance": "The law is read first.", "notes": "Started." }),
        )
        .await;
    let other = board.issue("Made by hand").await;
    board
        .ok(
            &board.human(),
            "issue.link",
            json!({ "issue": id, "kind": "related", "target": other }),
        )
        .await;
    let before = board.get(&id).await;

    let answer = import(&board, vec![finding("K1", "Second title")]).await;

    assert_eq!(ids(&answer, "updated"), [id.as_str()], "{answer}");
    let after = board.get(&id).await;
    assert_eq!(after["title"], "Second title");
    assert_eq!(
        after["description"],
        "Second title, as the checker reports it."
    );
    for kept in [
        "status",
        "claim",
        "acceptance",
        "notes",
        "relations",
        "labels",
        "evidence",
    ] {
        assert_eq!(after[kept], before[kept], "{kept} changed");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_complete_run_finishes_the_issue_of_a_gone_finding_and_a_partial_run_does_not() {
    let board = start().await;
    board.project("DEM").await;
    import(&board, vec![finding("K1", "Stays"), finding("K2", "Goes")]).await;
    let gone = issue_of(&board, "K2").await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    board
        .ok(
            &board.agent("worker"),
            "issue.claim",
            json!({ "issue": gone }),
        )
        .await;

    let partial = import_partial(&board, vec![finding("K1", "Stays")]).await;

    assert_eq!(partial["finished"]["count"], 0, "{partial}");
    assert_eq!(board.get(&gone).await["status"], "in-progress");

    let complete = import(&board, vec![finding("K1", "Stays")]).await;

    assert_eq!(ids(&complete, "finished"), [gone.as_str()], "{complete}");
    let finished = board.get(&gone).await;
    assert_eq!(finished["status"], "done");
    assert!(finished.get("claim").is_none(), "{finished}");
    let evidence = finished["evidence"].as_str().expect("evidence");
    assert!(
        evidence.contains("complete run of linter") && evidence.contains("sam"),
        "{evidence}"
    );
    assert_eq!(issue_of(&board, "K1").await["status"], "todo");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finding_that_comes_back_reopens_its_finished_issue() {
    let board = start().await;
    board.project("DEM").await;
    import(&board, vec![finding("K1", "Comes back")]).await;
    import(&board, vec![]).await;
    let id = issue_of(&board, "K1").await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    assert_eq!(board.get(&id).await["status"], "done");

    let answer = import(&board, vec![finding("K1", "Comes back")]).await;

    assert_eq!(ids(&answer, "reopened"), [id.as_str()], "{answer}");
    assert_eq!(board.get(&id).await["status"], "todo");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_canceled_issue_stays_canceled_whatever_the_run_reports() {
    let board = start().await;
    board.project("DEM").await;
    import(&board, vec![finding("K1", "Overruled")]).await;
    let id = issue_of(&board, "K1").await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let revision = board.get(&id).await["revision"].clone();
    board
        .ok(
            &board.human(),
            "issue.update",
            json!({ "issue": id, "revision": revision, "status": "canceled" }),
        )
        .await;

    let reported = import(&board, vec![finding("K1", "Overruled, retitled")]).await;
    let gone = import(&board, vec![]).await;

    assert_eq!(ids(&reported, "left"), [id.as_str()], "{reported}");
    assert_eq!(gone["finished"]["count"], 0, "{gone}");
    let issue = board.get(&id).await;
    assert_eq!(issue["status"], "canceled");
    assert_eq!(issue["title"], "Overruled");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_run_imported_twice_writes_no_second_commit() {
    let board = start().await;
    board.project("DEM").await;
    let run = vec![
        with(
            finding("D1", "Decide"),
            json!({ "decision": true, "links": [fact("a")], "labels": ["law"] }),
        ),
        with(finding("K1", "Work"), json!({ "links": [fact("a")] })),
    ];
    import(&board, run.clone()).await;
    let commit = last_commit(&board).await;

    let again = import(&board, run).await;

    assert_eq!(last_commit(&board).await, commit);
    assert_eq!(again["left"]["count"], 2, "{again}");
    for group in ["created", "updated", "reopened", "finished"] {
        assert_eq!(again[group]["count"], 0, "{group}: {again}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_decision_blocks_the_issues_that_share_a_fact_until_it_no_longer_does_and_a_shared_file_never_blocks()
 {
    let board = start().await;
    board.project("DEM").await;
    let decision = with(
        finding("D1", "Decide the age"),
        json!({ "decision": true, "assignee": { "kind": "human" }, "links": [fact("cosmos.universe.age"), file("cosmos/universe.md")] }),
    );
    let sharing = with(
        finding("K1", "Reword the age"),
        json!({ "links": [fact("cosmos.universe.age")] }),
    );
    let same_file = with(
        finding("K2", "Reword the season"),
        json!({ "links": [fact("seasons.dawn"), file("cosmos/universe.md")] }),
    );
    import(
        &board,
        vec![decision.clone(), sharing.clone(), same_file.clone()],
    )
    .await;
    let decision_id = issue_of(&board, "D1").await["id"].clone();
    let worker = board.agent("worker");

    let blocked = issue_of(&board, "K1").await;
    assert_eq!(blocked["blockedBy"], json!([decision_id]), "{blocked}");
    let beside = issue_of(&board, "K2").await;
    assert_eq!(
        beside["ready"], true,
        "a shared file does not block: {beside}"
    );

    let unlinked = with(
        finding("K1", "Reword the age"),
        json!({ "links": [fact("cosmos.universe.length")] }),
    );
    import(&board, vec![decision.clone(), unlinked, same_file.clone()]).await;
    let freed = issue_of(&board, "K1").await;
    assert_eq!(freed["blockedBy"], json!([]), "{freed}");
    assert_eq!(freed["relations"], json!([]), "{freed}");

    import(&board, vec![decision, sharing.clone(), same_file.clone()]).await;
    assert_eq!(
        issue_of(&board, "K1").await["blockedBy"],
        json!([decision_id])
    );

    import(&board, vec![sharing, same_file]).await;
    let freed = issue_of(&board, "K1").await;
    assert_eq!(freed["relations"], json!([]), "{freed}");
    assert_eq!(
        board.ready_ids(&worker, 50).await.len(),
        2,
        "both issues are ready once the decision is finished"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finding_that_a_later_run_reports_without_decision_stops_blocking() {
    let board = start().await;
    board.project("DEM").await;
    let work = with(finding("K1", "Work"), json!({ "links": [fact("a")] }));
    import(
        &board,
        vec![
            with(
                finding("D1", "Decide"),
                json!({ "decision": true, "links": [fact("a")] }),
            ),
            work.clone(),
        ],
    )
    .await;
    assert_eq!(issue_of(&board, "K1").await["ready"], false);

    import(
        &board,
        vec![
            with(finding("D1", "Decide"), json!({ "links": [fact("a")] })),
            work,
        ],
    )
    .await;

    let freed = issue_of(&board, "K1").await;
    assert_eq!(freed["blockedBy"], json!([]), "{freed}");
    assert_eq!(freed["relations"], json!([]), "{freed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relation_made_by_hand_survives_every_import() {
    let board = start().await;
    board.project("DEM").await;
    let decision = with(
        finding("D1", "Decide"),
        json!({ "decision": true, "links": [fact("a")] }),
    );
    let work = with(finding("K1", "Work"), json!({ "links": [fact("b")] }));
    import(&board, vec![decision.clone(), work.clone()]).await;
    let decision_id = issue_of(&board, "D1").await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let work_id = issue_of(&board, "K1").await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    board
        .ok(
            &board.human(),
            "issue.link",
            json!({ "issue": decision_id, "kind": "blocks", "target": work_id }),
        )
        .await;

    import(&board, vec![decision, work.clone()]).await;
    import(&board, vec![work]).await;

    let relations = board.get(&work_id).await["relations"].clone();
    assert_eq!(relations[0]["kind"], "blocks", "{relations}");
    assert_eq!(relations[0]["issue"], decision_id, "{relations}");
}

#[tokio::test(flavor = "multi_thread")]
async fn two_findings_with_one_key_are_refused_and_change_nothing() {
    let board = start().await;
    board.project("DEM").await;

    let failure = board
        .failure(
            &board.human(),
            "findings.import",
            json!({ "tool": "linter", "findings": [finding("K1", "One"), finding("K1", "Two")] }),
        )
        .await;

    assert_eq!(failure, json!({ "_tag": "DuplicateKey", "key": "K1" }));
    let listed = board
        .ok(&board.human(), "issue.list", json!({ "tool": "linter" }))
        .await;
    assert_eq!(listed["items"], json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reopened_issue_loses_the_evidence_that_the_import_wrote_at_its_finish() {
    let board = start().await;
    board.project("DEM").await;
    import(&board, vec![finding("K1", "Comes back")]).await;
    import(&board, vec![]).await;
    let id = issue_of(&board, "K1").await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    assert!(
        board.get(&id).await["evidence"].is_string(),
        "the finish wrote evidence"
    );

    import(&board, vec![finding("K1", "Comes back")]).await;

    let reopened = board.get(&id).await;
    assert_eq!(reopened["status"], "todo");
    assert!(reopened.get("evidence").is_none(), "{reopened}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reopened_issue_keeps_evidence_that_an_agent_wrote_after_the_import_finished_it() {
    let board = start().await;
    board.project("DEM").await;
    import(&board, vec![finding("K1", "Fixed by hand")]).await;
    import(&board, vec![]).await;
    let id = issue_of(&board, "K1").await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let human = board.human();
    let revision = board.get(&id).await["revision"].clone();
    board
        .ok(
            &human,
            "issue.update",
            json!({ "issue": id, "revision": revision, "status": "todo" }),
        )
        .await;
    let worker = board.agent("worker");
    board
        .ok(&worker, "issue.claim", json!({ "issue": id }))
        .await;
    board
        .ok(
            &worker,
            "issue.close",
            json!({ "issue": id, "evidence": "Reworded the span; cargo test passes." }),
        )
        .await;
    let revision = board.get(&id).await["revision"].clone();
    board
        .ok(
            &human,
            "issue.update",
            json!({ "issue": id, "revision": revision, "status": "done" }),
        )
        .await;

    import(&board, vec![finding("K1", "Fixed by hand")]).await;

    let reopened = board.get(&id).await;
    assert_eq!(reopened["status"], "todo");
    assert_eq!(
        reopened["evidence"], "Reworded the span; cargo test passes.",
        "{reopened}"
    );
}

fn dense_run(decisions: usize, others: usize) -> Vec<Value> {
    let decision = (0..decisions).map(|n| {
        with(
            finding(&format!("D{n}"), "Decide"),
            json!({ "decision": true, "links": [fact("shared")] }),
        )
    });
    let other = (0..others).map(|n| {
        with(
            finding(&format!("K{n}"), "Work"),
            json!({ "links": [fact("shared")] }),
        )
    });
    decision.chain(other).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_implies_more_blocks_than_the_bound_is_refused_and_changes_nothing() {
    let board = start().await;
    board.project("DEM").await;

    let failure = board
        .failure(
            &board.human(),
            "findings.import",
            json!({ "tool": "linter", "findings": dense_run(2, 501) }),
        )
        .await;

    assert_eq!(failure, json!({ "_tag": "TooManyBlocks", "limit": 1000 }));
    let listed = board
        .ok(&board.human(), "issue.list", json!({ "tool": "linter" }))
        .await;
    assert_eq!(listed["items"], json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_implies_exactly_the_bound_of_blocks_is_imported_within_the_hang_guard() {
    let board = start().await;
    board.project("DEM").await;

    let answer = import(&board, dense_run(2, 500)).await;

    assert_eq!(answer["created"]["count"], 502, "{answer}");
}

mod in_process {
    use std::sync::Arc;

    use acid_cat_core::{Outcome, Session};
    use quadratic_server::app::{Context, application, board_loads_on_this_thread};
    use quadratic_server::domain::{Caller, WorkingCopy};
    use quadratic_server::store::{Author, Call, Store, StoreError};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::dense_run;
    use crate::support::within;

    const HUMAN: &str = r#"{"kind":"human","name":"sam"}"#;

    async fn board_loads_on_the_writer_thread(store: &Store) -> usize {
        let call = Call::new(Author::new(HUMAN).expect("an author"), "test.count");
        store
            .write(call, |_| Ok::<_, StoreError>(board_loads_on_this_thread()))
            .await
            .expect("the count is read")
            .value
    }

    #[tokio::test]
    async fn a_run_with_hundreds_of_blocks_loads_the_board_once() {
        let home = tempfile::tempdir().expect("a temporary home");
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
        let before = board_loads_on_the_writer_thread(&store).await;

        let imported = within(
            "findings.import",
            session.invoke(
                "findings.import",
                json!({ "project": "DEM", "tool": "linter", "findings": dense_run(10, 30) }),
                &cancel,
            ),
        )
        .await;

        assert!(matches!(imported, Outcome::Value(_)), "{imported:?}");
        assert_eq!(board_loads_on_the_writer_thread(&store).await - before, 1);
    }
}
