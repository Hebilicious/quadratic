use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

use quadratic_server::domain::{Failure, RelationChange, WaitGraph};

use super::support::property;

fn parent(child: &str, parent: &str) -> RelationChange {
    RelationChange::Parent {
        child: child.into(),
        parent: parent.into(),
    }
}

fn blocks(blocker: &str, blocked: &str) -> RelationChange {
    RelationChange::Blocks {
        blocker: blocker.into(),
        blocked: blocked.into(),
    }
}

fn stacked_on(stacked: &str, base: &str) -> RelationChange {
    RelationChange::StackedOn {
        stacked: stacked.into(),
        base: base.into(),
    }
}

fn relations(changes: &[RelationChange]) -> WaitGraph {
    let mut graph = WaitGraph::default();
    for change in changes {
        graph.apply(change.clone());
    }
    graph
}

#[derive(Default)]
struct Relations {
    parents: BTreeMap<String, String>,
    blocks: BTreeSet<(String, String)>,
    stacks: BTreeSet<(String, String)>,
}

impl Relations {
    fn of(changes: &[RelationChange]) -> Relations {
        let mut relations = Relations::default();
        for change in changes.iter().cloned() {
            match change {
                RelationChange::Parent { child, parent } => {
                    relations.parents.insert(child, parent);
                }
                RelationChange::Blocks { blocker, blocked } => {
                    relations.blocks.insert((blocker, blocked));
                }
                RelationChange::StackedOn { stacked, base } => {
                    relations.stacks.insert((stacked, base));
                }
            }
        }
        relations
    }
}

fn cycle(path: &[&str]) -> Result<(), Failure> {
    Err(Failure::Cycle {
        path: path.iter().map(|id| id.to_string()).collect(),
    })
}

#[test]
fn a_parent_cannot_block_its_child() {
    let existing = relations(&[parent("B", "A")]);
    assert_eq!(existing.check(&blocks("A", "B")), cycle(&["A", "B", "A"]));
}

#[test]
fn an_issue_cannot_block_the_ancestor_of_its_own_blocker() {
    let existing = relations(&[parent("B", "A"), blocks("B", "C")]);
    assert_eq!(existing.check(&blocks("C", "A")), cycle(&["B", "C", "B"]));
}

#[test]
fn a_new_parent_that_closes_a_cycle_is_refused() {
    let existing = relations(&[parent("B", "A"), parent("C", "B")]);
    assert_eq!(
        existing.check(&parent("A", "C")),
        cycle(&["A", "B", "C", "A"])
    );
    assert_eq!(existing.check(&parent("A", "A")), cycle(&["A", "A"]));
    let blocked = relations(&[blocks("A", "B")]);
    assert_eq!(blocked.check(&parent("A", "B")), cycle(&["A", "A"]));
}

#[test]
fn changes_that_close_no_cycle_are_accepted() {
    let existing = relations(&[parent("B", "A"), blocks("B", "C")]);
    assert_eq!(existing.check(&blocks("A", "C")), Ok(()));
    assert_eq!(existing.check(&blocks("D", "A")), Ok(()));
    assert_eq!(existing.check(&parent("C", "D")), Ok(()));
    assert_eq!(existing.check(&blocks("B", "C")), Ok(()));
}

fn change() -> impl Strategy<Value = RelationChange> {
    let issue = prop::sample::select(vec!["A", "B", "C", "D", "E", "F"]);
    (0..3_u8, issue.clone(), issue).prop_map(|(kind, from, to)| match kind {
        0 => parent(from, to),
        1 => blocks(from, to),
        _ => stacked_on(from, to),
    })
}

fn waits_on(relations: &Relations, waiter: &str, target: &str) -> bool {
    let mut ancestors = vec![waiter.to_string()];
    let mut current = waiter.to_string();
    while let Some(next) = relations.parents.get(&current) {
        if ancestors.contains(next) {
            break;
        }
        ancestors.push(next.clone());
        current = next.clone();
    }
    let blocked_by_target = relations
        .blocks
        .iter()
        .any(|(blocker, blocked)| blocker == target && ancestors.contains(blocked));
    let child_is_target = relations.parents.get(target).map(String::as_str) == Some(waiter);
    let stacked_on_target = relations
        .stacks
        .contains(&(waiter.to_string(), target.to_string()));
    blocked_by_target || child_is_target || stacked_on_target
}

fn has_cycle(relations: &Relations) -> bool {
    let issues = ["A", "B", "C", "D", "E", "F"];
    issues.iter().any(|&start| {
        let mut seen = vec![start];
        let mut frontier = vec![start];
        while let Some(issue) = frontier.pop() {
            for &next in &issues {
                if waits_on(relations, issue, next) {
                    if next == start {
                        return true;
                    }
                    if !seen.contains(&next) {
                        seen.push(next);
                        frontier.push(next);
                    }
                }
            }
        }
        false
    })
}

#[test]
fn accepted_changes_never_leave_a_cycle() {
    property(2000, prop::collection::vec(change(), 1..16), |changes| {
        let mut accepted: Vec<RelationChange> = Vec::new();
        for change in changes {
            let current = relations(&accepted);
            match current.check(&change) {
                Ok(()) => {
                    accepted.push(change);
                    prop_assert!(!has_cycle(&Relations::of(&accepted)), "{:?}", accepted);
                }
                Err(Failure::Cycle { path }) => {
                    let mut proposed = accepted.clone();
                    proposed.push(change);
                    prop_assert!(
                        has_cycle(&Relations::of(&proposed)),
                        "refused without a cycle: {:?}",
                        path
                    );
                    prop_assert_eq!(path.first(), path.last());
                }
                Err(other) => prop_assert!(false, "unexpected failure {:?}", other),
            }
        }
        Ok(())
    });
}
