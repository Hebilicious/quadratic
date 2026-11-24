use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::issue::{Assignee, IssueId};
use super::workflow::Category;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Finding {
    pub key: String,
    pub title: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Assignee>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<Link>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub decision: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub kind: String,
    pub id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Create,
    Update,
    Reopen,
    Finish,
    Leave,
}

pub fn step(issue: Option<Category>, reported: bool, complete: bool) -> Step {
    match (issue, reported) {
        (None, true) => Step::Create,
        (None, false) => Step::Leave,
        (Some(Category::Canceled), _) => Step::Leave,
        (Some(Category::Completed), true) => Step::Reopen,
        (Some(Category::Completed), false) => Step::Leave,
        (Some(_), true) => Step::Update,
        (Some(_), false) if complete => Step::Finish,
        (Some(_), false) => Step::Leave,
    }
}

pub struct Sourced {
    pub id: IssueId,
    pub decision: bool,
    pub open: bool,
    pub links: BTreeSet<Link>,
}

pub const BLOCKING_KIND: &str = "fact";

pub const MAX_BLOCKS_CYCLE_CHECKED_IN_ONE_WRITE: usize = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooManyBlocks;

pub fn decision_blocks(issues: &[Sourced]) -> Result<BTreeSet<(IssueId, IssueId)>, TooManyBlocks> {
    let facts = |issue: &Sourced| {
        issue
            .links
            .iter()
            .filter(|link| link.kind == BLOCKING_KIND)
            .map(|link| link.id.clone())
            .collect::<BTreeSet<String>>()
    };
    let mut touching: BTreeMap<String, Vec<&IssueId>> = BTreeMap::new();
    for issue in issues.iter().filter(|issue| !issue.decision && issue.open) {
        for fact in facts(issue) {
            touching.entry(fact).or_default().push(&issue.id);
        }
    }
    let mut blocks = BTreeSet::new();
    for decision in issues.iter().filter(|issue| issue.decision && issue.open) {
        for fact in facts(decision) {
            for blocked in touching.get(&fact).into_iter().flatten() {
                blocks.insert((decision.id.clone(), (*blocked).clone()));
                if blocks.len() > MAX_BLOCKS_CYCLE_CHECKED_IN_ONE_WRITE {
                    return Err(TooManyBlocks);
                }
            }
        }
    }
    Ok(blocks)
}
