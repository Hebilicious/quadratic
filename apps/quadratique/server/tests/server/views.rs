use acid_cat_core::http::HttpClient;
use acid_cat_core::{CallFailure, Occurrence};
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};

use crate::support::{HANG_GUARD, Running, call, start, within};

/// The web app's client: the `web` target, from the server's own page.
fn page(board: &Running) -> HttpClient {
    let url = board.server.url().replace("/cli", "/web");
    let origin = format!("http://{}", board.server.address());
    HttpClient::new(&url)
        .expect("the web target's URL")
        .header("Origin", &origin)
        .expect("a valid header")
}

async fn ok(page: &HttpClient, id: &str, input: Value) -> Value {
    match call(page, id, input.clone()).await {
        Ok(value) => value,
        Err(failure) => panic!("{id} {input} failed: {failure}"),
    }
}

fn names(views: &Value) -> Vec<&str> {
    views["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|view| view["name"].as_str().expect("a name"))
        .collect()
}

type Occurrences = BoxStream<'static, Result<Occurrence, CallFailure>>;

async fn next(occurrences: &mut Occurrences) -> Occurrence {
    match tokio::time::timeout(HANG_GUARD, occurrences.next()).await {
        Ok(Some(Ok(occurrence))) => occurrence,
        Ok(other) => panic!("the subscription ended: {other:?}"),
        Err(_) => panic!("no occurrence came within the hang guard of {HANG_GUARD:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn views_are_saved_replaced_renamed_and_deleted_in_their_order() {
    let board = start().await;
    let page = page(&board);
    assert_eq!(
        ok(&page, "view.list", json!({})).await,
        json!({ "items": [] })
    );

    ok(
        &page,
        "view.save",
        json!({ "name": " Ready web ", "project": "DEM", "filter": "label=web&ready=true" }),
    )
    .await;
    ok(
        &page,
        "view.save",
        json!({ "name": "Mine", "filter": "claim=mine" }),
    )
    .await;
    let replaced = ok(
        &page,
        "view.save",
        json!({ "name": "READY WEB", "filter": "ready=true" }),
    )
    .await;
    assert_eq!(
        replaced,
        json!({ "items": [
            { "id": "v1", "name": "Ready web", "filter": "ready=true" },
            { "id": "v2", "name": "Mine", "filter": "claim=mine" },
        ] })
    );

    let taken = call(
        &page,
        "view.rename",
        json!({ "view": "v2", "name": " ready WEB" }),
    )
    .await;
    assert_eq!(taken, Err(json!({ "_tag": "NameTaken", "view": "v1" })));
    let renamed = ok(
        &page,
        "view.rename",
        json!({ "view": "v2", "name": "Mine, all" }),
    )
    .await;
    assert_eq!(names(&renamed), ["Ready web", "Mine, all"]);
    let deleted = ok(&page, "view.delete", json!({ "view": "v1" })).await;
    assert_eq!(names(&deleted), ["Mine, all"]);
    let again = ok(&page, "view.delete", json!({ "view": "v1" })).await;
    assert_eq!(again, deleted);
    let gone = call(
        &page,
        "view.rename",
        json!({ "view": "v1", "name": "Back" }),
    )
    .await;
    assert_eq!(
        gone,
        Err(json!({ "_tag": "NotFound", "kind": "view", "id": "v1" }))
    );

    let saved = ok(&page, "view.save", json!({ "name": "New", "filter": "" })).await;
    assert_eq!(
        saved["items"][1]["id"], "v3",
        "a deleted view's id is not given again"
    );
    assert_eq!(ok(&page, "view.list", json!({})).await, saved);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_import_skips_the_views_already_there_and_renames_the_others_whose_name_is_taken() {
    let board = start().await;
    let page = page(&board);
    ok(
        &page,
        "view.save",
        json!({ "name": "Ready web", "project": "DEM", "filter": "ready=true" }),
    )
    .await;
    ok(
        &page,
        "view.save",
        json!({ "name": "Mine", "filter": "claim=mine" }),
    )
    .await;
    let browser = json!({ "views": [
        { "name": "Ready web", "project": "DEM", "filter": "ready=true" },
        { "name": "mine", "filter": "claim=any" },
        { "name": "Docs", "filter": "label=docs" },
    ] });

    let imported = ok(&page, "views.import", browser.clone()).await;
    let again = ok(&page, "views.import", browser).await;

    assert_eq!(
        names(&imported),
        ["Ready web", "Mine", "mine (imported)", "Docs"]
    );
    assert_eq!(imported["items"][2]["filter"], "claim=any");
    assert_eq!(again, imported, "a repeated import adds nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_change_reaches_every_page_with_every_view_and_no_issue_change_is_sent() {
    let board = start().await;
    board.project("DEM").await;
    let first = page(&board);
    let other = page(&board);
    let mut views = within(
        "the view subscription",
        other.subscribe("view.changed", &json!({}), None, None),
    )
    .await
    .expect("the subscription starts");
    let mut issues = within(
        "the issue subscription",
        other.subscribe("issue.changed", &json!({}), None, None),
    )
    .await
    .expect("the subscription starts");

    ok(
        &first,
        "view.save",
        json!({ "name": "Mine", "filter": "claim=mine" }),
    )
    .await;
    board.issue("After the view").await;

    let saved = next(&mut views).await;
    assert_eq!(saved.data["operation"], "view.save");
    assert_eq!(saved.data["author"]["kind"], "human");
    assert_eq!(
        saved.data["items"],
        json!([{ "id": "v1", "name": "Mine", "filter": "claim=mine" }])
    );
    let created = next(&mut issues).await;
    assert_eq!(
        created.data["operation"], "issue.create",
        "the view's commit is no issue change"
    );

    ok(&first, "view.delete", json!({ "view": "v1" })).await;
    let deleted = next(&mut views).await;
    assert_eq!(deleted.data["items"], json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn no_agent_and_no_command_reaches_the_saved_views() {
    let board = start().await;
    for who in [board.human(), board.agent("fix")] {
        let outcome = within(
            "view.list on the command line's target",
            board.client(&who).invoke("view.list", &json!({})),
        )
        .await;
        assert!(
            !matches!(outcome, acid_cat_core::Outcome::Value(_)),
            "{outcome:?}"
        );
    }
}
