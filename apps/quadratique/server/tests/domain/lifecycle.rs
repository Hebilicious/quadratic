use quadratique_server::domain::{
    Assignee, Category, Change, ClaimAfter, Event, Failure, Landing, NotReadyReason, Outcome, Wait,
    WaitKind, create_status, decide,
};

use super::support::{
    Board, agent, agent_in, agent_with_role, assigned, boards, claim_of, claimed, human,
    other_agent,
};

const NOW: &str = "2026-11-18T10:00:00Z";

fn claim_event() -> Event {
    Event::Claim {
        waits: vec![],
        at: NOW.into(),
        worktree: None,
    }
}

fn update_to(status: &str) -> Event {
    Event::Update {
        revision: 3,
        status: Some(status.into()),
        changes_assignee: false,
    }
}

fn moved(status: &str, claim: ClaimAfter, takes_evidence: bool) -> Result<Outcome, Failure> {
    Ok(Outcome::Changed(Change {
        status: status.into(),
        claim,
        takes_evidence,
    }))
}

mod issue_claim {
    use super::*;

    #[test]
    fn row_1_the_caller_not_in_a_git_worktree_on_a_branch_gets_no_working_copy() {
        for board in boards() {
            let mut detached = agent();
            detached.working_copy.branch = None;
            let mut outside = agent();
            outside.working_copy.worktree = None;
            outside.working_copy.repository = None;
            for caller in [detached, outside, human()] {
                assert_eq!(
                    decide(
                        &claim_event(),
                        &board.issue(board.todo),
                        &caller,
                        &board.project
                    ),
                    Err(Failure::NoWorkingCopy),
                    "{}",
                    board.label
                );
            }
        }
    }

    fn claim_for(worktree: &str) -> Event {
        Event::Claim {
            waits: vec![],
            at: NOW.into(),
            worktree: Some(worktree.into()),
        }
    }

    #[test]
    fn row_1_a_named_worktree_that_is_not_the_callers_or_not_in_the_projects_repository_gets_no_working_copy()
     {
        for board in boards() {
            let issue = board.issue(board.todo);
            let mut elsewhere = agent();
            elsewhere.working_copy.repository = Some("/elsewhere/.git".into());
            let mut unbound = board.project.clone();
            unbound.repository = None;
            let mut without_repository = agent();
            without_repository.working_copy.repository = None;
            let cases = [
                (claim_for("/w/add-search"), agent(), &board.project),
                (claim_for("/w/fix-login"), elsewhere, &board.project),
                (claim_for("/w/fix-login"), agent(), &unbound),
                (claim_for("/w/fix-login"), without_repository, &unbound),
            ];
            for (event, caller, project) in cases {
                assert_eq!(
                    decide(&event, &issue, &caller, project),
                    Err(Failure::NoWorkingCopy),
                    "{}: {event:?} by {caller:?}",
                    board.label
                );
            }
        }
    }

    #[test]
    fn row_2_claimed_by_the_caller_is_unchanged() {
        for board in boards() {
            let issue = claimed(board.issue(board.in_progress), claim_of(&agent()));
            assert_eq!(
                decide(&claim_event(), &issue, &agent(), &board.project),
                Ok(Outcome::Unchanged),
                "{}",
                board.label
            );
        }
    }

    #[test]
    fn row_3_claimed_by_someone_else_is_held() {
        for board in boards() {
            let holder = other_agent();
            let issue = claimed(board.issue(board.in_progress), claim_of(&holder));
            let same_name_other_worktree = {
                let mut caller = agent_in("/w/elsewhere", "elsewhere");
                caller.actor.name = holder.actor.name.clone();
                caller
            };
            for caller in [agent(), same_name_other_worktree] {
                assert_eq!(
                    decide(&claim_event(), &issue, &caller, &board.project),
                    Err(Failure::Held {
                        issue: "DEM-12".into(),
                        claim: Box::new(claim_of(&holder)),
                    }),
                    "{}",
                    board.label
                );
            }
        }
    }

