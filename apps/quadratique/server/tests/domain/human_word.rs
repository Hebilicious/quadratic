//! Rule 4 of `docs/decisions/0002-claims-of-agents-that-ended.md`: an agent moves an issue
//! assigned to the human only with the human's words and high confidence that they decide it.

use quadratique_server::domain::{
    Assignee, ClaimAfter, Confidence, Event, Failure, Outcome, decide, decide_on_word,
};

use super::support::{Board, agent, assigned, boards, claim_of, claimed, human, other_agent};

fn update_to(status: &str) -> Event {
    Event::Update {
        revision: 3,
        status: Some(status.into()),
        changes_assignee: false,
    }
}

fn assignee_only() -> Event {
    Event::Update {
        revision: 3,
        status: None,
        changes_assignee: true,
    }
}

/// An issue assigned to the human, unclaimed, in the unstarted status.
fn decision(board: &Board) -> quadratique_server::domain::Issue {
    assigned(board.issue(board.todo), Assignee::Human)
}

/// An issue that `agent()` holds in progress, then assigned to the human.
fn held_decision(board: &Board) -> quadratique_server::domain::Issue {
    assigned(
        claimed(board.issue(board.in_progress), claim_of(&agent())),
        Assignee::Human,
    )
}

#[test]
fn rule_4_with_high_confidence_an_agent_moves_an_issue_assigned_to_the_human() {
    for board in boards() {
        let moved = decide_on_word(
            &update_to(board.done),
            &decision(&board),
            &agent(),
            &board.project,
            Some(Confidence::High),
        );
        let Ok(Outcome::Changed(change)) = moved else {
            panic!("{}: {moved:?}", board.label);
        };
        assert_eq!(change.status, board.done, "{}", board.label);

        let released = decide_on_word(
            &Event::Release,
            &held_decision(&board),
            &agent(),
            &board.project,
            Some(Confidence::High),
        );
        let Ok(Outcome::Changed(change)) = released else {
            panic!("{}: {released:?}", board.label);
        };
        assert_eq!(change.status, board.todo, "{}", board.label);
        assert_eq!(change.claim, ClaimAfter::Ended, "{}", board.label);

        let closed = decide_on_word(
            &Event::Finish,
            &held_decision(&board),
            &agent(),
            &board.project,
            Some(Confidence::High),
        );
        assert!(
            matches!(closed, Ok(Outcome::Changed(_))),
            "{}: {closed:?}",
            board.label
        );

        assert_eq!(
            decide_on_word(
                &assignee_only(),
                &decision(&board),
                &agent(),
                &board.project,
                Some(Confidence::High),
            ),
            Ok(Outcome::Unchanged),
            "{}: the assignee changes, no status moves",
            board.label
        );
    }
}

#[test]
fn rule_4_with_medium_or_low_confidence_nothing_moves_and_the_issue_is_left_for_the_human() {
    for board in boards() {
        for confidence in [Confidence::Medium, Confidence::Low] {
            let calls = [
                (update_to(board.done), decision(&board)),
                (assignee_only(), decision(&board)),
                (Event::Release, held_decision(&board)),
                (Event::Finish, held_decision(&board)),
            ];
            for (event, issue) in calls {
                assert_eq!(
                    decide_on_word(&event, &issue, &agent(), &board.project, Some(confidence)),
                    Ok(Outcome::LeftForHuman),
                    "{}: {event:?} at {confidence:?}",
                    board.label
                );
            }
        }
    }
}

#[test]
fn rule_4_without_the_words_or_the_confidence_an_agent_gets_human_only() {
    for board in boards() {
        let calls = [
            (update_to(board.done), decision(&board)),
            (assignee_only(), decision(&board)),
            (Event::Release, held_decision(&board)),
            (Event::Finish, held_decision(&board)),
        ];
        for (event, issue) in calls {
            assert_eq!(
                decide_on_word(&event, &issue, &agent(), &board.project, None),
                Err(Failure::HumanOnly),
                "{}: {event:?}",
                board.label
            );
            assert_eq!(
                decide(&event, &issue, &agent(), &board.project),
                Err(Failure::HumanOnly),
                "{}: {event:?}",
                board.label
            );
        }
    }
}

