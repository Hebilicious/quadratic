//! The human's saved views of the web app. The web target's caller is always the human, so the
//! views of a data folder are the human's.

use std::sync::Arc;

use acid_cat_core::{
    CapabilityDefinition, Defect, Emitted, EventDefinition, NoInput, Subscription,
};
use futures::StreamExt;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast::{self, error::RecvError};

use super::catalog::{capability, capability_without_input};
use super::{Context, Refused, finish, model};
use crate::domain::Actor;
use crate::store::{AsOf, Commit, Kind, SavedViewRecord, Tx, View};

const IMPORTED: &str = "imported";
const MOST_NAME_CHARACTERS: usize = 100;
/// The catalog's maximum of saved views: every write and every `view.changed` carries all of them.
const MOST_VIEWS: usize = 100;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub(super) struct SavedView {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    filter: String,
}

impl From<SavedViewRecord> for SavedView {
    fn from(record: SavedViewRecord) -> SavedView {
        SavedView {
            id: record.id,
            name: record.name,
            project: record.project,
            filter: record.filter,
        }
    }
}

#[derive(Serialize)]
pub(super) struct SavedViews {
    revision: i64,
    items: Vec<SavedView>,
}

fn saved_views(view: &View<'_>) -> Result<SavedViews, Refused> {
    let list = view.saved_views()?;
    Ok(SavedViews {
        revision: list.revision,
        items: list.views.into_iter().map(SavedView::from).collect(),
    })
}

/// Names are the same when they are once trimmed and lowercased.
fn same_name(left: &str, right: &str) -> bool {
    left.trim().to_lowercase() == right.trim().to_lowercase()
}

fn name_taken(view: &SavedViewRecord) -> Refused {
    Refused::Failure(json!({ "_tag": "NameTaken", "view": view.id }))
}

fn too_many_views() -> Refused {
    Refused::Failure(json!({ "_tag": "TooManyViews", "most": MOST_VIEWS }))
}

pub(super) fn list() -> CapabilityDefinition<Context, NoInput, SavedViews, Value> {
    capability_without_input::<SavedViews>("view.list")
        .run(|(), call| async move { finish(call.context().read(AsOf::Now, saved_views).await) })
}

#[derive(Deserialize)]
pub(super) struct SaveInput {
    name: String,
    project: Option<String>,
    filter: String,
    replace: Option<String>,
}

pub(super) fn save() -> CapabilityDefinition<Context, SaveInput, SavedViews, Value> {
    capability::<SaveInput, SavedViews>("view.save").run(|input, call| async move {
        finish(written(call.context(), "view.save", move |tx| save_view(tx, input)).await)
    })
}

/// Creates a view, or replaces the filters of the view the page named: never a view the page did
/// not mean, since both are checked against the list in this transaction.
fn save_view(tx: &mut Tx<'_>, input: SaveInput) -> Result<(), Refused> {
    let mut views = tx.saved_views()?.views;
    let name = input.name.trim().to_owned();
    match input.replace {
        Some(id) => {
            let Some(replaced) = views.iter_mut().find(|view| view.id == id) else {
                return Err(Refused::not_found(Kind::View, &id));
            };
            if !same_name(&replaced.name, &name) {
                return Err(Refused::Failure(
                    json!({ "_tag": "Renamed", "view": id, "name": replaced.name }),
                ));
            }
            replaced.project = input.project;
            replaced.filter = input.filter;
        }
        None => {
            if let Some(named) = views.iter().find(|view| same_name(&view.name, &name)) {
                return Err(name_taken(named));
            }
            if views.len() >= MOST_VIEWS {
                return Err(too_many_views());
            }
            views.push(SavedViewRecord {
                id: tx.new_saved_view_id()?,
                name,
                project: input.project,
                filter: input.filter,
            });
        }
    }
    Ok(tx.replace_saved_views(&views)?)
}

