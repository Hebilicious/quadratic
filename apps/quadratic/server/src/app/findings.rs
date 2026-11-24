use std::collections::{BTreeMap, BTreeSet};

use acid_cat_core::CapabilityDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::board::{Board, caller_project};
use super::catalog::capability;
use super::model::IssueIds;
use super::{Context, Refused, finish, model};
use crate::domain::{
    Category, Finding, MAX_BLOCKS_CYCLE_CHECKED_IN_ONE_WRITE, RelationChange, Sourced, Step,
    Workflow, create_status, decision_blocks, step,
};
use crate::store::{FindingLink, IssueFields, NewIssue, SourceRecord, Tx};

const OPERATION: &str = "findings.import";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImportInput {
    project: Option<String>,
    tool: String,
    findings: Vec<Finding>,
    #[serde(default = "complete_by_default")]
    complete: bool,
}

fn complete_by_default() -> bool {
    true
}

#[derive(Serialize)]
pub(super) struct Imported {
    created: IssueIds,
    updated: IssueIds,
    reopened: IssueIds,
    finished: IssueIds,
    left: IssueIds,
}

#[derive(Default)]
struct Outcomes {
    created: Vec<String>,
    updated: Vec<String>,
    reopened: Vec<String>,
    finished: Vec<String>,
    left: Vec<String>,
}

pub(super) fn import() -> CapabilityDefinition<Context, ImportInput, Imported, Value> {
    capability::<ImportInput, Imported>(OPERATION)
        .run(|input, call| async move { finish(import_run(call.context(), input).await) })
}

async fn import_run(context: &Context, input: ImportInput) -> Result<Imported, Refused> {
    refuse_duplicate_keys(&input.findings)?;
    let caller = context.caller.clone();
    let author = serde_json::to_string(&caller.actor).map_err(Refused::defect)?;
    let written = context
        .write(OPERATION, None, move |tx| {
            let project = caller_project(tx, input.project.as_deref(), &caller)?;
            let run = Run {
                project: project.key.clone(),
                workflow: model::workflow(&project.workflow)?,
                tool: input.tool,
                author,
            };
            run.apply(tx, &input.findings, input.complete)
        })
        .await?;
    let outcomes = written.value;
    Ok(Imported {
        created: IssueIds::of(outcomes.created),
        updated: IssueIds::of(outcomes.updated),
        reopened: IssueIds::of(outcomes.reopened),
        finished: IssueIds::of(outcomes.finished),
        left: IssueIds::of(outcomes.left),
    })
}

fn refuse_duplicate_keys(findings: &[Finding]) -> Result<(), Refused> {
    let mut seen = BTreeSet::new();
    for finding in findings {
        if !seen.insert(finding.key.as_str()) {
            return Err(Refused::Failure(
                json!({ "_tag": "DuplicateKey", "key": finding.key }),
            ));
        }
    }
    Ok(())
}

struct Run {
    project: String,
    workflow: Workflow,
    tool: String,
    author: String,
}

impl Run {
    fn apply(
        &self,
        tx: &mut Tx<'_>,
        findings: &[Finding],
        complete: bool,
    ) -> Result<Outcomes, Refused> {
        let existing: BTreeMap<String, SourceRecord> = tx
            .sources(&self.project, &self.tool)?
            .into_iter()
            .map(|source| (source.key.clone(), source))
            .collect();
        let mut outcomes = Outcomes::default();
        for finding in findings {
            let issue = existing.get(&finding.key);
            let category = issue.map(category).transpose()?;
            match (step(category, true, complete), issue) {
                (Step::Create, _) => outcomes.created.push(self.create(tx, finding)?),
                (Step::Update, Some(issue)) => {
                    if self.update(tx, &issue.issue, finding)? {
                        outcomes.updated.push(issue.issue.clone());
                    } else {
                        outcomes.left.push(issue.issue.clone());
                    }
                }
                (Step::Reopen, Some(issue)) => {
                    tx.set_status(&issue.issue, &self.unstarted())?;
                    forget_finish(tx, issue)?;
                    self.update(tx, &issue.issue, finding)?;
                    outcomes.reopened.push(issue.issue.clone());
                }
                (_, Some(issue)) => outcomes.left.push(issue.issue.clone()),
                (_, None) => {}
            }
        }
        let reported: BTreeSet<&str> = findings
            .iter()
            .map(|finding| finding.key.as_str())
            .collect();
        for (key, issue) in &existing {
            if reported.contains(key.as_str()) {
                continue;
            }
            if step(Some(category(issue)?), false, complete) == Step::Finish {
                self.finish(tx, &issue.issue)?;
                outcomes.finished.push(issue.issue.clone());
            }
        }
        self.keep_decision_blocks(tx)?;
        Ok(outcomes)
    }

    fn unstarted(&self) -> String {
        match create_status(&self.workflow, None) {
            Ok(status) => status,
            Err(_) => self.workflow.moves.release.clone(),
        }
    }

