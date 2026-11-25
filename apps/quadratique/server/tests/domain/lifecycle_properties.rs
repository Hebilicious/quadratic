use proptest::prelude::*;
use proptest::sample::Index;
use quadratique_server::domain::{
    Assignee, Caller, Category, Change, Claim, ClaimAfter, Event, Issue, Landing, Moves, Outcome,
    Project, Wait, WaitKind, Workflow, WorkflowStatus, create_status, decide,
};

use super::support::{agent_in, agent_with_role, claim_of, human, property};

const CATEGORIES: [Category; 5] = [
    Category::Backlog,
    Category::Unstarted,
    Category::Started,
    Category::Completed,
    Category::Canceled,
];

fn category() -> impl Strategy<Value = Category> {
    prop::sample::select(CATEGORIES.to_vec())
}

fn workflow() -> impl Strategy<Value = Workflow> {
    let merge_category = prop::sample::select(vec![Category::Started, Category::Completed]);
    (
        prop::collection::vec(category(), 0..=17),
        merge_category,
        any::<Index>(),
        any::<Index>(),
        any::<Index>(),
    )
        .prop_flat_map(|(extra, merge_category, claim, review, release)| {
            let mut categories = vec![Category::Unstarted, Category::Started, merge_category];
            categories.extend(extra);
            let merge_position = 2;
            let tagged: Vec<(usize, Category)> = categories.into_iter().enumerate().collect();
            (
                Just(tagged).prop_shuffle(),
                Just((merge_position, claim, review, release)),
            )
        })
        .prop_map(|(tagged, (merge_position, claim, review, release))| {
            let statuses: Vec<WorkflowStatus> = tagged
                .iter()
                .enumerate()
                .map(|(place, (_, category))| WorkflowStatus {
                    key: format!("s{place}"),
                    name: format!("Status {place}"),
                    category: *category,
                })
                .collect();
            let merge = statuses[tagged
                .iter()
                .position(|(origin, _)| *origin == merge_position)
                .expect("the merge status is in the list")]
            .key
            .clone();
            let pick = |index: Index, wanted: Category| {
                let candidates: Vec<&WorkflowStatus> = statuses
                    .iter()
                    .filter(|status| status.category == wanted && status.key != merge)
                    .collect();
                index.get(&candidates).key.clone()
            };
            let moves = Moves {
                claim: pick(claim, Category::Started),
                review: pick(review, Category::Started),
                merge: merge.clone(),
                release: pick(release, Category::Unstarted),
            };
            Workflow { statuses, moves }
        })
}

fn callers() -> Vec<Caller> {
    let mut without_worktree = agent_in("/w/fix-login", "fix-login");
    without_worktree.working_copy.worktree = None;
    let mut human_in_main = human();
    human_in_main.working_copy = agent_in("/w/demo", "main").working_copy;
    vec![
        agent_in("/w/fix-login", "fix-login"),
        agent_in("/w/add-search", "add-search"),
        agent_in("/w/demo", "main"),
        agent_with_role("security"),
        without_worktree,
        human(),
        human_in_main,
    ]
}

fn assignee() -> impl Strategy<Value = Option<Assignee>> {
    prop_oneof![
        Just(None),
        Just(Some(Assignee::Human)),
        Just(Some(Assignee::Agent {
            role: "security".into()
        })),
        Just(Some(Assignee::Agent {
            role: "web-design".into()
        })),
    ]
}

fn wait() -> impl Strategy<Value = Wait> {
    (
        prop::sample::select(vec!["DEM-1", "DEM-2", "DEM-3"]),
        prop::sample::select(vec![
            WaitKind::Blocker,
            WaitKind::Child,
            WaitKind::AncestorBlocker,
        ]),
        prop::option::of(category()),
    )
        .prop_map(|(on, kind, category)| Wait {
            on: on.into(),
            kind,
            category,
        })
}