    #[test]
    fn row_4_unstarted_and_ready_moves_to_the_claim_status_and_starts_the_claim() {
        let expected = |board: &Board| {
            let mut claim = claim_of(&agent());
            claim.since = NOW.into();
            moved(
                board.in_progress,
                ClaimAfter::Started(Box::new(claim)),
                false,
            )
        };
        for board in boards() {
            assert_eq!(
                decide(
                    &claim_event(),
                    &board.issue(board.todo),
                    &agent(),
                    &board.project
                ),
                expected(&board),
                "{}: todo -> in-progress",
                board.label
            );
        }
    }

    #[test]
    fn row_5_otherwise_is_not_ready_with_its_reasons_and_blockers() {
        for board in boards() {
            assert_eq!(
                decide(
                    &claim_event(),
                    &board.issue(board.backlog),
                    &agent(),
                    &board.project
                ),
                Err(Failure::NotReady {
                    issue: "DEM-12".into(),
                    status: board.backlog.into(),
                    category: Category::Backlog,
                    reasons: vec![NotReadyReason::Category],
                    blockers: vec![],
                }),
                "{}",
                board.label
            );

            let blocked = Event::Claim {
                waits: vec![Wait {
                    on: "DEM-11".into(),
                    kind: WaitKind::Blocker,
                    category: Some(Category::Started),
                }],
                at: NOW.into(),
                worktree: None,
            };
            let for_the_human = assigned(board.issue(board.todo), Assignee::Human);
            assert_eq!(
                decide(&blocked, &for_the_human, &agent(), &board.project),
                Err(Failure::NotReady {
                    issue: "DEM-12".into(),
                    status: board.todo.into(),
                    category: Category::Unstarted,
                    reasons: vec![NotReadyReason::Blocked, NotReadyReason::Assigned],
                    blockers: vec!["DEM-11".into()],
                }),
                "{}",
                board.label
            );
        }
    }
}

mod issue_update {
    use super::*;

    #[test]
    fn row_1_a_stale_revision_is_stale() {
        for board in boards() {
            for revision in [2, 4] {
                let event = Event::Update {
                    revision,
                    status: Some(board.in_review.into()),
                    changes_assignee: false,
                };
                assert_eq!(
                    decide(
                        &event,
                        &board.issue(board.in_progress),
                        &human(),
                        &board.project
                    ),
                    Err(Failure::Stale { revision: 3 }),
                    "{}",
                    board.label
                );
            }
        }
    }

    #[test]
    fn row_2_a_status_not_in_the_workflow_is_unknown() {
        for board in boards() {
            let statuses: Vec<String> = board
                .project
                .workflow
                .statuses
                .iter()
                .map(|status| status.key.clone())
                .collect();
            assert_eq!(
                decide(
                    &update_to("deployed"),
                    &board.issue(board.todo),
                    &human(),
                    &board.project
                ),
                Err(Failure::UnknownStatus {
                    status: "deployed".into(),
                    statuses,
                }),
                "{}",
                board.label
            );
        }
    }

    #[test]
    fn row_3_an_agent_on_an_issue_assigned_to_the_human_is_human_only() {
        for board in boards() {
            let issue = assigned(board.issue(board.todo), Assignee::Human);
            let assignee_only = Event::Update {
                revision: 3,
                status: None,
                changes_assignee: true,
            };
            for event in [update_to(board.in_progress), assignee_only] {
                assert_eq!(
                    decide(&event, &issue, &agent(), &board.project),
                    Err(Failure::HumanOnly),
                    "{}",
                    board.label
                );
            }
        }
    }

    #[test]
    fn row_4_claimed_by_someone_else_is_held_unless_the_caller_is_the_human() {
        for board in boards() {
            let issue = claimed(board.issue(board.in_progress), claim_of(&other_agent()));
            assert_eq!(
                decide(
                    &update_to(board.in_review),
                    &issue,
                    &agent(),
                    &board.project
                ),
                Err(Failure::Held {
                    issue: "DEM-12".into(),
                    claim: Box::new(claim_of(&other_agent())),
                }),
                "{}",
                board.label
            );
            assert_eq!(
                decide(
                    &update_to(board.in_review),
                    &issue,
                    &human(),
                    &board.project
                ),
                moved(board.in_review, ClaimAfter::Kept, false),
                "{}",
                board.label
            );
        }
    }

