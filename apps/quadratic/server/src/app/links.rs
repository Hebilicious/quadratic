use acid_cat_core::CapabilityDefinition;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::board::{Board, require_issue};
use super::catalog::capability;
use super::lifecycle::release_held;
use super::model::{IssueSummary, Relation};
use super::{Context, Refused, finish};
use crate::domain::{RelationChange, STACKED_ON};

#[derive(Deserialize)]
pub(super) struct LinkInput {
    issue: String,
    kind: String,
    target: String,
    #[serde(default)]
    remove: bool,
}

#[derive(Serialize)]
pub(super) struct Relations {
    issue: IssueSummary,
    relations: Vec<Relation>,
}

pub(super) fn link() -> CapabilityDefinition<Context, LinkInput, Relations, Value> {
    capability::<LinkInput, Relations>("issue.link")
        .run(|input, call| async move { finish(link_issues(call.context(), input).await) })
}

async fn link_issues(context: &Context, input: LinkInput) -> Result<Relations, Refused> {
    if input.issue == input.target {
        return Err(Refused::tagged("SelfLink"));
    }
    let (relations, issue) = context
        .write_issue("issue.link", None, move |tx| {
            require_issue(tx, &input.issue)?;
            require_issue(tx, &input.target)?;
            if input.remove {
                tx.remove_relation(&input.issue, &input.kind, &input.target)?;
                if input.kind == STACKED_ON {
                    release_held(tx, vec![input.issue.clone()])?;
                }
            } else {
                if let Some(change) = RelationChange::link(&input.issue, &input.kind, &input.target)
                {
                    Board::load(tx)?.check(&change)?;
                }
                tx.add_relation(&input.issue, &input.kind, &input.target)?;
            }
            let relations: Vec<Relation> = tx
                .relations(&input.issue)?
                .iter()
                .map(Relation::of)
                .collect::<Result<_, _>>()?;
            Ok((relations, input.issue))
        })
        .await?;
    Ok(Relations { issue, relations })
}