#[test]
fn rule_4_stale_and_unknown_status_come_before_the_words() {
    for board in boards() {
        let stale = Event::Update {
            revision: 2,
            status: Some(board.done.into()),
            changes_assignee: false,
        };
        let unknown = update_to("deployed");
        for confidence in [Confidence::High, Confidence::Medium, Confidence::Low] {
            assert_eq!(
                decide_on_word(
                    &stale,
                    &decision(&board),
                    &agent(),
                    &board.project,
                    Some(confidence)
                ),
                Err(Failure::Stale { revision: 3 }),
                "{}",
                board.label
            );
            assert!(
                matches!(
                    decide_on_word(
                        &unknown,
                        &decision(&board),
                        &agent(),
                        &board.project,
                        Some(confidence)
                    ),
                    Err(Failure::UnknownStatus { .. })
                ),
                "{}",
                board.label
            );
        }
    }
}

#[test]
fn rule_4_with_high_confidence_the_rules_after_human_only_still_apply() {
    for board in boards() {
        let held_by_another = assigned(
            claimed(board.issue(board.in_progress), claim_of(&other_agent())),
            Assignee::Human,
        );
        for event in [update_to(board.in_review), Event::Finish, Event::Release] {
            assert!(
                matches!(
                    decide_on_word(
                        &event,
                        &held_by_another,
                        &agent(),
                        &board.project,
                        Some(Confidence::High)
                    ),
                    Err(Failure::Held { .. })
                ),
                "{}: {event:?}",
                board.label
            );
        }
        assert_eq!(
            decide_on_word(
                &Event::Finish,
                &decision(&board),
                &agent(),
                &board.project,
                Some(Confidence::High)
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
fn rule_4_the_humans_own_calls_ignore_the_words() {
    for board in boards() {
        for confidence in [None, Some(Confidence::Low)] {
            assert_eq!(
                decide_on_word(
                    &update_to(board.done),
                    &decision(&board),
                    &human(),
                    &board.project,
                    confidence
                ),
                decide(
                    &update_to(board.done),
                    &decision(&board),
                    &human(),
                    &board.project
                ),
                "{}",
                board.label
            );
        }
    }
}

#[test]
fn rule_4_at_medium_or_low_a_refusal_after_human_only_keeps_its_place() {
    for board in boards() {
        let held_by_another = assigned(
            claimed(board.issue(board.in_progress), claim_of(&other_agent())),
            Assignee::Human,
        );
        for confidence in [Confidence::Medium, Confidence::Low] {
            for event in [update_to(board.in_review), Event::Finish, Event::Release] {
                assert!(
                    matches!(
                        decide_on_word(
                            &event,
                            &held_by_another,
                            &agent(),
                            &board.project,
                            Some(confidence)
                        ),
                        Err(Failure::Held { .. })
                    ),
                    "{}: {event:?} at {confidence:?}",
                    board.label
                );
            }
            assert_eq!(
                decide_on_word(
                    &Event::Finish,
                    &decision(&board),
                    &agent(),
                    &board.project,
                    Some(confidence)
                ),
                Err(Failure::NotClaimed {
                    issue: "DEM-12".into()
                }),
                "{}",
                board.label
            );
        }
    }
}

#[test]
fn rule_4_on_an_issue_not_assigned_to_the_human_the_words_change_nothing() {
    for board in boards() {
        let held_by_another = claimed(board.issue(board.in_progress), claim_of(&other_agent()));
        let own = claimed(board.issue(board.in_progress), claim_of(&agent()));
        let unclaimed = board.issue(board.todo);
        for confidence in [Confidence::High, Confidence::Medium, Confidence::Low] {
            for (event, issue) in [
                (update_to(board.todo), &held_by_another),
                (Event::Release, &held_by_another),
                (Event::Finish, &unclaimed),
                (update_to(board.in_review), &own),
                (Event::Finish, &own),
            ] {
                assert_eq!(
                    decide_on_word(&event, issue, &agent(), &board.project, Some(confidence)),
                    decide(&event, issue, &agent(), &board.project),
                    "{}: {event:?} at {confidence:?}",
                    board.label
                );
            }
        }
    }
}