    #[test]
    fn row_5_into_the_merge_status_or_out_of_started_ends_any_claim() {
        for board in boards() {
            let issue = board.issue(board.merged);
            assert_eq!(
                decide(&update_to(board.done), &issue, &agent(), &board.project),
                moved(board.done, ClaimAfter::Ended, false),
                "{}: merged -> done",
                board.label
            );
            let held = claimed(board.issue(board.in_progress), claim_of(&agent()));
            for target in [board.merged, board.todo, board.canceled] {
                assert_eq!(
                    decide(&update_to(target), &held, &agent(), &board.project),
                    moved(target, ClaimAfter::Ended, false),
                    "{}: in-progress -> {target}",
                    board.label
                );
            }
        }
    }

    #[test]
    fn row_6_otherwise_moves_to_the_status_and_keeps_the_claim() {
        for board in boards() {
            let held = claimed(board.issue(board.in_progress), claim_of(&agent()));
            assert_eq!(
                decide(&update_to(board.in_review), &held, &agent(), &board.project),
                moved(board.in_review, ClaimAfter::Kept, false),
                "{}: in-progress -> in-review",
                board.label
            );
            let sent_back = claimed(board.issue(board.in_review), claim_of(&agent()));
            assert_eq!(
                decide(
                    &update_to(board.in_progress),
                    &sent_back,
                    &agent(),
                    &board.project
                ),
                moved(board.in_progress, ClaimAfter::Kept, false),
                "{}: in-review -> in-progress",
                board.label
            );
        }
    }

    #[test]
    fn an_update_without_a_status_or_an_assignee_leaves_the_lifecycle_unchanged() {
        for board in boards() {
            let issue = assigned(
                claimed(board.issue(board.in_progress), claim_of(&other_agent())),
                Assignee::Human,
            );
            let fields_only = Event::Update {
                revision: 3,
                status: None,
                changes_assignee: false,
            };
            assert_eq!(
                decide(&fields_only, &issue, &agent(), &board.project),
                Ok(Outcome::Unchanged),
                "{}",
                board.label
            );
        }
    }
}

mod issue_create {
    use super::*;

    #[test]
    fn row_1_a_status_not_in_the_workflow_is_unknown() {
        for board in boards() {
            let statuses: Vec<String> = board
                .project
                .workflow
                .statuses
                .iter()
                .map(|status| status.key.clone())
                .collect();
            assert_eq!(
                create_status(&board.project.workflow, Some("deployed")),
                Err(Failure::UnknownStatus {
                    status: "deployed".into(),
                    statuses,
                }),
                "{}",
                board.label
            );
        }
    }

    #[test]
    fn row_2_otherwise_the_given_status_or_the_first_unstarted_one() {
        for board in boards() {
            assert_eq!(
                create_status(&board.project.workflow, None),
                Ok(board.todo.to_string()),
                "{}",
                board.label
            );
            assert_eq!(
                create_status(&board.project.workflow, Some(board.backlog)),
                Ok(board.backlog.to_string()),
                "{}",
                board.label
            );
        }
    }
}

mod issue_release {
    use super::*;

    #[test]
    fn row_1_unclaimed_is_unchanged() {
        for board in boards() {
            let issue = assigned(board.issue(board.todo), Assignee::Human);
            assert_eq!(
                decide(&Event::Release, &issue, &agent(), &board.project),
                Ok(Outcome::Unchanged),
                "{}",
                board.label
            );
        }
    }

    #[test]
    fn row_2_an_agent_on_an_issue_assigned_to_the_human_is_human_only() {
        for board in boards() {
            let issue = assigned(
                claimed(board.issue(board.in_progress), claim_of(&agent())),
                Assignee::Human,
            );
            assert_eq!(
                decide(&Event::Release, &issue, &agent(), &board.project),
                Err(Failure::HumanOnly),
                "{}",
                board.label
            );
        }
    }

    #[test]
    fn row_3_neither_the_holder_nor_the_human_is_held() {
        for board in boards() {
            let issue = claimed(board.issue(board.in_progress), claim_of(&other_agent()));
            assert_eq!(
                decide(&Event::Release, &issue, &agent(), &board.project),
                Err(Failure::Held {
                    issue: "DEM-12".into(),
                    claim: Box::new(claim_of(&other_agent())),
                }),
                "{}",
                board.label
            );
        }
    }

