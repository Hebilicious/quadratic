//! `B stacked-on A`: each test is named after the rule it proves
//! (`docs/decisions/0001-stacked-on.md`).

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::repository::{Repository, git};
use quadratique_server::app::branch_checks_in;

use crate::support::{Running, Who, ids, reasons, start};

struct Stack {
    board: Running,
    repository: Repository,
    a: String,
    b: String,
    a_agent: Who,
    a_worktree: PathBuf,
}

impl Stack {
    async fn new() -> Stack {
        let board = start().await;
        let repository = Repository::new();
        board
            .ok(
                &repository.human(),
                "project.create",
                json!({ "key": "ABC", "name": "Project ABC", "defaultBranch": "main" }),
            )
            .await;
        let human = repository.human();
        let a = create(&board, &human, "Parse the headers").await;
        let b = create(&board, &human, "Cache the parsed headers").await;
        let a_worktree = repository.worktree("parse");
        let a_agent = repository.agent(&a_worktree, "parse");
        board.look().await;
        board
            .ok(&a_agent, "issue.claim", json!({ "issue": a }))
            .await;
        repository.commit(&a_worktree, "parser.rs", "Parse the headers");
        board.look().await;
        let stack = Stack {
            board,
            repository,
            a,
            b,
            a_agent,
            a_worktree,
        };
        stack
            .link("stacked-on", &stack.b, &stack.a)
            .await
            .expect("B is stacked on A");
        stack
    }

    fn human(&self) -> Who {
        self.repository.human()
    }

    async fn link(&self, kind: &str, issue: &str, target: &str) -> Result<Value, Value> {
        self.board
            .call(
                &self.human(),
                "issue.link",
                json!({ "issue": issue, "kind": kind, "target": target }),
            )
            .await
    }

    async fn agent_from(&self, branch: &str, start: &str) -> (Who, PathBuf) {
        let worktree = self.repository.worktree_from(branch, start);
        let agent = self.repository.agent(&worktree, branch);
        self.board.look().await;
        (agent, worktree)
    }

    async fn close_a(&self) {
        let closed = self
            .board
            .ok(
                &self.a_agent,
                "issue.close",
                json!({ "issue": self.a, "evidence": "the tests pass" }),
            )
            .await;
        assert_eq!(closed["issue"]["status"], "in-review", "{closed}");
    }

    async fn move_to(&self, who: &Who, issue: &str, status: &str) -> Result<Value, Value> {
        let revision = self.board.get(issue).await["revision"].clone();
        self.board
            .call(
                who,
                "issue.update",
                json!({ "issue": issue, "revision": revision, "status": status }),
            )
            .await
    }

    async fn summary_for(&self, who: &Who, issue: &str) -> Value {
        self.board
            .ok(who, "issue.get", json!({ "issue": issue }))
            .await["issue"]
            .clone()
    }

    async fn status(&self, issue: &str) -> String {
        self.board.get(issue).await["status"]
            .as_str()
            .expect("a status")
            .to_owned()
    }

    async fn restarted(self) -> Stack {
        let Stack {
            board,
            repository,
            a,
            b,
            a_agent,
            a_worktree,
        } = self;
        let (home, code) = board.stop().await;
        let board = Running::resume(home, code).await;
        Stack {
            board,
            repository,
            a,
            b,
            a_agent,
            a_worktree,
        }
    }

    /// B, unclaimed, in review, whose commit lands on main while a base is not merged.
    async fn hold_b(&self) -> String {
        self.move_to(&self.human(), &self.b, "in-review")
            .await
            .expect("B in review");
        let root = &self.repository.root;
        let landing =
            self.repository
                .commit(root, "cache.rs", &format!("Cache\n\nRefs: {}", self.b));
        self.board.look().await;
        assert_eq!(self.status(&self.b).await, "in-review", "B is held");
        landing
    }

    /// The changes that moved `issue` to `status`, with their store commits.
    async fn moves_to(&self, issue: &str, status: &str) -> Vec<Value> {
        let page = self
            .board
            .ok(&self.human(), "history.query", json!({ "issue": issue }))
            .await;
        let after = Value::from(json!(status).to_string());
        page["items"]
            .as_array()
            .expect("a list of changes")
            .iter()
            .filter(|change| {
                change["fields"]
                    .as_array()
                    .expect("fields")
                    .iter()
                    .any(|field| field["field"] == "status" && field["after"] == after)
            })
            .cloned()
            .collect()
    }

