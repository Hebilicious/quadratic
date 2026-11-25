use std::collections::{BTreeMap, BTreeSet};

use super::failure::Failure;
use super::issue::IssueId;
use super::readiness::{Wait, WaitKind};
use super::stack::STACKED_ON;
use super::workflow::Category;

const WAITING_KIND: &str = "blocks";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelationChange {
    Parent { child: IssueId, parent: IssueId },
    Blocks { blocker: IssueId, blocked: IssueId },
    StackedOn { stacked: IssueId, base: IssueId },
}

impl RelationChange {
    pub fn link(source: &str, kind: &str, target: &str) -> Option<RelationChange> {
        match kind {
            WAITING_KIND => Some(RelationChange::Blocks {
                blocker: source.to_owned(),
                blocked: target.to_owned(),
            }),
            STACKED_ON => Some(RelationChange::StackedOn {
                stacked: source.to_owned(),
                base: target.to_owned(),
            }),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WaitGraph {
    categories: BTreeMap<IssueId, Option<Category>>,
    parents: BTreeMap<IssueId, IssueId>,
    children: BTreeMap<IssueId, BTreeSet<IssueId>>,
    blockers: BTreeMap<IssueId, BTreeSet<IssueId>>,
    bases: BTreeMap<IssueId, BTreeSet<IssueId>>,
}

impl WaitGraph {
    pub fn issue(&mut self, id: IssueId, category: Option<Category>) {
        self.categories.insert(id, category);
    }

    pub fn apply(&mut self, change: RelationChange) {
        match change {
            RelationChange::Parent { child, parent } => {
                if let Some(previous) = self.parents.insert(child.clone(), parent.clone())
                    && let Some(siblings) = self.children.get_mut(&previous)
                {
                    siblings.remove(&child);
                }
                self.children.entry(parent).or_default().insert(child);
            }
            RelationChange::Blocks { blocker, blocked } => {
                self.blockers.entry(blocked).or_default().insert(blocker);
            }
            RelationChange::StackedOn { stacked, base } => {
                self.bases.entry(stacked).or_default().insert(base);
            }
        }
    }

    pub fn bases(&self, id: &str) -> Vec<IssueId> {
        self.bases.get(id).into_iter().flatten().cloned().collect()
    }

    pub fn stacked_on(&self, base: &str) -> Vec<IssueId> {
        self.bases
            .iter()
            .filter(|(_, bases)| bases.contains(base))
            .map(|(stacked, _)| stacked.clone())
            .collect()
    }

    pub fn waits(&self, id: &str) -> Vec<Wait> {
        let mut waits: Vec<Wait> = Vec::new();
        for blocker in self.blockers.get(id).into_iter().flatten() {
            waits.push(self.wait(blocker, WaitKind::Blocker));
        }
        for child in self.children.get(id).into_iter().flatten() {
            waits.push(self.wait(child, WaitKind::Child));
        }
        let mut seen = BTreeSet::from([id]);
        let mut ancestor = self.parents.get(id);
        while let Some(current) = ancestor.filter(|current| seen.insert(current.as_str())) {
            for blocker in self.blockers.get(current).into_iter().flatten() {
                waits.push(self.wait(blocker, WaitKind::AncestorBlocker));
            }
            ancestor = self.parents.get(current);
        }
        waits
    }

    pub fn check(&self, change: &RelationChange) -> Result<(), Failure> {
        self.check_all(std::slice::from_ref(change))
    }

    /// Whether the graph after all the changes has a cycle. It answers for the final graph only.
    /// For changes that only add edges, such as the `Blocks` of an import, that answer covers
    /// every prefix too: when all of them close no cycle, no part of them does, so one search
    /// replaces a search per change. A `Parent` change can replace an edge, so a sequence with
    /// one can pass through a cycle and still end without one; check those one at a time.
    pub fn check_all(&self, changes: &[RelationChange]) -> Result<(), Failure> {
        let mut proposed = self.clone();
        for change in changes {
            proposed.apply(change.clone());
        }
        match proposed.find_cycle() {
            Some(path) => Err(Failure::Cycle { path }),
            None => Ok(()),
        }
    }

    fn wait(&self, on: &str, kind: WaitKind) -> Wait {
        Wait {
            on: on.to_owned(),
            kind,
            category: self.categories.get(on).copied().flatten(),
        }
    }

    /// The edges a cycle may not close: every wait, and every base. A stacked issue does not
    /// wait for its bases to be resolved, but its branch starts from theirs, so a loop through
    /// `stacked-on` and `blocks` together could never be worked through either.
    fn waited_on(&self, id: &str) -> BTreeSet<IssueId> {
        self.waits(id)
            .into_iter()
            .map(|wait| wait.on)
            .chain(self.bases(id))
            .collect()
    }

    fn find_cycle(&self) -> Option<Vec<IssueId>> {
        let issues: BTreeSet<&IssueId> = self
            .categories
            .keys()
            .chain(self.parents.keys())
            .chain(self.children.keys())
            .chain(self.blockers.keys())
            .chain(self.bases.keys())
            .collect();
        let mut done: BTreeSet<IssueId> = BTreeSet::new();
        for start in issues {
            let mut path: Vec<IssueId> = Vec::new();
            if let Some(cycle) = self.visit(start, &mut path, &mut done) {
                return Some(cycle);
            }
        }
        None
    }

    fn visit(
        &self,
        issue: &IssueId,
        path: &mut Vec<IssueId>,
        done: &mut BTreeSet<IssueId>,
    ) -> Option<Vec<IssueId>> {
        if let Some(position) = path.iter().position(|on_path| on_path == issue) {
            let mut cycle = path[position..].to_vec();
            cycle.push(issue.clone());
            return Some(cycle);
        }
        if done.contains(issue) {
            return None;
        }
        path.push(issue.clone());
        for next in self.waited_on(issue) {
            if let Some(cycle) = self.visit(&next, path, done) {
                return Some(cycle);
            }
        }
        path.pop();
        done.insert(issue.clone());
        None
    }
}
