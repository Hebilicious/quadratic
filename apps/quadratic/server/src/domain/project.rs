use super::failure::Failure;
use super::issue::{Issue, IssueId, Project};
use super::workflow::Status;

pub fn check_project_update(
    current: &Project,
    proposed: &Project,
    issues: &[Issue],
) -> Result<(), Failure> {
    proposed.workflow.validate()?;

    let displaced: Vec<&Issue> = issues
        .iter()
        .filter(|issue| proposed.workflow.category_of(&issue.status) != Some(issue.category))
        .collect();
    if !displaced.is_empty() {
        let mut statuses: Vec<Status> = Vec::new();
        for issue in &displaced {
            if !statuses.contains(&issue.status) {
                statuses.push(issue.status.clone());
            }
        }
        return Err(Failure::StatusInUse {
            statuses,
            issues: displaced.iter().map(|issue| issue.id.clone()).collect(),
        });
    }

    let binding_changes = proposed.repository != current.repository
        || proposed.default_branch != current.default_branch;
    let in_proposed_merge_status = |issue: &Issue| issue.status == proposed.workflow.moves.merge;
    let held: Vec<IssueId> = issues
        .iter()
        .filter(|issue| {
            issue.claim.is_some() && (binding_changes || in_proposed_merge_status(issue))
        })
        .map(|issue| issue.id.clone())
        .collect();
    if !held.is_empty() {
        return Err(Failure::ClaimsHeld { issues: held });
    }
    Ok(())
}
