use acid_cat_core::http::HttpClient;
use acid_cat_core::{CallFailure, Occurrence};
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};

use quadratique_server::web::WEB_BASE;

use crate::support::{HANG_GUARD, Running, call, start, within};

/// The web app's client: the `web` target, from the server's own page.
async fn page(board: &Running) -> HttpClient {
    board.page(WEB_BASE).await
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

fn items(views: &Value) -> Value {
    views["items"].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn views_are_created_replaced_renamed_and_deleted_in_their_order_with_a_growing_revision() {
    let board = start().await;
    let page = page(&board).await;
    assert_eq!(
        ok(&page, "view.list", json!({})).await,
        json!({ "revision": 0, "items": [] })
    );

    let first = ok(
        &page,
        "view.save",
        json!({ "name": " Ready web ", "project": "DEM", "filter": "label=web&ready=true" }),
    )
    .await;
    assert_eq!(first["revision"], 1);
    ok(
        &page,
        "view.save",
        json!({ "name": "Mine", "filter": "claim=mine" }),
    )
    .await;
    let replaced = ok(
        &page,
        "view.save",
        json!({ "name": "READY WEB", "filter": "ready=true", "replace": "v1" }),
    )
    .await;
    assert_eq!(
        replaced,
        json!({ "revision": 3, "items": [
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
    assert_eq!(
        again, deleted,
        "a delete that changes nothing keeps the revision"
    );
    let gone = call(
        &page,
        "view.rename",
        json!({ "view": "v1", "name": "Mine, all" }),
    )
    .await;
    assert_eq!(
        gone,
        Err(json!({ "_tag": "NotFound", "kind": "view", "id": "v1" })),
        "a deleted id is NotFound even when the name is taken"
    );

    let saved = ok(&page, "view.save", json!({ "name": "New", "filter": "" })).await;
    assert_eq!(
        saved["items"][1]["id"], "v3",
        "a deleted view's id is not given again"
    );
    assert_eq!(ok(&page, "view.list", json!({})).await, saved);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_save_from_an_older_list_never_changes_a_view_it_did_not_mean() {
    let board = start().await;
    let first = page(&board).await;
    let second = page(&board).await;
    ok(
        &second,
        "view.save",
        json!({ "name": "Mine", "filter": "claim=mine" }),
    )
    .await;

    let created = call(
        &first,
        "view.save",
        json!({ "name": "mine", "filter": "ready=true" }),
    )
    .await;
    assert_eq!(created, Err(json!({ "_tag": "NameTaken", "view": "v1" })));

    ok(
        &second,
        "view.rename",
        json!({ "view": "v1", "name": "Others" }),
    )
    .await;
    let replaced = call(
        &first,
        "view.save",
        json!({ "name": "Mine", "filter": "ready=true", "replace": "v1" }),
    )
    .await;
    assert_eq!(
        replaced,
        Err(json!({ "_tag": "Renamed", "view": "v1", "name": "Others" }))
    );
    let gone = call(
        &first,
        "view.save",
        json!({ "name": "Mine", "filter": "ready=true", "replace": "v9" }),
    )
    .await;
    assert_eq!(
        gone,
        Err(json!({ "_tag": "NotFound", "kind": "view", "id": "v9" }))
    );
    assert_eq!(
        items(&ok(&first, "view.list", json!({})).await),
        json!([{ "id": "v1", "name": "Others", "filter": "claim=mine" }])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hundred_views_is_the_most() {
    let board = start().await;
    let page = page(&board).await;
    let views: Vec<Value> = (0..100)
        .map(|number| json!({ "old": format!("o{number}"), "name": format!("View {number}"), "filter": "" }))
        .collect();
    let imported = ok(
        &page,
        "views.import",
        json!({ "source": "browser-one", "views": views }),
    )
    .await;
    assert_eq!(imported["items"].as_array().map(Vec::len), Some(100));
    assert_eq!(imported["full"], json!([]));

    let refused = call(
        &page,
        "view.save",
        json!({ "name": "One more", "filter": "" }),
    )
    .await;
    assert_eq!(refused, Err(json!({ "_tag": "TooManyViews", "most": 100 })));
    let full = ok(
        &page,
        "views.import",
        json!({ "source": "browser-two", "views": [{ "old": "x", "name": "Late", "filter": "" }] }),
    )
    .await;
    assert_eq!(full["full"], json!(["x"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_import_keeps_a_receipt_per_old_view_so_two_old_views_never_become_one() {
    let board = start().await;
    let page = page(&board).await;
    ok(
        &page,
        "view.save",
        json!({ "name": "Mine", "filter": "ready=false" }),
    )
    .await;
    let browser = json!({ "source": "browser-one", "views": [
        { "old": "v1", "name": "Mine", "filter": "ready=true" },
        { "old": "v2", "name": "Mine (imported)", "filter": "ready=true" },
    ] });

    let imported = ok(&page, "views.import", browser.clone()).await;
    let again = ok(&page, "views.import", browser).await;
    let other_browser = ok(
        &page,
        "views.import",
        json!({ "source": "browser-two", "views": [{ "old": "v1", "name": "Mine", "filter": "ready=true" }] }),
    )
    .await;

    assert_eq!(
        names(&imported),
        ["Mine", "Mine (imported)", "Mine (imported) (imported)"]
    );
    assert_eq!(imported["full"], json!([]));
    assert_eq!(
        again["items"], imported["items"],
        "a repeated import adds nothing"
    );
    assert_eq!(again["revision"], imported["revision"]);
    assert_eq!(
        names(&other_browser).last().copied(),
        Some("Mine (imported 2)"),
        "another browser's old view is another view"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_change_reaches_every_page_with_every_view_and_no_issue_change_is_sent() {
    let board = start().await;
    board.project("DEM").await;
    let first = page(&board).await;
    let other = page(&board).await;
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
    assert_eq!(saved.data["revision"], 1);
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
