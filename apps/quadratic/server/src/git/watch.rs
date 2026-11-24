use std::collections::BTreeSet;

use crate::store::{ClaimedWork, GitWatch, GitWork};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub repository: String,
    pub default_branch: String,
    pub heads: Vec<Head>,
    pub local: Reading,
    pub upstream: Option<Reading>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub place: Place,
    pub outside: BTreeSet<String>,
    pub reached: Reached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub name: String,
    pub tip: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub branch: String,
    pub tip: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reached {
    Unknown,
    Since(Vec<Commit>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub branch: String,
    pub tip: String,
    pub work: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub place: Place,
    pub branches: Vec<Candidate>,
    pub trailers: Vec<Commit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Landed {
    Branch {
        branch: String,
        tip: String,
        claims: Vec<ClaimedWork>,
        place: Place,
    },
    Trailer {
        commit: String,
        refs: Vec<String>,
        place: Place,
    },
}

pub fn continues(known: &GitWatch, repository: &str, default_branch: &str) -> bool {
    known.repository == repository && known.default_branch == default_branch
}

pub fn step(known: Option<&GitWatch>, seen: &Seen) -> (Vec<Observed>, GitWatch) {
    let known = known.filter(|known| continues(known, &seen.repository, &seen.default_branch));
    let readings: Vec<&Reading> = std::iter::once(&seen.local).chain(&seen.upstream).collect();
    let mut observed = Vec::new();
    if let Some(known) = known {
        let mut taken: BTreeSet<&str> = BTreeSet::new();
        for reading in &readings {
            let Reached::Since(commits) = &reading.reached else {
                continue;
            };
            let branches = known
                .work
                .iter()
                .filter(|work| !reading.outside.contains(&work.branch))
                .filter(|work| taken.insert(work.branch.as_str()))
                .filter_map(|work| {
                    let head = seen.heads.iter().find(|head| head.branch == work.branch)?;
                    Some(Candidate {
                        branch: work.branch.clone(),
                        tip: head.tip.clone(),
                        work: work.tip.clone(),
                    })
                })
                .collect();
            observed.push(Observed {
                place: reading.place.clone(),
                branches,
                trailers: commits
                    .iter()
                    .filter(|commit| !commit.refs.is_empty())
                    .cloned()
                    .collect(),
            });
        }
    }
    let work = seen
        .heads
        .iter()
        .filter(|head| head.branch != seen.default_branch)
        .filter(|head| {
            readings
                .iter()
                .all(|reading| reading.outside.contains(&head.branch))
        })
        .map(|head| GitWork {
            branch: head.branch.clone(),
            tip: head.tip.clone(),
        })
        .collect();
    let next = GitWatch {
        repository: seen.repository.clone(),
        default_branch: seen.default_branch.clone(),
        tip: seen.local.place.tip.clone(),
        upstream: seen
            .upstream
            .as_ref()
            .map(|reading| reading.place.tip.clone()),
        work,
    };
    (observed, next)
}
