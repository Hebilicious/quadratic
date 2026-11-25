use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::actor::{Actor, ActorKind};
use super::stack::Base;
use super::workflow::{Category, Status, Workflow};

pub type IssueId = String;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Assignee {
    Human,
    Agent { role: String },
}

impl Assignee {
    pub fn is(&self, actor: &Actor) -> bool {
        match self {
            Assignee::Human => actor.kind == ActorKind::Human,
            Assignee::Agent { role } => {
                actor.kind == ActorKind::Agent && actor.role.as_ref() == Some(role)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub holder: Actor,
    pub branch: String,
    pub worktree: String,
    pub since: String,
}

impl Claim {
    pub fn holder_key(&self) -> HolderKey {
        HolderKey::of(&self.holder.name, &self.worktree)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderKey(String);

impl HolderKey {
    fn of(name: &str, worktree: &str) -> HolderKey {
        HolderKey(serde_json::json!([name, worktree]).to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkingCopy {
    pub repository: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    /// The base branches that git verified `branch` contains, in `repository`; `None` when no
    /// decision asked git.
    pub contains: Option<BTreeSet<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub actor: Actor,
    pub working_copy: WorkingCopy,
}

impl Caller {
    pub fn holder_key(&self) -> HolderKey {
        HolderKey::of(
            &self.actor.name,
            self.working_copy.worktree.as_deref().unwrap_or_default(),
        )
    }

    /// The holder is the same actor name in the same worktree, or, from any worktree, an actor
    /// with the holder's harness and session: the match of `issue.list --claim mine`.
    pub fn holds(&self, claim: &Claim) -> bool {
        self.holder_key() == claim.holder_key() || self.in_session_of(&claim.holder)
    }

    fn in_session_of(&self, holder: &Actor) -> bool {
        self.actor
            .harness_session()
            .is_some_and(|session| holder.harness_session() == Some(session))
    }

    pub fn is_human(&self) -> bool {
        self.actor.kind == ActorKind::Human
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub id: IssueId,
    pub status: Status,
    pub category: Category,
    pub revision: u64,
    pub claim: Option<Claim>,
    pub assignee: Option<Assignee>,
    /// The issues this one is stacked on.
    pub bases: Vec<Base>,
}

impl Issue {
    pub fn assigned_to_human(&self) -> bool {
        self.assignee == Some(Assignee::Human)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub workflow: Workflow,
    pub default_branch: String,
    pub repository: Option<String>,
}
