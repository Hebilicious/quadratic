//! `B stacked-on A`: each test is named after the rule it proves
//! (`docs/decisions/0001-stacked-on.md`).

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::repository::{Repository, git};
use crate::support::{Running, Who, reasons, start};

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
