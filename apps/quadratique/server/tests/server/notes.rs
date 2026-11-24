use acid_cat_core::Outcome;
use serde_json::{Value, json};

use crate::support::{Running, Who, start};

async fn add_note(board: &Running, who: &Who, issue: &str, text: &str) -> Value {
    let revision = board.get(issue).await["revision"].clone();
    board
        .ok(
            who,
            "issue.update",
            json!({ "issue": issue, "revision": revision, "notes": text }),
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn notes_from_two_actors_are_both_kept_in_order_with_their_author_and_time() {
    let board = start().await;
    board.project("DEM").await;
    let issue = board.issue("Review the parser").await;
    let reviewer = board.agent("reviewer");

    add_note(
        &board,
        &reviewer,
        &issue,
        "Sent back: the parser drops the last line.",
    )
    .await;
    let first = board.get(&issue).await["notes"][0].clone();
    add_note(
        &board,
        &board.human(),
        &issue,
        "Correction: only on Windows line ends.",
    )
    .await;

    let notes = board.get(&issue).await["notes"].clone();
    assert_eq!(notes.as_array().map(Vec::len), Some(2), "{notes}");
    assert_eq!(notes[0], first);
    assert_eq!(
        notes[0]["text"],
        "Sent back: the parser drops the last line."
    );
    assert_eq!(notes[0]["author"], reviewer.name());
    assert_eq!(notes[1]["text"], "Correction: only on Windows line ends.");
    assert_eq!(notes[1]["author"], "sam");
    let at = |note: &Value| note["at"].as_str().expect("a time").to_owned();
    assert!(at(&notes[0]) < at(&notes[1]), "{notes}");
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_cannot_name_the_notes() {
    let board = start().await;
    board.project("DEM").await;
    let issue = board
        .issue_with(json!({ "acceptance": "It is done.", "title": "Keep the notes", "notes": "First note." }))
        .await;
    let revision = board.get(&issue).await["revision"].clone();

    let answer = board
        .client(&board.human())
        .invoke(
            "issue.update",
            &json!({ "issue": issue, "revision": revision, "clear": ["notes"] }),
        )
        .await;

    assert!(
        matches!(&answer, Outcome::CallFailure(failure) if failure.kind.as_str() == "invalid-input"),
        "{answer:?}"
    );
    let notes = board.get(&issue).await["notes"].clone();
    assert_eq!(notes[0]["text"], "First note.");
    assert_eq!(notes.as_array().map(Vec::len), Some(1), "{notes}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_history_shows_a_second_note_as_one_added_note() {
    let board = start().await;
    board.project("DEM").await;
    let issue = board
        .issue_with(
            json!({ "acceptance": "It is done.", "title": "Two notes", "notes": "First note." }),
        )
        .await;
    add_note(&board, &board.agent("writer"), &issue, "Second note.").await;

    let history = board
        .ok(
            &board.human(),
            "history.query",
            json!({ "issue": issue, "limit": 1 }),
        )
        .await;

    let fields = &history["items"][0]["fields"];
    let notes: Vec<&Value> = fields
        .as_array()
        .expect("field changes")
        .iter()
        .filter(|field| field["field"] == "notes")
        .collect();
    assert_eq!(
        notes,
        [&json!({ "field": "notes", "after": "\"Second note.\"" })],
        "{fields}"
    );
}
