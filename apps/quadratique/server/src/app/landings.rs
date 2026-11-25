use std::collections::BTreeMap;

use super::board::Board;
use super::lifecycle::apply;
use super::{Context, Refused, board, model};
use crate::domain::{Actor, Caller, Event, Failure, Landing, Outcome, WorkingCopy, decide};
use crate::git::ids;
use crate::git::watch::Landed;
use crate::store::{AsOf, ClaimedWork, GitLanding, GitWatch, Store, View};

const OPERATION: &str = "git.merged";
const MOST_REASON: usize = 500;

#[derive(Debug, Clone)]
pub(crate) struct Watched {
    pub key: String,
    pub repository: String,
    pub default_branch: String,
    pub known: Option<GitWatch>,
    pub claims: Vec<ClaimedWork>,
}

#[derive(Debug, Clone)]
struct IssueLanding {
    git_commit: String,
    landed_in: String,
    branch: Option<String>,
    signal: Signal,
    said: String,
}

#[derive(Debug, Clone)]
enum Signal {
    Branch {
        branch: String,
        since_claim: Option<ClaimedWork>,
    },
    Trailer,
}

pub(crate) async fn watched(store: &Store) -> Result<Vec<Watched>, Refused> {
    server(store, None)
        .read(AsOf::Now, |view| {
            view.projects()?
                .into_iter()
                .filter_map(|project| Some((project.repository.clone()?, project)))
                .map(|(repository, project)| {
                    Ok(Watched {
                        known: view.git_watch(&project.key)?,
                        claims: view.claimed_work(&project.key, &repository)?,
                        key: project.key,
                        repository,
                        default_branch: project.default_branch,
                    })
                })
                .collect()
        })
        .await
}

pub(crate) async fn save(store: &Store, project: &Watched, watch: GitWatch) -> Result<(), Refused> {
    let project = project.clone();
    server(store, None)
        .write("git.watch", None, move |tx| {
            if still_bound(tx, &project)? {
                tx.save_git_watch(&project.key, &watch)?;
            }
            Ok(())
        })
        .await?;
    Ok(())
}

fn still_bound(view: &View<'_>, project: &Watched) -> Result<bool, Refused> {
    let current = board::project(view, &project.key)?;
    Ok(
        current.repository.as_deref() == Some(project.repository.as_str())
            && current.default_branch == project.default_branch,
    )
}

pub(crate) async fn settle(
    store: &Store,
    project: &Watched,
    landed: Vec<Landed>,
) -> Result<(), Refused> {
    for (issue, landings) in by_issue(store, project, landed).await? {
        settle_issue(store, project, issue, landings).await?;
    }
    Ok(())
}

async fn by_issue(
    store: &Store,
    project: &Watched,
    landed: Vec<Landed>,
) -> Result<BTreeMap<String, Vec<IssueLanding>>, Refused> {
    let key = project.key.clone();
    let claims = project.claims.clone();
    server(store, None)
        .read(AsOf::Now, move |view| {
            let mut named: Vec<(String, IssueLanding)> = Vec::new();
            for landed in landed {
                match landed {
                    Landed::Branch {
                        branch,
                        tip,
                        claims: since_claim,
                        place,
                    } => {
                        let said = format!(
                            "{branch} landed in {}: its tip {tip}, {} at {}",
                            place.name, place.name, place.tip
                        );
                        let mut issues: Vec<String> = claims
                            .iter()
                            .filter(|claim| claim.branch == branch)
                            .map(|claim| claim.issue.clone())
                            .collect();
                        issues.extend(ids::in_branch(&branch, &key));
                        for issue in issues {
                            let signal = Signal::Branch {
                                branch: branch.clone(),
                                since_claim: since_claim
                                    .iter()
                                    .find(|claim| claim.issue == issue)
                                    .cloned(),
                            };
                            named.push((
                                issue,
                                IssueLanding {
                                    git_commit: tip.clone(),
                                    landed_in: place.tip.clone(),
                                    branch: Some(branch.clone()),
                                    signal,
                                    said: said.clone(),
                                },
                            ));
                        }
                    }
                    Landed::Trailer {
                        commit,
                        refs,
                        place,
                    } => {
                        let said = format!(
                            "commit {commit}, whose Refs trailer names the issue, landed in {} at {}",
                            place.name, place.tip
                        );
                        for issue in ids::in_trailers(&refs, &key) {
                            named.push((
                                issue,
                                IssueLanding {
                                    git_commit: commit.clone(),
                                    landed_in: place.tip.clone(),
                                    branch: None,
                                    signal: Signal::Trailer,
                                    said: said.clone(),
                                },
                            ));
                        }
                    }
                }
            }
            let mut issues: BTreeMap<String, Vec<IssueLanding>> = BTreeMap::new();
            for (issue, landing) in named {
                let Some(record) = view.issue(&issue)? else {
                    continue;
                };
                if record.project != key || view.git_landed(&issue, &landing.git_commit)? {
                    continue;
                }
                issues.entry(issue).or_default().push(landing);
            }
            Ok(issues)
        })
        .await
}