    #[test]
    fn row_4_otherwise_moves_to_the_release_status_and_ends_the_claim() {
        for board in boards() {
            let issue = claimed(board.issue(board.in_progress), claim_of(&agent()));
            for caller in [agent(), human()] {
                assert_eq!(
                    decide(&Event::Release, &issue, &caller, &board.project),
                    moved(board.todo, ClaimAfter::Ended, false),
                    "{}: in-progress -> todo",
                    board.label
                );
            }
        }
    }
}

mod issue_close {
    use super::*;

    #[test]
    fn row_1_canceled_is_resolved() {
        for board in boards() {
            assert_eq!(
                decide(
                    &Event::Close,
                    &board.issue(board.canceled),
                    &human(),
                    &board.project
                ),
                Err(Failure::Resolved {
                    issue: "DEM-12".into(),
                    status: board.canceled.into(),
                }),
                "{}",
                board.label
            );
        }
    }

    #[test]
    fn row_2_completed_or_in_the_merge_status_is_unchanged() {
        for board in boards() {
            for status in [board.merged, board.done] {
                let issue = assigned(board.issue(status), Assignee::Human);
                assert_eq!(
                    decide(&Event::Close, &issue, &other_agent(), &board.project),
                    Ok(Outcome::Unchanged),
                    "{}: {status}",
                    board.label
                );
            }
        }
    }

    #[test]
    fn row_3_an_agent_on_an_issue_assigned_to_the_human_is_human_only() {
        for board in boards() {
            let issue = assigned(
                claimed(board.issue(board.in_progress), claim_of(&agent())),
                Assignee::Human,
            );
            assert_eq!(
                decide(&Event::Close, &issue, &agent(), &board.project),
                Err(Failure::HumanOnly),
                "{}",
                board.label
            );
        }
    }

    #[test]
    fn row_4_an_agent_that_does_not_hold_the_issue_is_held_or_not_claimed() {
        for board in boards() {
            let held = claimed(board.issue(board.in_progress), claim_of(&other_agent()));
            assert_eq!(
                decide(&Event::Close, &held, &agent(), &board.project),
                Err(Failure::Held {
                    issue: "DEM-12".into(),
                    claim: Box::new(claim_of(&other_agent())),
                }),
                "{}",
                board.label
            );
            assert_eq!(
                decide(
                    &Event::Close,
                    &board.issue(board.todo),
                    &agent(),
                    &board.project
                ),
                Err(Failure::NotClaimed {
                    issue: "DEM-12".into()
                }),
                "{}",
                board.label
            );
        }
    }

    #[test]
    fn row_5_the_holder_off_the_default_branch_moves_to_review_with_the_evidence_and_keeps_the_claim()
     {
        for board in boards() {
            for status in [board.in_progress, board.in_review] {
                let issue = claimed(board.issue(status), claim_of(&agent()));
                assert_eq!(
                    decide(&Event::Close, &issue, &agent(), &board.project),
                    moved(board.in_review, ClaimAfter::Kept, true),
                    "{}: {status} -> in-review",
                    board.label
                );
            }
        }
    }

    #[test]
    fn row_6_otherwise_moves_to_the_merge_status_with_the_evidence_and_ends_the_claim() {
        for board in boards() {
            let on_main = agent_in("/w/demo", "main");
            let issue = claimed(board.issue(board.in_review), claim_of(&on_main));
            assert_eq!(
                decide(&Event::Close, &issue, &on_main, &board.project),
                moved(board.merged, ClaimAfter::Ended, true),
                "{}: in-review -> merged",
                board.label
            );
            let held_by_another = claimed(board.issue(board.in_review), claim_of(&other_agent()));
            assert_eq!(
                decide(&Event::Close, &held_by_another, &human(), &board.project),
                moved(board.merged, ClaimAfter::Ended, true),
                "{}: the human closes",
                board.label
            );
        }
    }
}

mod ref_watcher_merge {
    use super::*;