    fn create(&self, tx: &mut Tx<'_>, finding: &Finding) -> Result<String, Refused> {
        let created = tx.create_issue(&NewIssue {
            project: self.project.clone(),
            title: finding.title.clone(),
            status: self.unstarted(),
            priority: finding.priority.unwrap_or(0),
            labels: finding.labels.clone().unwrap_or_default(),
            assignee: finding
                .assignee
                .as_ref()
                .map(model::assignee_text)
                .transpose()?,
            description: Some(finding.description.clone()),
            created_by: self.author.clone(),
            ..NewIssue::default()
        })?;
        let id = created.id().to_owned();
        tx.set_source(&id, &self.tool, &finding.key, finding.decision)?;
        if let Some(links) = &finding.links {
            tx.set_finding_links(&id, &stored_links(links))?;
        }
        Ok(id)
    }

    fn update(&self, tx: &mut Tx<'_>, id: &str, finding: &Finding) -> Result<bool, Refused> {
        let mut changed = tx.update_fields(
            id,
            &IssueFields {
                title: Some(finding.title.clone()),
                description: Some(Some(finding.description.clone())),
                priority: finding.priority,
                assignee: finding
                    .assignee
                    .as_ref()
                    .map(|assignee| model::assignee_text(assignee).map(Some))
                    .transpose()?,
                ..IssueFields::default()
            },
        )?;
        if let Some(labels) = &finding.labels {
            changed |= tx.set_labels(id, labels)?;
        }
        if let Some(links) = &finding.links {
            changed |= tx.set_finding_links(id, &stored_links(links))?;
        }
        changed |= tx.set_source(id, &self.tool, &finding.key, finding.decision)?;
        Ok(changed)
    }

    fn finish(&self, tx: &mut Tx<'_>, id: &str) -> Result<(), Refused> {
        let Some(completed) = self.workflow.first_of(Category::Completed) else {
            return Err(Refused::Failure(
                json!({ "_tag": "NoCompletedStatus", "project": self.project }),
            ));
        };
        let evidence = format!(
            "The complete run of {} imported at {} by {} no longer reports this finding.",
            self.tool,
            tx.now(),
            model::actor(&self.author)?.name,
        );
        tx.set_status(id, completed)?;
        tx.clear_claim(id)?;
        tx.update_fields(
            id,
            &IssueFields {
                evidence: Some(Some(evidence.clone())),
                ..IssueFields::default()
            },
        )?;
        tx.set_finished_evidence(id, Some(&evidence))?;
        Ok(())
    }

    fn keep_decision_blocks(&self, tx: &mut Tx<'_>) -> Result<(), Refused> {
        let mut issues = Vec::new();
        for source in tx.sources(&self.project, &self.tool)? {
            issues.push(Sourced {
                open: !category(&source)?.is_resolved(),
                links: tx
                    .finding_links(&source.issue)?
                    .into_iter()
                    .map(|link| crate::domain::Link {
                        kind: link.kind,
                        id: link.id,
                    })
                    .collect(),
                decision: source.decision,
                id: source.issue,
            });
        }
        let implied = decision_blocks(&issues).map_err(|_| {
            Refused::Failure(
                json!({ "_tag": "TooManyBlocks", "limit": MAX_BLOCKS_CYCLE_CHECKED_IN_ONE_WRITE }),
            )
        })?;
        let made: BTreeSet<(String, String)> = tx
            .finding_blocks(&self.project, &self.tool)?
            .into_iter()
            .collect();
        for (decision, blocked) in made.difference(&implied) {
            tx.remove_relation(decision, "blocks", blocked)?;
        }
        let mut board = Board::load(tx)?;
        for (decision, blocked) in implied.difference(&made) {
            let change = RelationChange::Blocks {
                blocker: decision.clone(),
                blocked: blocked.clone(),
            };
            if board.check(&change).is_ok() {
                tx.add_finding_block(decision, blocked)?;
                board.apply(change);
            }
        }
        Ok(())
    }
}

fn forget_finish(tx: &mut Tx<'_>, source: &SourceRecord) -> Result<(), Refused> {
    let Some(written) = &source.finished_evidence else {
        return Ok(());
    };
    let current = tx.issue(&source.issue)?.and_then(|issue| issue.evidence);
    if current.as_ref() == Some(written) {
        tx.update_fields(
            &source.issue,
            &IssueFields {
                evidence: Some(None),
                ..IssueFields::default()
            },
        )?;
    }
    tx.set_finished_evidence(&source.issue, None)?;
    Ok(())
}

fn category(source: &SourceRecord) -> Result<Category, Refused> {
    source
        .category
        .as_deref()
        .and_then(model::category)
        .ok_or_else(|| {
            Refused::defect(format!(
                "{} is in a status its workflow lacks",
                source.issue
            ))
        })
}

fn stored_links(links: &[crate::domain::Link]) -> Vec<FindingLink> {
    links
        .iter()
        .map(|link| FindingLink {
            kind: link.kind.clone(),
            id: link.id.clone(),
        })
        .collect()
}
