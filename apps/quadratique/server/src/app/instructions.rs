use acid_cat_core::{CapabilityDefinition, NoInput};
use serde::Serialize;
use serde_json::Value;

use super::catalog::capability_without_input;
use super::{Context, Refused, finish};
use crate::domain::{Actor, ActorKind};
use crate::store::AsOf;

const REFERENCE: &str = "\
The skill `quadratique` gives the workflow. This is the command reference. Every command prints JSON; `quadratique <command> --help` lists its flags. Issue ids look like ABC-12. An answer about one issue has it under `issue`; a list has its entries under `items`.

Issues
- `quadratique issue list` filters by `--claim mine` (your session's claims anywhere), `--status`, `--category`, `--assignee`, `--labels`, `--text`, `--asOf`. `notReady` says why an issue is not ready: `category` (backlog, started or finished: only an unstarted issue is ready), `blocked` (an open issue blocks it or a parent, named in `blockedBy`), `children` (open children), `assigned` (to someone else), `claimed` (someone else holds it), `mine` (you hold it). An issue you hold shows `category` and `mine`, not blocked; it also shows `stacked` while an issue it is stacked on is back before review.
- `quadratique ready`: the issues you can start now, those for your role first, then by priority (1 urgent to 4 low, 0 none last). A snapshot.
- `quadratique show --issue ABC-12`: one issue with its notes and relations. `quadratique history query --issue ABC-12`: its changes.
- `quadratique create --title \"...\" --description \"...\" --acceptance \"...\" --priority 2`; `--acceptance` is required; `--origin ABC-12` links work found on that issue; `--assignee '{\"kind\":\"human\"}'` assigns it to the human.
- `quadratique issue link --issue A --kind blocks --target B`.
- `quadratique issue link --issue B --kind stacked-on --target A`: B's branch starts from A's. B is ready once A is in review, from A's branch or one on top of it, and reaches merged only after A (`BaseNotMerged`). `stacked` (an issue it is stacked on holds it back, named in `blockedBy`): `show` that issue; before review, wait for it, and a claim you hold stays yours; canceled, remove the link; in review, work from a branch of the same repository that contains its branch.

Work
- `quadratique claim --issue ABC-12`: atomic; prints the whole issue. `Held` (exit code 1): someone else holds it. `NotReady`: what it waits on.
- `quadratique issue update --issue ABC-12 --revision <revision>` with `--notes` (added after the earlier notes), `--description` or `--acceptance` (replaced), `--status`, `--priority`. On `Stale`, `show` and write again.
- `quadratique close --issue ABC-12 --evidence \"...\"`: on a feature branch the issue moves to review and stays yours until the merge. Commits end with `Refs: ABC-12`; Quadratique then sees the merge, even a squash, and moves the issue on.
- `quadratique release --issue ABC-12 --reason \"...\"` gives a claim back.
- An issue with a `source` is a finding of a tool: the tool's next complete run finishes it when the finding is gone, and reopens it when the finding comes back.

Keep tasks in Quadratique only: never in files of the repository.";

#[derive(Serialize)]
pub(super) struct Instructions {
    text: String,
    actor: Actor,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<String>,
}

pub(super) fn instructions() -> CapabilityDefinition<Context, NoInput, Instructions, Value> {
    capability_without_input::<Instructions>("workspace.instructions")
        .run(|(), call| async move { finish(give_instructions(call.context()).await) })
}

async fn give_instructions(context: &Context) -> Result<Instructions, Refused> {
    let caller = context.caller.clone();
    let repository = caller.working_copy.repository.clone();
    let project = context
        .read(AsOf::Now, move |view| match &repository {
            Some(repository) => Ok(view
                .project_by_repository(repository)?
                .map(|project| project.key)),
            None => Ok(None),
        })
        .await?;
    Ok(Instructions {
        text: format!(
            "{}\n\n{REFERENCE}",
            whereabouts(&caller.actor, project.as_deref())
        ),
        actor: caller.actor,
        project,
        branch: caller.working_copy.branch,
        worktree: caller.working_copy.worktree,
    })
}

fn whereabouts(actor: &Actor, project: Option<&str>) -> String {
    let kind = match actor.kind {
        ActorKind::Human => "the human",
        ActorKind::Agent => "an agent",
    };
    match project {
        Some(project) => format!(
            "You are {name} ({kind}). This repository is the project {project}.",
            name = actor.name
        ),
        None => format!(
            "You are {name} ({kind}). No project has this repository: ask the human to run `quadratique project create --key ABC --name \"...\"` in it, or give `--project` to each command.",
            name = actor.name
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::REFERENCE;
    use crate::domain::{
        Actor, Caller, Category, Issue, NotReadyReason, Readiness, WorkingCopy, readiness,
    };

    fn every_reason() -> [NotReadyReason; 7] {
        let every = [
            NotReadyReason::Category,
            NotReadyReason::Blocked,
            NotReadyReason::Children,
            NotReadyReason::Stacked,
            NotReadyReason::Assigned,
            NotReadyReason::Claimed,
            NotReadyReason::Mine,
        ];
        for reason in every {
            match reason {
                NotReadyReason::Category
                | NotReadyReason::Blocked
                | NotReadyReason::Children
                | NotReadyReason::Stacked
                | NotReadyReason::Assigned
                | NotReadyReason::Claimed
                | NotReadyReason::Mine => {}
            }
        }
        every
    }

    fn meaning(name: &str) -> &'static str {
        let opening = format!("`{name}` (");
        let start = REFERENCE
            .find(&opening)
            .map(|at| at + opening.len())
            .unwrap_or_else(|| panic!("the reference names `{name}` without its meaning"));
        let length = REFERENCE[start..].find(')').expect("a closed meaning");
        &REFERENCE[start..start + length]
    }

    #[test]
    fn a_backlog_issue_is_not_ready_for_its_category_and_the_reference_says_so() {
        let backlog = Issue {
            id: "DEM-1".into(),
            status: "backlog".into(),
            category: Category::Backlog,
            revision: 1,
            claim: None,
            assignee: None,
            bases: Vec::new(),
        };
        let caller = Caller {
            actor: Actor::without_header(),
            working_copy: WorkingCopy::default(),
        };

        let Readiness::NotReady { reasons, .. } = readiness(&backlog, &[], &caller) else {
            panic!("a backlog issue is ready");
        };
        assert_eq!(reasons, vec![NotReadyReason::Category]);
        assert!(
            meaning("category").contains("backlog"),
            "{}",
            meaning("category")
        );
    }

    #[test]
    fn the_reference_says_what_each_not_ready_reason_means() {
        for reason in every_reason() {
            let name = serde_json::to_value(reason).expect("a reason serializes");
            let name = name.as_str().expect("a reason is a string");
            assert!(!meaning(name).is_empty(), "`{name}` has an empty meaning");
        }
    }
}