    fn squash_onto_main(&self, branch: &str, issue: &str) {
        let root = &self.repository.root;
        git(root, &["merge", "--quiet", "--squash", branch]);
        let message = format!("Land {branch}\n\nRefs: {issue}");
        git(
            root,
            &["commit", "--quiet", "--allow-empty", "--message", &message],
        );
    }
}

async fn create(board: &Running, who: &Who, title: &str) -> String {
    let created = board
        .ok(
            who,
            "issue.create",
            json!({ "acceptance": "It is done.", "title": title }),
        )
        .await;
    created["issue"]["id"].as_str().expect("an id").to_owned()
}

fn assert_stacked_on(summary: &Value, base: &str) {
    assert!(
        reasons(&summary["notReady"]).contains("stacked"),
        "{summary}"
    );
    assert_eq!(summary["blockedBy"], json!([base]), "{summary}");
    assert_eq!(summary["ready"], false, "{summary}");
}

#[tokio::test(flavor = "multi_thread")]
async fn b_is_ready_and_can_be_claimed_once_a_is_in_review_and_not_before() {
    let stack = Stack::new().await;
    let (agent, _) = stack.agent_from("cache", "parse").await;

    assert_stacked_on(&stack.summary_for(&agent, &stack.b).await, &stack.a);
    let refused = stack
        .board
        .failure(&agent, "issue.claim", json!({ "issue": stack.b }))
        .await;
    assert_eq!(refused["_tag"], "NotReady", "{refused}");
    assert_eq!(refused["reasons"], json!(["stacked"]), "{refused}");
    assert_eq!(refused["blockers"], json!([stack.a]), "{refused}");

    stack.close_a().await;

    assert_eq!(stack.summary_for(&agent, &stack.b).await["ready"], true);
    let claimed = stack
        .board
        .ok(&agent, "issue.claim", json!({ "issue": stack.b }))
        .await;
    assert_eq!(claimed["issue"]["status"], "in-progress", "{claimed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ready_offers_b_only_from_a_branch_or_a_branch_on_top_of_it() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let (on_top, _) = stack.agent_from("cache", "parse").await;
    let (unrelated, _) = stack.agent_from("docs", "main").await;
    let on_a_branch = stack.a_agent.clone();

    assert!(
        stack
            .board
            .ready_ids(&on_a_branch, 10)
            .await
            .contains(&stack.b)
    );
    assert!(stack.board.ready_ids(&on_top, 10).await.contains(&stack.b));
    assert!(
        !stack
            .board
            .ready_ids(&unrelated, 10)
            .await
            .contains(&stack.b)
    );
    assert_stacked_on(&stack.summary_for(&unrelated, &stack.b).await, &stack.a);
}

#[tokio::test(flavor = "multi_thread")]
async fn b_can_be_claimed_only_from_a_branch_that_contains_a_branch() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let (unrelated, _) = stack.agent_from("docs", "main").await;

    let refused = stack
        .board
        .failure(&unrelated, "issue.claim", json!({ "issue": stack.b }))
        .await;
    assert_eq!(refused["_tag"], "NotReady", "{refused}");
    assert_eq!(refused["reasons"], json!(["stacked"]), "{refused}");
    assert_eq!(refused["blockers"], json!([stack.a]), "{refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn b_is_offered_to_no_caller_while_a_branch_cannot_be_read() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let (on_top, _) = stack.agent_from("cache", "parse").await;
    git(
        &stack.repository.root,
        &[
            "worktree",
            "remove",
            "--force",
            &stack.a_worktree.display().to_string(),
        ],
    );
    git(
        &stack.repository.root,
        &["branch", "--quiet", "-D", "parse"],
    );

    assert!(!stack.board.ready_ids(&on_top, 10).await.contains(&stack.b));
    assert_stacked_on(&stack.summary_for(&on_top, &stack.b).await, &stack.a);
}

#[tokio::test(flavor = "multi_thread")]
async fn b_is_ready_for_any_caller_once_a_is_merged() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let (unrelated, _) = stack.agent_from("docs", "main").await;
    stack
        .move_to(&stack.human(), &stack.a, "merged")
        .await
        .expect("A merges");

    assert!(
        stack
            .board
            .ready_ids(&unrelated, 10)
            .await
            .contains(&stack.b)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn b_cannot_move_to_merged_or_later_before_a_has() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let human = stack.human();

    for status in ["merged", "done"] {
        let refused = stack
            .move_to(&human, &stack.b, status)
            .await
            .expect_err("B cannot pass A");
        assert_eq!(
            refused,
            json!({ "_tag": "BaseNotMerged", "issue": stack.b, "bases": [stack.a] })
        );
    }
    let refused = stack
        .board
        .failure(
            &human,
            "issue.close",
            json!({ "issue": stack.b, "evidence": "checked by hand" }),
        )
        .await;
    assert_eq!(refused["_tag"], "BaseNotMerged", "{refused}");
    stack
        .move_to(&human, &stack.b, "canceled")
        .await
        .expect("a stacked issue can still be canceled");
}

#[tokio::test(flavor = "multi_thread")]
async fn b_moves_to_merged_once_a_has() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let human = stack.human();
    stack
        .move_to(&human, &stack.a, "merged")
        .await
        .expect("A merges");

    let moved = stack.move_to(&human, &stack.b, "merged").await;
    assert!(moved.is_ok(), "{moved:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_landing_of_b_before_a_merges_holds_b_until_a_reaches_merged() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let (agent, worktree) = stack.agent_from("cache", "parse").await;
    stack
        .board
        .ok(&agent, "issue.claim", json!({ "issue": stack.b }))
        .await;
    stack
        .repository
        .commit(&worktree, "cache.rs", "Cache the parsed headers");
    stack.board.look().await;
    stack
        .board
        .ok(
            &agent,
            "issue.close",
            json!({ "issue": stack.b, "evidence": "the tests pass" }),
        )
        .await;

    stack.squash_onto_main("cache", &stack.b);
    stack.board.look().await;
    assert_eq!(stack.status(&stack.b).await, "in-review");
    assert_eq!(stack.status(&stack.a).await, "in-review");

    stack.squash_onto_main("parse", &stack.a);
    stack.board.look().await;
    assert_eq!(stack.status(&stack.a).await, "merged");
    assert_eq!(stack.status(&stack.b).await, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_landing_of_b_moves_once_the_link_is_removed() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let human = stack.human();
    stack
        .move_to(&human, &stack.b, "in-review")
        .await
        .expect("B in review");

    let root = &stack.repository.root;
    stack
        .repository
        .commit(root, "cache.rs", &format!("Cache\n\nRefs: {}", stack.b));
    stack.board.look().await;
    assert_eq!(stack.status(&stack.b).await, "in-review");

    stack
        .board
        .ok(
            &human,
            "issue.link",
            json!({ "issue": stack.b, "kind": "stacked-on", "target": stack.a, "remove": true }),
        )
        .await;
    assert_eq!(stack.status(&stack.b).await, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn when_a_goes_back_from_review_to_in_progress_b_keeps_its_claim_and_shows_a_reason_naming_a()
{
    let stack = Stack::new().await;
    stack.close_a().await;
    let (agent, _) = stack.agent_from("cache", "parse").await;
    stack
        .board
        .ok(&agent, "issue.claim", json!({ "issue": stack.b }))
        .await;

    stack
        .move_to(&stack.a_agent, &stack.a, "in-progress")
        .await
        .expect("A goes back");

    for who in [agent.clone(), stack.human()] {
        let shown = stack.summary_for(&who, &stack.b).await;
        assert_eq!(shown["status"], "in-progress", "{shown}");
        assert_eq!(shown["claim"]["branch"], "cache", "{shown}");
        assert_stacked_on(&shown, &stack.a);
    }
    let listed = stack
        .board
        .ok(&agent, "issue.list", json!({ "claim": "mine" }))
        .await;
    assert_stacked_on(&listed["items"][0], &stack.a);

    stack.close_a().await;
    let shown = stack.summary_for(&agent, &stack.b).await;
    assert!(!reasons(&shown["notReady"]).contains("stacked"), "{shown}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_canceled_base_keeps_b_not_ready_until_the_link_is_removed() {
    let stack = Stack::new().await;
    let human = stack.human();
    stack
        .move_to(&human, &stack.a, "canceled")
        .await
        .expect("A is canceled");

    assert_stacked_on(&stack.summary_for(&human, &stack.b).await, &stack.a);
    stack
        .board
        .ok(
            &human,
            "issue.link",
            json!({ "issue": stack.b, "kind": "stacked-on", "target": stack.a, "remove": true }),
        )
        .await;
    assert_eq!(stack.summary_for(&human, &stack.b).await["ready"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn stacked_on_does_not_make_b_wait_for_a_to_be_resolved() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let shown = stack.summary_for(&stack.a_agent, &stack.b).await;
    assert_eq!(stack.board.get(&stack.a).await["category"], "started");
    assert_eq!(shown["ready"], true, "{shown}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cycle_through_stacked_on_and_blocks_together_fails_with_cycle() {
    let stack = Stack::new().await;
    let c = create(&stack.board, &stack.human(), "Serve the cache").await;
    stack
        .link("blocks", &stack.b, &c)
        .await
        .expect("B blocks C");

    let refused = stack
        .link("stacked-on", &stack.a, &c)
        .await
        .expect_err("A stacked on C closes a loop");
    assert_eq!(refused["_tag"], "Cycle", "{refused}");
    let refused = stack
        .link("blocks", &c, &stack.a)
        .await
        .expect_err("C blocks A closes a loop");
    assert_eq!(refused["_tag"], "Cycle", "{refused}");
    let refused = stack
        .link("stacked-on", &stack.a, &stack.b)
        .await
        .expect_err("A stacked on B closes a loop");
    assert_eq!(refused["_tag"], "Cycle", "{refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_issue_stacked_on_two_issues_waits_for_both() {
    let stack = Stack::new().await;
    let human = stack.human();
    let d = create(&stack.board, &human, "Log the parsed headers").await;
    stack
        .link("stacked-on", &stack.b, &d)
        .await
        .expect("a second base");
    stack.close_a().await;
    let on_a = stack.a_agent.clone();

    assert_stacked_on(&stack.summary_for(&on_a, &stack.b).await, &d);
    stack
        .move_to(&human, &d, "in-review")
        .await
        .expect("D in review");
    assert_stacked_on(&stack.summary_for(&on_a, &stack.b).await, &d);
    stack.move_to(&human, &d, "merged").await.expect("D merges");
    assert_eq!(stack.summary_for(&on_a, &stack.b).await["ready"], true);
    let refused = stack
        .move_to(&human, &stack.b, "merged")
        .await
        .expect_err("A is not merged");
    assert_eq!(refused["bases"], json!([stack.a]), "{refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_landing_moves_once_every_base_is_merged_in_the_last_base_commit() {
    let stack = Stack::new().await;
    let human = stack.human();
    let d = create(&stack.board, &human, "Log the parsed headers").await;
    stack
        .link("stacked-on", &stack.b, &d)
        .await
        .expect("a second base");
    stack.close_a().await;
    stack.hold_b().await;

    stack.move_to(&human, &d, "merged").await.expect("D merges");
    assert_eq!(stack.status(&stack.b).await, "in-review", "A still holds B");

    let stack = stack.restarted().await;
    stack
        .move_to(&stack.human(), &stack.a, "merged")
        .await
        .expect("A merges");

    assert_eq!(stack.status(&stack.b).await, "merged");
    let moves = stack.moves_to(&stack.b, "merged").await;
    assert_eq!(moves.len(), 1, "{moves:?}");
    let a_moves = stack.moves_to(&stack.a, "merged").await;
    assert_eq!(
        moves[0]["commit"], a_moves[0]["commit"],
        "{moves:?} {a_moves:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn advance_leaves_a_held_landing_in_left() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let landing = stack.hold_b().await;

    let advanced = stack
        .board
        .ok(
            &stack.human(),
            "issue.advance",
            json!({ "project": "ABC", "commit": landing, "status": "done" }),
        )
        .await;
    assert_eq!(advanced["moved"]["issues"], json!([]), "{advanced}");
    assert_eq!(advanced["left"]["issues"], json!([stack.b]), "{advanced}");
    assert_eq!(stack.status(&stack.b).await, "in-review");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hold_ends_when_its_issue_is_canceled_and_a_new_claim_never_lands_with_it() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let human = stack.human();
    let (agent, worktree) = stack.agent_from("cache", "parse").await;
    stack
        .board
        .ok(&agent, "issue.claim", json!({ "issue": stack.b }))
        .await;
    stack
        .repository
        .commit(&worktree, "cache.rs", "Cache the parsed headers");
    stack.board.look().await;
    stack
        .board
        .ok(
            &agent,
            "issue.close",
            json!({ "issue": stack.b, "evidence": "the tests pass" }),
        )
        .await;
    stack.squash_onto_main("cache", &stack.b);
    stack.board.look().await;
    assert_eq!(stack.status(&stack.b).await, "in-review", "B is held");

    stack
        .move_to(&human, &stack.b, "canceled")
        .await
        .expect("B is canceled");
    stack
        .move_to(&human, &stack.b, "todo")
        .await
        .expect("B reopens");
    let (again, _) = stack.agent_from("cache-again", "parse").await;
    stack
        .board
        .ok(&again, "issue.claim", json!({ "issue": stack.b }))
        .await;
    stack
        .move_to(&human, &stack.a, "merged")
        .await
        .expect("A merges");

    let shown = stack.board.get(&stack.b).await;
    assert_eq!(shown["status"], "in-progress", "{shown}");
    assert_eq!(shown["claim"]["branch"], "cache-again", "{shown}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hold_ends_when_its_claim_is_released() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let human = stack.human();
    let (agent, worktree) = stack.agent_from("cache", "parse").await;
    stack
        .board
        .ok(&agent, "issue.claim", json!({ "issue": stack.b }))
        .await;
    stack
        .repository
        .commit(&worktree, "cache.rs", "Cache the parsed headers");
    stack.board.look().await;
    stack
        .board
        .ok(
            &agent,
            "issue.close",
            json!({ "issue": stack.b, "evidence": "the tests pass" }),
        )
        .await;
    stack.squash_onto_main("cache", &stack.b);
    stack.board.look().await;
    assert_eq!(stack.status(&stack.b).await, "in-review", "B is held");

    stack
        .board
        .ok(&agent, "issue.release", json!({ "issue": stack.b }))
        .await;
    stack
        .move_to(&human, &stack.a, "merged")
        .await
        .expect("A merges");

    assert_eq!(stack.status(&stack.b).await, "todo");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_of_the_same_name_in_another_repository_does_not_satisfy_the_branch_rule() {
    let stack = Stack::new().await;
    stack.close_a().await;
    let other = Repository::new();
    let worktree = other.worktree("parse");
    other.commit(&worktree, "parser.rs", "Parse the headers elsewhere");
    let stranger = other.agent(&worktree, "parse");

    let ready = stack
        .board
        .ok(
            &stranger,
            "issue.ready",
            json!({ "project": "ABC", "limit": 10 }),
        )
        .await;
    assert!(!ids(&ready["items"]).contains(&stack.b), "{ready}");
    let refused = stack
        .board
        .failure(&stranger, "issue.claim", json!({ "issue": stack.b }))
        .await;
    assert_eq!(refused["_tag"], "NotReady", "{refused}");
    assert_eq!(refused["reasons"], json!(["stacked"]), "{refused}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_branch_rule_starts_git_only_for_a_decision_about_a_base_in_review() {
    let board = start().await;
    let repository = Repository::new();
    let human = repository.human();
    board
        .ok(
            &human,
            "project.create",
            json!({ "key": "ABC", "name": "Project ABC", "defaultBranch": "main" }),
        )
        .await;
    let a = create(&board, &human, "Parse the headers").await;
    let b = create(&board, &human, "Cache the parsed headers").await;
    let worktree = repository.worktree("parse");
    let agent = repository.agent(&worktree, "parse");
    board.look().await;
    let checks = || branch_checks_in(&repository.common_dir());

    board.ok(&agent, "project.list", json!({})).await;
    board.ok(&agent, "issue.list", json!({})).await;
    board.ok(&agent, "issue.ready", json!({})).await;
    board.ok(&agent, "issue.get", json!({ "issue": b })).await;
    board.ok(&agent, "issue.claim", json!({ "issue": a })).await;
    assert_eq!(checks(), 0, "a board without stacked-on asks git nothing");

    board
        .ok(
            &human,
            "issue.link",
            json!({ "issue": b, "kind": "stacked-on", "target": a }),
        )
        .await;
    board.ok(&agent, "issue.ready", json!({})).await;
    assert_eq!(checks(), 0, "a base before review asks git nothing");
    repository.commit(&worktree, "parser.rs", "Parse the headers");
    board
        .ok(
            &agent,
            "issue.close",
            json!({ "issue": a, "evidence": "done" }),
        )
        .await;
    assert!(board.ready_ids(&agent, 10).await.contains(&b));
    assert_eq!(
        checks(),
        1,
        "a base in review asks git once for the caller's branch"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stacked_on_link_from_an_issue_at_merge_to_an_unmerged_base_is_refused() {
    let stack = Stack::new().await;
    let human = stack.human();
    let c = create(&stack.board, &human, "Document the cache").await;
    for status in ["merged", "done"] {
        stack.move_to(&human, &c, status).await.expect("C moves");
        let refused = stack
            .link("stacked-on", &c, &stack.a)
            .await
            .expect_err("C cannot be stacked on an unmerged A");
        assert_eq!(
            refused,
            json!({ "_tag": "BaseNotMerged", "issue": c, "bases": [stack.a] })
        );
    }
    stack
        .move_to(&human, &stack.a, "merged")
        .await
        .expect("A merges");
    stack
        .link("stacked-on", &c, &stack.a)
        .await
        .expect("a merged base may be linked");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_added_to_a_claimed_issue_shows_stacked_only_while_the_base_is_before_review() {
    let stack = Stack::new().await;
    let human = stack.human();
    let c = create(&stack.board, &human, "Document the cache").await;
    let (unrelated, _) = stack.agent_from("docs", "main").await;
    stack
        .board
        .ok(&unrelated, "issue.claim", json!({ "issue": c }))
        .await;

    stack
        .link("stacked-on", &c, &stack.a)
        .await
        .expect("C on A");
    assert_stacked_on(&stack.summary_for(&unrelated, &c).await, &stack.a);
    stack.close_a().await;
    let shown = stack.summary_for(&unrelated, &c).await;
    assert!(!reasons(&shown["notReady"]).contains("stacked"), "{shown}");
}

fn workflow(order: &[&str]) -> Value {
    let category = |key: &str| match key {
        "backlog" => "backlog",
        "todo" => "unstarted",
        "done" => "completed",
        "canceled" => "canceled",
        _ => "started",
    };
    let statuses: Vec<Value> = order
        .iter()
        .map(|key| json!({ "key": key, "name": key, "category": category(key) }))
        .collect();
    json!({
        "statuses": statuses,
        "moves": { "claim": "in-progress", "review": "in-review", "merge": "merged", "release": "todo" },
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_that_puts_a_stacked_issue_at_merge_ahead_of_its_base_is_refused() {
    let stack = Stack::new().await;
    let human = stack.human();
    let c = create(&stack.board, &human, "Name the headers").await;
    let d = create(&stack.board, &human, "Index the named headers").await;
    stack.link("stacked-on", &d, &c).await.expect("D on C");
    stack
        .move_to(&human, &d, "in-progress")
        .await
        .expect("D starts");

    let refused = stack
        .board
        .failure(
            &human,
            "project.update",
            json!({
                "project": "ABC",
                "workflow": workflow(&["backlog", "todo", "merged", "in-progress", "in-review", "done", "canceled"]),
            }),
        )
        .await;
    assert_eq!(
        refused,
        json!({ "_tag": "MergedBeforeBase", "issues": [d] })
    );
    let project = stack.board.ok(&human, "project.list", json!({})).await;
    assert_eq!(
        project["items"][0]["workflow"]["statuses"][2]["key"],
        "in-progress"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_change_that_makes_every_base_merged_releases_a_held_landing() {
    let stack = Stack::new().await;
    stack.close_a().await;
    stack.hold_b().await;

    stack
        .board
        .ok(
            &stack.human(),
            "project.update",
            json!({
                "project": "ABC",
                "workflow": workflow(&["backlog", "todo", "in-progress", "merged", "in-review", "done", "canceled"]),
            }),
        )
        .await;
    assert_eq!(stack.status(&stack.b).await, "merged");
}

async fn import(stack: &Stack, findings: Value) -> Value {
    stack
        .board
        .ok(
            &stack.human(),
            "findings.import",
            json!({ "tool": "linter", "findings": findings }),
        )
        .await
}

fn finding(key: &str) -> Value {
    json!({ "key": key, "title": format!("Finding {key}"), "description": "As reported." })
}

#[tokio::test(flavor = "multi_thread")]
async fn an_import_does_not_finish_a_finding_stacked_on_an_unmerged_base_and_goes_on() {
    let stack = Stack::new().await;
    let first = import(&stack, json!([finding("stacked"), finding("plain")])).await;
    let stacked = first["created"]["issues"][0]
        .as_str()
        .expect("an id")
        .to_owned();
    let plain = first["created"]["issues"][1]
        .as_str()
        .expect("an id")
        .to_owned();
    stack
        .link("stacked-on", &stacked, &stack.a)
        .await
        .expect("the finding is stacked on A");

    let second = import(&stack, json!([])).await;
    assert_eq!(second["finished"]["issues"], json!([plain]), "{second}");
    assert_eq!(second["left"]["issues"], json!([stacked]), "{second}");
    assert_eq!(stack.status(&stacked).await, "todo");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_import_that_finishes_a_base_releases_its_held_landing() {
    let stack = Stack::new().await;
    let first = import(&stack, json!([finding("base")])).await;
    let base = first["created"]["issues"][0]
        .as_str()
        .expect("an id")
        .to_owned();
    stack
        .board
        .ok(
            &stack.human(),
            "issue.link",
            json!({ "issue": stack.b, "kind": "stacked-on", "target": stack.a, "remove": true }),
        )
        .await;
    stack
        .link("stacked-on", &stack.b, &base)
        .await
        .expect("B on the finding");
    stack.hold_b().await;

    let second = import(&stack, json!([])).await;
    assert_eq!(second["finished"]["issues"], json!([base]), "{second}");
    assert_eq!(stack.status(&stack.b).await, "merged");
}

/// A merged, then B merged, then A moved back before the `merge` status by hand.
async fn out_of_order(stack: &Stack) {
    let human = stack.human();
    stack.close_a().await;
    stack
        .move_to(&human, &stack.a, "merged")
        .await
        .expect("A merges");
    stack
        .move_to(&human, &stack.b, "merged")
        .await
        .expect("B merges");
    stack
        .move_to(&human, &stack.a, "in-review")
        .await
        .expect("A goes back");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_change_is_refused_only_for_an_order_it_breaks() {
    let stack = Stack::new().await;
    out_of_order(&stack).await;
    let human = stack.human();
    let elsewhere = Who {
        repository: None,
        worktree: None,
        branch: None,
        ..human.clone()
    };
    stack
        .board
        .ok(
            &elsewhere,
            "project.create",
            json!({ "key": "XYZ", "name": "Project XYZ" }),
        )
        .await;
    let order = [
        "backlog",
        "todo",
        "in-progress",
        "in-review",
        "merged",
        "done",
        "canceled",
    ];

    for project in ["XYZ", "ABC"] {
        let updated = stack
            .board
            .call(
                &human,
                "project.update",
                json!({ "project": project, "workflow": workflow(&order) }),
            )
            .await;
        assert!(updated.is_ok(), "{project}: {updated:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn adding_a_link_that_exists_changes_nothing_and_succeeds() {
    let stack = Stack::new().await;
    out_of_order(&stack).await;

    let repeated = stack
        .link("stacked-on", &stack.b, &stack.a)
        .await
        .expect("the link exists");
    assert_eq!(
        repeated["relations"].as_array().map(Vec::len),
        Some(1),
        "{repeated}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_base_branch_name_with_a_revision_suffix_gives_no_evidence() {
    let stack = Stack::new().await;
    let human = stack.human();
    let c = create(&stack.board, &human, "Name the headers").await;
    let d = create(&stack.board, &human, "Index the named headers").await;
    stack.link("stacked-on", &d, &c).await.expect("D on C");
    let suffixed = Who {
        branch: Some("main^{commit}".into()),
        worktree: Some(stack.repository.root.join("suffixed").display().to_string()),
        ..stack.repository.agent(&stack.repository.root, "suffixed")
    };
    stack
        .board
        .ok(&suffixed, "issue.claim", json!({ "issue": c }))
        .await;
    stack
        .board
        .ok(
            &suffixed,
            "issue.close",
            json!({ "issue": c, "evidence": "done" }),
        )
        .await;
    let (on_main, _) = stack.agent_from("docs", "main").await;

    assert!(!stack.board.ready_ids(&on_main, 10).await.contains(&d));
    let refused = stack
        .board
        .failure(&on_main, "issue.claim", json!({ "issue": d }))
        .await;
    assert_eq!(refused["reasons"], json!(["stacked"]), "{refused}");
}