#[derive(Deserialize)]
pub(super) struct RenameInput {
    view: String,
    name: String,
}

pub(super) fn rename() -> CapabilityDefinition<Context, RenameInput, SavedViews, Value> {
    capability::<RenameInput, SavedViews>("view.rename").run(|input, call| async move {
        finish(
            written(call.context(), "view.rename", move |tx| {
                rename_view(tx, input)
            })
            .await,
        )
    })
}

fn rename_view(tx: &mut Tx<'_>, input: RenameInput) -> Result<(), Refused> {
    let mut views = tx.saved_views()?.views;
    if !views.iter().any(|view| view.id == input.view) {
        return Err(Refused::not_found(Kind::View, &input.view));
    }
    let name = input.name.trim();
    if let Some(taken) = views
        .iter()
        .find(|view| view.id != input.view && same_name(&view.name, name))
    {
        return Err(name_taken(taken));
    }
    for view in views.iter_mut().filter(|view| view.id == input.view) {
        name.clone_into(&mut view.name);
    }
    Ok(tx.replace_saved_views(&views)?)
}

#[derive(Deserialize)]
pub(super) struct DeleteInput {
    view: String,
}

pub(super) fn delete() -> CapabilityDefinition<Context, DeleteInput, SavedViews, Value> {
    capability::<DeleteInput, SavedViews>("view.delete").run(|input, call| async move {
        finish(
            written(call.context(), "view.delete", move |tx| {
                let mut views = tx.saved_views()?.views;
                views.retain(|view| view.id != input.view);
                Ok(tx.replace_saved_views(&views)?)
            })
            .await,
        )
    })
}

#[derive(Deserialize)]
pub(super) struct ImportInput {
    source: String,
    views: Vec<OldView>,
}

#[derive(Deserialize)]
pub(super) struct OldView {
    old: String,
    name: String,
    project: Option<String>,
    filter: String,
}

#[derive(Serialize)]
pub(super) struct Imported {
    revision: i64,
    items: Vec<SavedView>,
    full: Vec<String>,
}

pub(super) fn import() -> CapabilityDefinition<Context, ImportInput, Imported, Value> {
    capability::<ImportInput, Imported>("views.import").run(|input, call| async move {
        let imported = call
            .context()
            .write("views.import", None, move |tx| {
                let full = import_views(tx, input)?;
                let SavedViews { revision, items } = saved_views(tx)?;
                Ok(Imported {
                    revision,
                    items,
                    full,
                })
            })
            .await
            .map(|written| written.value);
        finish(imported)
    })
}

/// Imports each old view that has no receipt of an earlier import, and answers the old ids that
/// did not fit under the maximum.
fn import_views(tx: &mut Tx<'_>, input: ImportInput) -> Result<Vec<String>, Refused> {
    let mut views = tx.saved_views()?.views;
    let mut full = Vec::new();
    for old in input.views {
        if tx.saved_view_import(&input.source, &old.old)?.is_some() {
            continue;
        }
        if views.len() >= MOST_VIEWS {
            full.push(old.old);
            continue;
        }
        let name = old.name.trim();
        let taken = |candidate: &str| views.iter().any(|view| same_name(&view.name, candidate));
        let Some(name) = import_names(name, views.len()).find(|candidate| !taken(candidate)) else {
            return Err(Refused::defect(
                "every name an imported view may take is taken",
            ));
        };
        let id = tx.new_saved_view_id()?;
        tx.record_saved_view_import(&input.source, &old.old, &id)?;
        views.push(SavedViewRecord {
            id,
            name,
            project: old.project,
            filter: old.filter,
        });
    }
    tx.replace_saved_views(&views)?;
    Ok(full)
}

