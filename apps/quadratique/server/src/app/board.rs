use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use super::Refused;
use super::model::{self, IssueSummary};
use crate::domain::{
    self, Base, Caller, IssueId, RelationChange, STACKED_ON, Stage, Wait, WaitGraph, Workflow,
    readiness,
};
use crate::store::{self, IssueFilter, IssueRecord, Kind, View};

pub(crate) struct Board {
    graph: WaitGraph,
    stacked: BTreeMap<IssueId, Node>,
}

/// What the `stacked-on` rules read of an issue at either end of such a link. Only those
/// issues are read: a board without the link reads no project and no stage.
struct Node {
    project: String,
    stage: Stage,
    branch: Option<String>,
    repository: Option<String>,
    open: bool,
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
        let ends: BTreeSet<&str> = stored
            .links
            .iter()
            .filter(|link| link.kind == STACKED_ON)
            .flat_map(|link| [link.source.as_str(), link.target.as_str()])
            .collect();
        let projects: BTreeMap<String, (Workflow, Option<String>)> = if ends.is_empty() {
            BTreeMap::new()
        } else {
            view.projects()?
                .iter()
                .map(|project| {
                    let workflow = model::workflow(&project.workflow)?;
                    Ok((project.key.clone(), (workflow, project.repository.clone())))
                })
                .collect::<Result<_, Refused>>()?
        };
        let mut graph = WaitGraph::default();
        let mut stacked = BTreeMap::new();
        for node in &stored.issues {
            if let Some(parent) = &node.parent {
                graph.apply(RelationChange::Parent {
                    child: node.id.clone(),
                    parent: parent.clone(),
                });
            }
            let category = node.category.as_deref().and_then(model::category);
            if ends.contains(node.id.as_str()) {
                let (stage, repository) = match projects.get(&node.project) {
                    Some((workflow, repository)) => (
                        Stage::of(workflow, &node.status, category),
                        repository.clone(),
                    ),
                    None => (Stage::BeforeReview, None),
                };
                stacked.insert(
                    node.id.clone(),
                    Node {
                        project: node.project.clone(),
                        stage,
                        branch: node.claim_branch.clone(),
                        repository,
                        open: node.claim_branch.is_none()
                            && !category.is_some_and(domain::Category::is_resolved),
                    },
                );
            }
            graph.issue(node.id.clone(), category);
        }
        for link in &stored.links {
            if let Some(change) = RelationChange::link(&link.source, &link.kind, &link.target) {
                graph.apply(change);
            }
        }
        Ok(Board { graph, stacked })
    }

    /// The base branches that git must check against the caller's branch before deciding
    /// whether the caller can start one of `issues` (every issue when `None`): the bases in
    /// review, in the caller's repository, of unclaimed open issues stacked on them.
    pub(crate) fn branches_to_check(
        &self,
        issues: Option<&[IssueId]>,
        caller: &Caller,
    ) -> BTreeSet<String> {
        let mut branches = BTreeSet::new();
        for (id, node) in &self.stacked {
            let wanted = issues.is_none_or(|issues| issues.contains(id));
            if !wanted || !node.open {
                continue;
            }
            for base in self.bases(id) {
                if base.asks_git(caller)
                    && let Some(branch) = base.branch
                {
                    branches.insert(branch);
                }
            }
        }
        branches
    }

    /// The pairs (issue, base) with an end in `project` where the issue is at the `merge`
    /// status or later and the base is not merged.
    pub(crate) fn out_of_order(&self, project: &str) -> BTreeSet<(IssueId, IssueId)> {
        let in_project = |id: &str| {
            self.stacked
                .get(id)
                .is_some_and(|node| node.project == project)
        };
        let mut pairs = BTreeSet::new();
        for (id, node) in &self.stacked {
            if node.stage != Stage::Merged {
                continue;
            }
            for base in self.bases(id) {
                if base.stage != Stage::Merged && (in_project(id) || in_project(&base.on)) {
                    pairs.insert((id.clone(), base.on));
                }
            }
        }
        pairs
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
            .map(|on| match self.stacked.get(&on) {
                Some(node) => Base {
                    stage: node.stage,
                    branch: node.branch.clone(),
                    repository: node.repository.clone(),
                    on,
                },
                None => Base {
                    on,
                    stage: Stage::BeforeReview,
                    branch: None,
                    repository: None,
                },
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

    pub(crate) fn check_all(&self, changes: &[RelationChange]) -> Result<(), Refused> {
        Ok(self.graph.check_all(changes)?)
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
