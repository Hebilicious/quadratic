use std::cell::Cell;
use std::collections::BTreeMap;

use super::Refused;
use super::model::{self, IssueSummary};
use crate::domain::{
    self, Base, Caller, IssueId, RelationChange, Stage, Wait, WaitGraph, Workflow, readiness,
};
use crate::store::{self, IssueFilter, IssueRecord, Kind, View};

pub(crate) struct Board {
    graph: WaitGraph,
    bases: BTreeMap<IssueId, BaseState>,
}

struct BaseState {
    stage: Stage,
    branch: Option<String>,
}

thread_local! {
    static LOADS: Cell<usize> = const { Cell::new(0) };
}

#[doc(hidden)]
pub fn board_loads_on_this_thread() -> usize {
    LOADS.with(Cell::get)
}

impl Board {
    pub(crate) fn load(view: &View<'_>) -> Result<Board, Refused> {
        LOADS.with(|loads| loads.set(loads.get() + 1));
        let stored = view.graph()?;
        let workflows: BTreeMap<String, Workflow> = view
            .projects()?
            .iter()
            .map(|project| Ok((project.key.clone(), model::workflow(&project.workflow)?)))
            .collect::<Result<_, Refused>>()?;
        let mut graph = WaitGraph::default();
        let mut bases = BTreeMap::new();
        for node in stored.issues {
            if let Some(parent) = node.parent {
                graph.apply(RelationChange::Parent {
                    child: node.id.clone(),
                    parent,
                });
            }
            let category = node.category.as_deref().and_then(model::category);
            let stage = match workflows.get(&node.project) {
                Some(workflow) => Stage::of(workflow, &node.status, category),
                None => Stage::BeforeReview,
            };
            bases.insert(
                node.id.clone(),
                BaseState {
                    stage,
                    branch: node.claim_branch,
                },
            );
            graph.issue(node.id, category);
        }
        for link in stored.links {
            if let Some(change) = RelationChange::link(&link.source, &link.kind, &link.target) {
                graph.apply(change);
            }
        }
        Ok(Board { graph, bases })
    }

    pub(crate) fn waits(&self, id: &str) -> Vec<Wait> {
        self.graph.waits(id)
    }

    pub(crate) fn stacked_on(&self, base: &str) -> Vec<IssueId> {
        self.graph.stacked_on(base)
    }

    fn bases(&self, id: &str) -> Vec<Base> {
        self.graph
            .bases(id)
            .into_iter()
            .map(|on| {
                let (stage, branch) = match self.bases.get(&on) {
                    Some(state) => (state.stage, state.branch.clone()),
                    None => (Stage::BeforeReview, None),
                };
                Base { on, stage, branch }
            })
            .collect()
    }

    pub(crate) fn issue(&self, record: &IssueRecord) -> Result<domain::Issue, Refused> {
        Ok(domain::Issue {
            bases: self.bases(&record.id),
            ..model::domain_issue(record)?
        })
    }

    pub(crate) fn check(&self, change: &RelationChange) -> Result<(), Refused> {
        Ok(self.graph.check(change)?)
    }

    pub(crate) fn apply(&mut self, change: RelationChange) {
        self.graph.apply(change);
    }

    pub(crate) fn summary(
        &self,
        record: &IssueRecord,
        caller: &Caller,
    ) -> Result<IssueSummary, Refused> {
        let issue = self.issue(record)?;
        let readiness = readiness(&issue, &self.waits(&record.id), caller);
        Ok(IssueSummary::of(record, &issue, readiness))
    }
}

pub(crate) fn issue(view: &View<'_>, id: &str) -> Result<IssueRecord, Refused> {
    view.issue(id)?
        .ok_or_else(|| Refused::not_found(Kind::Issue, id))
}

pub(crate) fn require_issue(view: &View<'_>, id: &str) -> Result<(), Refused> {
    issue(view, id).map(|_| ())
}

pub(crate) fn project(view: &View<'_>, key: &str) -> Result<store::Project, Refused> {
    view.project(key)?
        .ok_or_else(|| Refused::not_found(Kind::Project, key))
}

pub(crate) fn caller_project(
    view: &View<'_>,
    given: Option<&str>,
    caller: &Caller,
) -> Result<store::Project, Refused> {
    if let Some(key) = given {
        return project(view, key);
    }
    let bound = match &caller.working_copy.repository {
        Some(repository) => view.project_by_repository(repository)?,
        None => None,
    };
    bound.ok_or_else(|| Refused::tagged("NoProject"))
}

pub(crate) fn summary(view: &View<'_>, id: &str, caller: &Caller) -> Result<IssueSummary, Refused> {
    Board::load(view)?.summary(&issue(view, id)?, caller)
}

pub(crate) fn all_issues(
    view: &View<'_>,
    filter: &IssueFilter,
) -> Result<Vec<IssueRecord>, Refused> {
    const PAGE: usize = 1000;
    let mut issues = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = view.issues(filter, cursor.as_deref(), PAGE)?;
        issues.extend(page.items);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(issues),
        }
    }
}
