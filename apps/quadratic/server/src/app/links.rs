use acid_cat_core::CapabilityDefinition;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::board::{self, Board, require_issue};
use super::catalog::capability;
use super::lifecycle::release_held;
use super::model::{IssueSummary, Relation};
use super::{Context, Refused, finish, model};
use crate::domain::{RelationChange, STACKED_ON, Stage, may_stack};
use crate::store::{Direction, View};

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
            } else if !link_exists(tx, &input.issue, &input.kind, &input.target)? {
                if let Some(change) = RelationChange::link(&input.issue, &input.kind, &input.target)
                {
                    let board = Board::load(tx)?;
                    board.check(&change)?;
                    if input.kind == STACKED_ON {
                        may_stack(
                            &input.issue,
                            stage_of(tx, &input.issue)?,
                            &input.target,
                            stage_of(tx, &input.target)?,
                        )?;
                    }
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

fn stage_of(view: &View<'_>, id: &str) -> Result<Stage, Refused> {
    let record = board::issue(view, id)?;
    let project = board::project(view, &record.project)?;
    let workflow = model::workflow(&project.workflow)?;
    let category = record.category.as_deref().and_then(model::category);
    Ok(Stage::of(&workflow, &record.status, category))
}

fn link_exists(view: &View<'_>, issue: &str, kind: &str, target: &str) -> Result<bool, Refused> {
    Ok(view.relations(issue)?.iter().any(|relation| {
        relation.kind == kind
            && relation.issue == target
            && matches!(relation.direction, Direction::Outgoing)
    }))
}