async fn settle_issue(
    store: &Store,
    project: &Watched,
    issue: String,
    landings: Vec<IssueLanding>,
) -> Result<(), Refused> {
    let branch = landings.iter().find_map(|landing| landing.branch.clone());
    let context = server(store, branch);
    let caller = context.caller.clone();
    let project = project.clone();
    let reason = reason(&landings);
    context
        .write(OPERATION, Some(reason), move |tx| {
            if !still_bound(tx, &project)? {
                return Ok(());
            }
            let claims = tx.claimed_work(&project.key, &project.repository)?;
            let mut fresh = Vec::new();
            for landing in landings {
                if !tx.git_landed(&issue, &landing.git_commit)? {
                    fresh.push(landing);
                }
            }
            if fresh.is_empty() {
                return Ok(());
            }
            let record = board::issue(tx, &issue)?;
            let domain_project = model::domain_project(&board::project(tx, &record.project)?)?;
            let domain_issue = Board::load(tx)?.issue(&record)?;
            for landing in &fresh {
                let event = Event::Merge {
                    landing: match &landing.signal {
                        Signal::Branch {
                            branch,
                            since_claim,
                        } => Landing::Branch {
                            branch: branch.clone(),
                            since_claim: since_claim
                                .as_ref()
                                .is_some_and(|claim| claims.contains(claim)),
                        },
                        Signal::Trailer => Landing::Trailer,
                    },
                };
                match decide(&event, &domain_issue, &caller, &domain_project) {
                    Ok(Outcome::Changed(change)) => {
                        apply(tx, &issue, &change, None)?;
                        break;
                    }
                    Ok(Outcome::Unchanged | Outcome::LeftForHuman) => {}
                    Err(Failure::BaseNotMerged { .. }) => {
                        tx.hold_merge(&issue)?;
                        break;
                    }
                    Err(failure) => return Err(failure.into()),
                }
            }
            for landing in &fresh {
                tx.record_git_landing(&GitLanding {
                    issue: issue.clone(),
                    git_commit: landing.git_commit.clone(),
                    project: project.key.clone(),
                    branch: landing.branch.clone(),
                    landed_in: landing.landed_in.clone(),
                })?;
            }
            Ok(())
        })
        .await?;
    Ok(())
}

fn reason(landings: &[IssueLanding]) -> String {
    let mut said: Vec<&str> = Vec::new();
    for landing in landings {
        if !said.contains(&landing.said.as_str()) {
            said.push(&landing.said);
        }
    }
    let said = said.join("; ");
    match said.char_indices().nth(MOST_REASON) {
        Some((cut, _)) => said[..cut].to_owned(),
        None => said,
    }
}

fn server(store: &Store, branch: Option<String>) -> Context {
    Context::new(
        store.clone(),
        Caller {
            actor: Actor::server(),
            working_copy: WorkingCopy {
                branch,
                ..WorkingCopy::default()
            },
        },
    )
}