#[derive(Clone, Debug)]
enum Step {
    Claim(Vec<Wait>),
    Release,
    Close,
    Update {
        revision_offset: i8,
        status: Index,
        unknown_status: bool,
        changes_assignee: bool,
    },
    Merge(Option<(&'static str, bool)>),
}

fn step() -> impl Strategy<Value = (Step, Index)> {
    let step = prop_oneof![
        prop::collection::vec(wait(), 0..3).prop_map(Step::Claim),
        Just(Step::Release),
        Just(Step::Close),
        (
            -1i8..=1,
            any::<Index>(),
            prop::bool::weighted(0.1),
            any::<bool>()
        )
            .prop_map(
                |(revision_offset, status, unknown_status, changes_assignee)| Step::Update {
                    revision_offset,
                    status,
                    unknown_status,
                    changes_assignee,
                }
            ),
        prop::option::of((
            prop::sample::select(vec!["fix-login", "add-search", "main"]),
            any::<bool>(),
        ))
        .prop_map(Step::Merge),
    ];
    (step, any::<Index>())
}

fn event(step: &Step, issue: &Issue, workflow: &Workflow) -> Event {
    match step {
        Step::Claim(waits) => Event::Claim {
            waits: waits.clone(),
            at: "2026-11-18T10:00:00Z".into(),
            worktree: None,
        },
        Step::Release => Event::Release,
        Step::Close => Event::Close,
        Step::Update {
            revision_offset,
            status,
            unknown_status,
            changes_assignee,
        } => Event::Update {
            revision: issue
                .revision
                .saturating_add_signed((*revision_offset).into()),
            status: Some(if *unknown_status {
                "not-a-status".into()
            } else {
                status.get(&workflow.statuses).key.clone()
            }),
            changes_assignee: *changes_assignee,
        },
        Step::Merge(branch) => Event::Merge {
            landing: match branch {
                Some((branch, since_claim)) => Landing::Branch {
                    branch: (*branch).into(),
                    since_claim: *since_claim,
                },
                None => Landing::Trailer,
            },
        },
    }
}

fn initial_issue(
    workflow: &Workflow,
    status: Index,
    holder: Index,
    assignee: Option<Assignee>,
) -> Issue {
    let status = status.get(&workflow.statuses);
    let may_hold = status.category == Category::Started && status.key != workflow.moves.merge;
    let claim: Option<Claim> = may_hold.then(|| {
        let holders = callers();
        let holder = holder.get(&holders);
        let mut claim = claim_of(holder);
        if claim.worktree.is_empty() {
            claim.worktree = "/w/gone".into();
        }
        claim
    });
    Issue {
        id: "DEM-12".into(),
        status: status.key.clone(),
        category: status.category,
        revision: 3,
        claim,
        assignee,
        bases: Vec::new(),
    }
}

fn claim_rule_holds(issue: &Issue, workflow: &Workflow) -> bool {
    issue.claim.is_none()
        || (workflow.category_of(&issue.status) == Some(Category::Started)
            && issue.status != workflow.moves.merge)
}

fn ready_for_a_claim(step: &Step, issue: &Issue, caller: &Caller) -> bool {
    let resolved = |wait: &Wait| {
        matches!(
            wait.category,
            Some(Category::Completed | Category::Canceled)
        )
    };
    let Step::Claim(waits) = step else {
        return false;
    };
    waits.iter().all(resolved)
        && issue.category == Category::Unstarted
        && issue.claim.is_none()
        && issue.assignee.is_none()
        && caller.working_copy.worktree.is_some()
        && caller.working_copy.branch.is_some()
}

#[test]
fn no_sequence_of_events_leaves_a_claim_outside_the_started_category_and_a_ready_claim_is_taken() {
    let generated = (
        workflow(),
        any::<Index>(),
        any::<Index>(),
        assignee(),
        any::<bool>(),
        prop::collection::vec(step(), 1..12),
    );
    property(
        2000,
        generated,
        |(workflow, status, holder, assignee, created, steps)| {
            let project = Project {
                workflow: workflow.clone(),
                default_branch: "main".into(),
                repository: Some("/home/sam/code/demo/.git".into()),
            };
            let mut issue = initial_issue(&workflow, status, holder, assignee);
            if created {
                issue.status = create_status(&workflow, None)
                    .expect("a valid workflow has an unstarted status");
                issue.category = workflow
                    .category_of(&issue.status)
                    .expect("the status is in the workflow");
                issue.claim = None;
            }
            prop_assert!(claim_rule_holds(&issue, &workflow));
            let callers = callers();
            for (step, caller) in &steps {
                let caller = caller.get(&callers);
                let event = event(step, &issue, &workflow);
                let decided = decide(&event, &issue, caller, &project);
                if ready_for_a_claim(step, &issue, caller) {
                    prop_assert!(
                        matches!(
                            &decided,
                            Ok(Outcome::Changed(Change { status, claim: ClaimAfter::Started(claim), .. }))
                                if *status == workflow.moves.claim && claim.holder == caller.actor
                        ),
                        "a ready, unassigned issue was not claimed by {:?}: {:?}",
                        caller.actor.name,
                        decided
                    );
                }
                let Ok(Outcome::Changed(change)) = decided else {
                    continue;
                };
                let category = workflow.category_of(&change.status);
                prop_assert!(
                    category.is_some(),
                    "{} is not in the workflow",
                    change.status
                );
                let entered_merge =
                    change.status == workflow.moves.merge && issue.status != workflow.moves.merge;
                issue.claim = match change.claim {
                    ClaimAfter::Kept => issue.claim.clone(),
                    ClaimAfter::Started(claim) => {
                        prop_assert!(
                            matches!(event, Event::Claim { .. }),
                            "{:?} started a claim",
                            event
                        );
                        Some(*claim)
                    }
                    ClaimAfter::Ended => None,
                    ClaimAfter::Freed(former) => {
                        prop_assert!(
                            matches!(event, Event::Release)
                                && issue.claim.as_ref() == Some(&*former)
                                && change.status == issue.status,
                            "{:?} freed {:?} and moved to {}",
                            event,
                            former,
                            change.status
                        );
                        None
                    }
                };
                issue.status = change.status;
                issue.category = category.expect("checked above");
                issue.revision += 1;
                prop_assert!(
                    !(entered_merge && issue.claim.is_some()),
                    "a claim survived entering the merge status"
                );
                prop_assert!(
                    claim_rule_holds(&issue, &workflow),
                    "claim rule broken after {:?}: {:?}",
                    event,
                    issue
                );
            }
            Ok(())
        },
    );
}