    fn branch_landing(branch: &str, since_claim: bool) -> Event {
        Event::Merge {
            landing: Landing::Branch {
                branch: branch.into(),
                since_claim,
            },
        }
    }

    fn trailer_landing() -> Event {
        Event::Merge {
            landing: Landing::Trailer,
        }
    }

    fn holders_branch_landing() -> Event {
        branch_landing("fix-login", true)
    }

    #[test]
    fn row_1_an_unclaimed_unresolved_issue_outside_the_merge_status_moves_to_merge_on_any_landing()
    {
        for board in boards() {
            for status in [
                board.backlog,
                board.todo,
                board.in_progress,
                board.in_review,
            ] {
                for landing in [holders_branch_landing(), trailer_landing()] {
                    assert_eq!(
                        decide(&landing, &board.issue(status), &human(), &board.project),
                        moved(board.merged, ClaimAfter::Ended, false),
                        "{}: {status} on {landing:?}",
                        board.label
                    );
                }
            }
        }
    }

    #[test]
    fn row_1_a_claim_on_the_merged_branch_with_work_since_the_claim_moves_to_merge_and_ends_the_claim()
     {
        for board in boards() {
            for status in [board.in_progress, board.in_review] {
                let issue = claimed(board.issue(status), claim_of(&agent()));
                assert_eq!(
                    decide(&holders_branch_landing(), &issue, &human(), &board.project),
                    moved(board.merged, ClaimAfter::Ended, false),
                    "{}: {status} -> merged",
                    board.label
                );
            }
        }
    }

    #[test]
    fn row_1_a_trailer_lands_a_claimed_issue_that_its_holder_closed_into_review() {
        for board in boards() {
            let issue = claimed(board.issue(board.in_review), claim_of(&agent()));
            assert_eq!(
                decide(&trailer_landing(), &issue, &human(), &board.project),
                moved(board.merged, ClaimAfter::Ended, false),
                "{}",
                board.label
            );
        }
    }

    #[test]
    fn row_2_a_resolved_issue_or_one_in_the_merge_status_is_unchanged() {
        for board in boards() {
            for status in [board.merged, board.done, board.canceled] {
                for landing in [holders_branch_landing(), trailer_landing()] {
                    assert_eq!(
                        decide(&landing, &board.issue(status), &human(), &board.project),
                        Ok(Outcome::Unchanged),
                        "{}: {status}",
                        board.label
                    );
                }
            }
        }
    }

    #[test]
    fn row_2_a_claim_on_another_branch_is_unchanged() {
        for board in boards() {
            for status in [board.in_progress, board.in_review] {
                let issue = claimed(board.issue(status), claim_of(&agent()));
                assert_eq!(
                    decide(
                        &branch_landing("old-branch", true),
                        &issue,
                        &human(),
                        &board.project
                    ),
                    Ok(Outcome::Unchanged),
                    "{}: {status}",
                    board.label
                );
            }
        }
    }

    #[test]
    fn row_2_the_claimed_branch_without_work_since_the_claim_is_unchanged() {
        for board in boards() {
            let issue = claimed(board.issue(board.in_review), claim_of(&agent()));
            assert_eq!(
                decide(
                    &branch_landing("fix-login", false),
                    &issue,
                    &human(),
                    &board.project
                ),
                Ok(Outcome::Unchanged),
                "{}",
                board.label
            );
        }
    }

    #[test]
    fn row_2_a_trailer_leaves_an_issue_whose_holder_is_still_at_work() {
        for board in boards() {
            let issue = claimed(board.issue(board.in_progress), claim_of(&agent()));
            assert_eq!(
                decide(&trailer_landing(), &issue, &human(), &board.project),
                Ok(Outcome::Unchanged),
                "{}",
                board.label
            );
        }
    }
}

#[test]
fn an_agent_with_the_assigned_role_can_claim() {
    let board = Board::default_workflow();
    let issue = assigned(
        board.issue("todo"),
        Assignee::Agent {
            role: "security".into(),
        },
    );
    let caller = agent_with_role("security");
    let outcome = decide(&claim_event(), &issue, &caller, &board.project);
    assert!(
        matches!(&outcome, Ok(Outcome::Changed(change)) if change.status == "in-progress"),
        "{outcome:?}"
    );
}
