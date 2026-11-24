use acid_cat_core::CapabilityDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::board;
use super::catalog::capability;
use super::cursor::Listing;
use super::issues::as_of;
use super::model::Change;
use super::{Context, Refused, finish};
use crate::store::Subject;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QueryInput {
    issue: Option<String>,
    project: Option<String>,
    as_of: Option<String>,
    #[serde(default = "limit")]
    limit: usize,
    cursor: Option<String>,
}

fn limit() -> usize {
    50
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChangePage {
    items: Vec<Change>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

pub(super) fn query() -> CapabilityDefinition<Context, QueryInput, ChangePage, Value> {
    capability::<QueryInput, ChangePage>("history.query")
        .run(|input, call| async move { finish(query_history(call.context(), input).await) })
}

async fn query_history(context: &Context, input: QueryInput) -> Result<ChangePage, Refused> {
    let listing = Listing::of(&json!({
        "listing": "history.query",
        "issue": input.issue,
        "project": input.project,
        "asOf": input.as_of,
    }));
    let cursor = listing.position(input.cursor)?;
    context
        .read(as_of(input.as_of), move |view| {
            if let Some(key) = &input.project {
                board::project(view, key)?;
            }
            let subject = match (input.issue, input.project) {
                (Some(issue), project) => {
                    let record = board::issue(view, &issue)?;
                    if project.is_some_and(|project| project != record.project) {
                        return Ok(ChangePage {
                            items: Vec::new(),
                            next_cursor: None,
                        });
                    }
                    Subject::Issue(issue)
                }
                (None, Some(project)) => Subject::Project(project),
                (None, None) => Subject::Board,
            };
            let page = view.history(&subject, cursor.as_deref(), input.limit)?;
            Ok(ChangePage {
                items: page
                    .items
                    .into_iter()
                    .map(Change::of)
                    .collect::<Result<_, _>>()?,
                next_cursor: page.next_cursor.map(|position| listing.bind(position)),
            })
        })
        .await
}