/// The names an imported view may take, in order: `name`, `<name> (imported)`, `<name> (imported
/// 2)` and so on, cut on a character boundary to fit the longest name. With `saved` views, one of
/// the first `saved + 1` is free.
fn import_names(name: &str, saved: usize) -> impl Iterator<Item = String> + '_ {
    std::iter::once(name.to_owned()).chain((1..=saved).map(move |number| {
        let suffix = match number {
            1 => format!(" ({IMPORTED})"),
            _ => format!(" ({IMPORTED} {number})"),
        };
        let room = MOST_NAME_CHARACTERS - suffix.chars().count();
        let base: String = name.chars().take(room).collect();
        format!("{}{suffix}", base.trim_end())
    }))
}

async fn written<F>(context: &Context, operation: &str, work: F) -> Result<SavedViews, Refused>
where
    F: for<'c> FnOnce(&mut Tx<'c>) -> Result<(), Refused> + Send + 'static,
{
    context
        .write(operation, None, move |tx| {
            work(tx)?;
            saved_views(tx)
        })
        .await
        .map(|written| written.value)
}

/// The operations whose commits change the saved views.
fn changes_views(operation: &str) -> bool {
    operation.starts_with("view.") || operation == "views.import"
}

#[derive(Deserialize)]
pub(super) struct ChangedInput {}

#[derive(Serialize)]
pub(super) struct ChangedPayload {
    commit: String,
    at: String,
    author: Actor,
    operation: String,
    revision: i64,
    items: Vec<SavedView>,
}

pub(super) fn event() -> EventDefinition<Context, ChangedInput, ChangedPayload> {
    super::catalog::event::<ChangedInput, ChangedPayload>("view.changed").subscribe(follow)
}

fn follow(
    _: ChangedInput,
    subscription: Subscription<Context>,
) -> BoxStream<'static, Result<Emitted<ChangedPayload>, Defect>> {
    let context = subscription.context_arc();
    let commits = match context.store() {
        Ok(store) => store.subscribe(),
        Err(refused) => {
            return futures::stream::once(async move { Err(Defect::new(format!("{refused:?}"))) })
                .boxed();
        }
    };
    futures::stream::unfold((context, commits), |(context, mut commits)| async move {
        let commit = next_view_commit(&mut commits).await?;
        let emitted = occurrence(&context, commit)
            .await
            .map_err(|refused| Defect::new(format!("{refused:?}")));
        Some((emitted, (context, commits)))
    })
    .boxed()
}

/// The next commit that changed the saved views. `None` ends the stream when the server closes,
/// and when commits were skipped: the page then subscribes again and reads `view.list`.
async fn next_view_commit(commits: &mut broadcast::Receiver<Commit>) -> Option<Commit> {
    loop {
        match commits.recv().await {
            Ok(commit) if changes_views(&commit.operation) => return Some(commit),
            Ok(_) => {}
            Err(RecvError::Lagged(_) | RecvError::Closed) => return None,
        }
    }
}

async fn occurrence(
    context: &Arc<Context>,
    commit: Commit,
) -> Result<Emitted<ChangedPayload>, Refused> {
    let views = context
        .read(AsOf::Commit(commit.hash.clone()), saved_views)
        .await?;
    let payload = ChangedPayload {
        author: model::actor(&commit.author)?,
        commit: commit.hash.clone(),
        at: commit.at.clone(),
        operation: commit.operation,
        revision: views.revision,
        items: views.items,
    };
    Ok(Emitted::new(payload)
        .event_id(&commit.hash)
        .timestamp(&commit.at))
}

#[cfg(test)]
mod tests {
    use super::{import_names, same_name};

    #[test]
    fn an_imported_view_whose_name_is_taken_comes_in_as_imported_then_numbered() {
        let names: Vec<String> = import_names("Web", 2).collect();
        assert_eq!(names, ["Web", "Web (imported)", "Web (imported 2)"]);
        assert!(same_name(" Web ", "wEB"));
        let long = "n".repeat(100);
        assert!(import_names(&long, 12).all(|name| name.chars().count() <= 100));
    }
}
