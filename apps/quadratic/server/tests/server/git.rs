use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use acid_cat_core::http::HttpClient;
use quadratic_server::git::repository::Repository as GitRepository;
use quadratic_server::git::watch::{Reached, step};
use quadratic_server::store::GitWatch;
use quadratic_server::web::WEB_BASE;
use serde_json::{Value, json};

use crate::repository::{Repository, git, has_object};
use crate::support::{HANG_GUARD, Running, Who, call, start, start_looking_every, within};

struct Workspace {
    board: Running,
    repository: Repository,
}

impl Workspace {
    async fn new(key: &str) -> Workspace {
        Workspace::on(start().await, key).await
    }

    async fn looking_only_when_asked(key: &str) -> Workspace {
        Workspace::on(start_looking_every(HANG_GUARD * 10).await, key).await
    }

    async fn on(board: Running, key: &str) -> Workspace {
        let repository = Repository::new();
        bind(&board, &repository, key).await;
        Workspace { board, repository }
    }

    async fn issue_in_review(&self, title: &str) -> String {
        let created = self
            .board
            .ok(
                &self.repository.human(),
                "issue.create",
                json!({ "acceptance": "It is done.", "title": title, "status": "in-review" }),
            )
            .await;
        created["issue"]["id"].as_str().expect("an id").to_owned()
    }

    async fn status_on_the_web_page(&self, issue: &str) -> String {
        let origin = format!("http://{}", self.board.server.address());
        let page = HttpClient::new(&format!("{origin}{WEB_BASE}"))
            .expect("the web route's URL")
            .header("Origin", &origin)
            .expect("a valid header");
        let shown = call(&page, "issue.get", json!({ "issue": issue }))
            .await
            .unwrap_or_else(|failure| panic!("issue.get failed: {failure}"));
        shown["issue"]["status"]
            .as_str()
            .expect("a status")
            .to_owned()
    }

    async fn issue(&self, title: &str) -> String {
        let created = self
            .board
            .ok(
                &self.repository.human(),
                "issue.create",
                json!({ "acceptance": "It is done.", "title": title }),
            )
            .await;
        created["issue"]["id"].as_str().expect("an id").to_owned()
    }

    async fn agent_on(&self, branch: &str) -> (Who, PathBuf) {
        let worktree = self.repository.worktree(branch);
        let agent = self.repository.agent(&worktree, branch);
        self.board.look().await;
        (agent, worktree)
    }

    async fn claim(&self, agent: &Who, issue: &str) {
        self.board
            .ok(agent, "issue.claim", json!({ "issue": issue }))
            .await;
    }

    async fn close(&self, agent: &Who, issue: &str) {
        let closed = self
            .board
            .ok(
                agent,
                "issue.close",
                json!({ "issue": issue, "evidence": "the tests pass" }),
            )
            .await;
        assert_eq!(closed["issue"]["status"], "in-review", "{closed}");
    }

    async fn squash_with_refs(&self, branch: &str, issue: &str) {
        let root = &self.repository.root;
        git(root, &["merge", "--quiet", "--squash", branch]);
        let message = format!("Parse the headers (#7)\n\nRefs: {issue}");
        git(root, &["commit", "--quiet", "--message", &message]);
    }

    async fn status(&self, issue: &str) -> (String, Value) {
        let shown = self.board.get(issue).await;
        (
            shown["status"].as_str().expect("a status").to_owned(),
            shown.get("claim").cloned().unwrap_or(Value::Null),
        )
    }

    async fn merges(&self, issue: &str) -> Vec<Value> {
        history(&self.board, &self.repository.human(), issue).await
    }
}

async fn bind(board: &Running, repository: &Repository, key: &str) {
    board
        .ok(
            &repository.human(),
            "project.create",
            json!({ "key": key, "name": format!("Project {key}"), "defaultBranch": "main" }),
        )
        .await;
}

async fn history(board: &Running, who: &Who, issue: &str) -> Vec<Value> {
    let page = board
        .ok(who, "history.query", json!({ "issue": issue }))
        .await;
    page["items"]
        .as_array()
        .expect("a list of changes")
        .iter()
        .filter(|change| change["operation"] == "git.merged")
        .cloned()
        .collect()
}

fn assert_by_the_server(change: &Value) {
    assert_eq!(change["author"]["name"], "quadratic", "{change}");
    assert_eq!(change["author"]["harness"], "quadratic", "{change}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_merged_with_no_ff_moves_its_claimed_issue_from_review_to_merged_and_ends_the_claim()
 {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    let (agent, worktree) = space.agent_on("agent-a").await;
    space.claim(&agent, &issue).await;
    let tip = space
        .repository
        .commit(&worktree, "parser.rs", "Parse the headers");
    space.board.look().await;
    space.close(&agent, &issue).await;

    let merge = space
        .repository
        .merge(&["--no-ff", "--message", "Merge agent-a", "agent-a"]);
    space.board.look().await;

    let (status, claim) = space.status(&issue).await;
    assert_eq!(status, "merged");
    assert_eq!(claim, Value::Null);
    let merges = space.merges(&issue).await;
    assert_eq!(merges.len(), 1, "{merges:?}");
    let change = &merges[0];
    assert_by_the_server(change);
    assert_eq!(change["branch"], "agent-a");
    let reason = change["reason"].as_str().expect("a reason");
    assert!(reason.contains(&tip), "{reason}");
    assert!(reason.contains(&merge), "{reason}");
    let fields: Vec<&Value> = change["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| &field["field"])
        .collect();
    assert!(fields.contains(&&json!("status")), "{change}");
    assert!(fields.contains(&&json!("claim")), "{change}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_read_of_an_issue_after_its_merge_answers_merged() {
    let space = Workspace::looking_only_when_asked("ABC").await;
    let issue = space.issue_in_review("Parse the headers").await;
    space.board.look().await;

    let root = &space.repository.root;
    space.repository.commit(
        root,
        "parser.rs",
        &format!("Parse the headers\n\nRefs: {issue}"),
    );

    assert_eq!(space.status(&issue).await.0, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_from_the_web_page_answers_from_the_store_without_a_look() {
    let space = Workspace::looking_only_when_asked("ABC").await;
    let issue = space.issue_in_review("Parse the headers").await;
    space.board.look().await;

    let root = &space.repository.root;
    space.repository.commit(
        root,
        "parser.rs",
        &format!("Parse the headers\n\nRefs: {issue}"),
    );
    assert_eq!(space.status_on_the_web_page(&issue).await, "in-review");

    space.board.look().await;
    assert_eq!(space.status_on_the_web_page(&issue).await, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_squash_merge_settles_the_issue_its_refs_trailer_names_and_nothing_without_one() {
    let space = Workspace::new("ABC").await;
    let named = space.issue("Parse the headers").await;
    let unnamed = space.issue("Parse the body").await;
    let (first, first_worktree) = space.agent_on("agent-a").await;
    let (second, second_worktree) = space.agent_on("agent-b").await;
    space.claim(&first, &named).await;
    space.claim(&second, &unnamed).await;
    space
        .repository
        .commit(&first_worktree, "headers.rs", "Parse the headers");
    space
        .repository
        .commit(&second_worktree, "body.rs", "Parse the body");
    space.board.look().await;
    space.close(&first, &named).await;
    space.close(&second, &unnamed).await;

    let root = &space.repository.root;
    git(root, &["merge", "--quiet", "--squash", "agent-a"]);
    let message = format!("Parse the headers\n\nSquashed from agent-a.\n\nRefs: {named}");
    git(root, &["commit", "--quiet", "--message", &message]);
    git(root, &["merge", "--quiet", "--squash", "agent-b"]);
    git(root, &["commit", "--quiet", "--message", "Parse the body"]);
    let squash = space.repository.tip(root, "HEAD~1");
    space.board.look().await;

    assert_eq!(space.status(&named).await, ("merged".into(), Value::Null));
    let merges = space.merges(&named).await;
    assert_eq!(merges.len(), 1, "{merges:?}");
    assert_by_the_server(&merges[0]);
    let reason = merges[0]["reason"].as_str().expect("a reason");
    assert!(reason.contains(&squash), "{reason}");

    let (status, claim) = space.status(&unnamed).await;
    assert_eq!(status, "in-review");
    assert_eq!(claim["branch"], "agent-b");
    assert_eq!(space.merges(&unnamed).await, Vec::<Value>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_github_squash_settles_the_issue_named_by_a_refs_line_in_any_paragraph() {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    let (agent, worktree) = space.agent_on("agent-a").await;
    space.claim(&agent, &issue).await;
    space
        .repository
        .commit(&worktree, "headers.rs", "Parse the headers");
    space.board.look().await;
    space.close(&agent, &issue).await;

    let root = &space.repository.root;
    git(root, &["merge", "--quiet", "--squash", "agent-a"]);
    let message = format!(
        "Parse the headers (#7)\n\n* feat: parse the headers\n\nRefs: {issue}\n\n* fix: a header may be empty"
    );
    git(root, &["commit", "--quiet", "--message", &message]);
    space.board.look().await;

    assert_eq!(space.status(&issue).await, ("merged".into(), Value::Null));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_that_is_not_utf8_still_settles_its_refs_and_the_commits_after_it() {
    let space = Workspace::new("ABC").await;
    let latin = space.issue("Parse the Latin-1 names").await;
    let later = space.issue("Parse the body").await;
    space.board.look().await;

    let mut message = b"Parse the names of Jos\xe9\n\nRefs: ".to_vec();
    message.extend_from_slice(latin.as_bytes());
    space.repository.commit_raw_message_on_main(&message);
    space.repository.commit(
        &space.repository.root,
        "body.rs",
        &format!("Parse the body\n\nRefs: {later}"),
    );
    space.board.look().await;

    assert_eq!(space.status(&latin).await.0, "merged");
    assert_eq!(space.status(&later).await.0, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_merged_branch_named_after_an_unclaimed_issue_settles_that_issue_alone() {
    let space = Workspace::new("ABC").await;
    let first = space.issue("Parse the headers").await;
    let other = space.issue("Not this one").await;
    let branch = format!("{}-parse-headers", first.to_lowercase());
    let (_, worktree) = space.agent_on(&branch).await;
    space
        .repository
        .commit(&worktree, "headers.rs", "Parse the headers");
    space.board.look().await;

    space
        .repository
        .merge(&["--no-ff", "--message", "Merge", &branch]);
    space.board.look().await;

    assert_eq!(space.status(&first).await, ("merged".into(), Value::Null));
    let merges = space.merges(&first).await;
    assert_eq!(merges.len(), 1, "{merges:?}");
    assert_eq!(merges[0]["branch"], branch.as_str());
    assert_eq!(space.status(&other).await.0, "todo");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_with_no_commit_of_its_own_settles_nothing_and_a_claim_on_it_stays() {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    let branch = format!("{}-parse-headers", issue.to_lowercase());
    let (agent, worktree) = space.agent_on(&branch).await;
    space.claim(&agent, &issue).await;

    space
        .repository
        .commit(&space.repository.root, "other.rs", "Unrelated work on main");
    space.board.look().await;
    git(&worktree, &["merge", "--quiet", "--ff-only", "main"]);
    space.board.look().await;
    space
        .repository
        .commit(&space.repository.root, "more.rs", "More work on main");
    space.board.look().await;

    let (status, claim) = space.status(&issue).await;
    assert_eq!(status, "in-progress");
    assert_eq!(claim["branch"], branch.as_str());
    assert_eq!(space.merges(&issue).await, Vec::<Value>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_landing_while_the_server_is_down_is_settled_at_start_and_never_twice() {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    let (agent, worktree) = space.agent_on("agent-a").await;
    space.claim(&agent, &issue).await;
    space
        .repository
        .commit(&worktree, "parser.rs", "Parse the headers");
    space.board.look().await;
    space.close(&agent, &issue).await;
    let Workspace { board, repository } = space;
    let (home, code) = board.stop().await;

    repository.merge(&["--no-ff", "--message", "Merge agent-a", "agent-a"]);
    let board = Running::resume(home, code).await;
    let space = Workspace { board, repository };
    within("the settlement after the start", async {
        while space.status(&issue).await != ("merged".into(), Value::Null) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert_eq!(space.merges(&issue).await.len(), 1);

    let revision = space.board.get(&issue).await["revision"].clone();
    space
        .board
        .ok(
            &space.repository.human(),
            "issue.update",
            json!({ "issue": issue, "revision": revision, "status": "done" }),
        )
        .await;
    let Workspace { board, repository } = space;
    let board = board.restart().await;
    board.look().await;
    let space = Workspace { board, repository };
    assert_eq!(space.status(&issue).await.0, "done");
    assert_eq!(space.merges(&issue).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_landing_that_moves_nothing_is_still_recorded_in_the_history() {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    let revision = space.board.get(&issue).await["revision"].clone();
    space
        .board
        .ok(
            &space.repository.human(),
            "issue.update",
            json!({ "issue": issue, "revision": revision, "status": "done" }),
        )
        .await;
    space.board.look().await;

    let message = format!("Fix a typo in the parser\n\nRefs: {issue}");
    let commit = space
        .repository
        .commit(&space.repository.root, "typo.rs", &message);
    space.board.look().await;

    assert_eq!(space.status(&issue).await.0, "done");
    let merges = space.merges(&issue).await;
    assert_eq!(merges.len(), 1, "{merges:?}");
    assert_by_the_server(&merges[0]);
    assert_eq!(merges[0]["fields"], json!([]));
    assert!(merges[0]["reason"].as_str().unwrap().contains(&commit));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_look_never_fetches_an_object_that_a_partial_clone_lacks() {
    let source = Repository::new();
    let known = source.tip(&source.root, "HEAD");
    let missing = source.commit(&source.root, "parser.rs", "Work only the promisor has");
    let directory = tempfile::tempdir().expect("a temporary directory");
    let partial = directory.path().join("partial.git");
    git(
        directory.path(),
        &["init", "--quiet", "--bare", "partial.git"],
    );
    git(
        &source.root,
        &[
            "push",
            "--quiet",
            &partial.display().to_string(),
            &format!("{known}:refs/heads/main"),
        ],
    );
    let promisor = source.common_dir();
    for (key, value) in [
        ("core.repositoryformatversion", "1"),
        ("remote.origin.url", promisor.as_str()),
        ("remote.origin.promisor", "true"),
        ("extensions.partialClone", "origin"),
    ] {
        git(&partial, &["config", key, value]);
    }
    std::fs::write(partial.join("refs/heads/work"), format!("{missing}\n"))
        .expect("the ref file is written");

    let _ = GitRepository::at(&partial.display().to_string(), HANG_GUARD)
        .look("main", None)
        .await;

    assert!(!has_object(&partial, &missing));
}

#[tokio::test]
async fn a_branch_is_reached_only_at_the_hash_that_was_read_for_it() {
    let repository = Repository::new();
    let main = repository.tip(&repository.root, "main");
    let unlanded = repository.commit_beside("main", "Work that never landed");
    git(&repository.root, &["branch", "agent-a", &main]);
    let read_before_it_moved = BTreeMap::from([
        ("main".to_owned(), main.clone()),
        ("agent-a".to_owned(), unlanded),
    ]);

    let outside = GitRepository::at(&repository.common_dir(), HANG_GUARD)
        .outside(&main, &read_before_it_moved)
        .await
        .expect("git answers");

    assert_eq!(outside, BTreeSet::from(["agent-a".to_owned()]));
}

#[tokio::test]
async fn an_upstream_that_is_a_local_branch_is_no_landing_ref() {
    let repository = Repository::new();
    git(&repository.root, &["branch", "integration"]);
    git(
        &repository.root,
        &["branch", "--set-upstream-to=integration", "main"],
    );

    let seen = GitRepository::at(&repository.common_dir(), HANG_GUARD)
        .look("main", None)
        .await
        .expect("git answers")
        .expect("main exists");

    assert_eq!(seen.upstream, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claimed_branch_reset_onto_the_default_branch_has_not_landed() {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    let (agent, worktree) = space.agent_on("agent-a").await;
    space.claim(&agent, &issue).await;
    space
        .repository
        .commit(&worktree, "parser.rs", "Parse the headers");
    space.board.look().await;

    git(&worktree, &["reset", "--quiet", "--hard", "main"]);
    space.board.look().await;

    assert_eq!(space.status(&issue).await.0, "in-progress");
    assert_eq!(space.merges(&issue).await, Vec::<Value>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claimed_branch_that_moved_on_has_not_landed_when_only_its_older_tip_does() {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    let (agent, worktree) = space.agent_on("agent-a").await;
    space.claim(&agent, &issue).await;
    let older = space
        .repository
        .commit(&worktree, "parser.rs", "Parse the headers");
    space.board.look().await;

    space
        .repository
        .commit(&worktree, "trailers.rs", "Parse the trailers");
    space
        .repository
        .merge(&["--no-ff", "--message", "Merge the older tip", &older]);
    space.board.look().await;

    let (status, claim) = space.status(&issue).await;
    assert_eq!(status, "in-progress");
    assert_eq!(claim["branch"], "agent-a");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_made_from_another_repository_is_not_ended_by_a_branch_of_the_same_name() {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    let elsewhere = Repository::new();
    let other_worktree = elsewhere.worktree("agent-a");
    let agent = elsewhere.agent(&other_worktree, "agent-a");
    space.claim(&agent, &issue).await;

    let (_, worktree) = space.agent_on("agent-a").await;
    space.repository.commit(
        &worktree,
        "parser.rs",
        "Unrelated work on a branch of the same name",
    );
    space.board.look().await;
    space
        .repository
        .merge(&["--no-ff", "--message", "Merge agent-a", "agent-a"]);
    space.board.look().await;

    let (status, claim) = space.status(&issue).await;
    assert_eq!(status, "in-progress");
    assert_eq!(claim["worktree"], other_worktree.display().to_string());
}

#[tokio::test(flavor = "multi_thread")]
async fn work_on_the_branch_before_the_claim_does_not_settle_it_and_work_after_the_claim_does() {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    let (agent, worktree) = space.agent_on("agent-a").await;
    let earlier = space
        .repository
        .commit(&worktree, "earlier.rs", "An earlier holder's work");
    space.claim(&agent, &issue).await;
    space.board.look().await;

    space
        .repository
        .merge(&["--no-ff", "--message", "Merge the earlier work", &earlier]);
    space.board.look().await;
    assert_eq!(space.status(&issue).await.0, "in-progress");

    space
        .repository
        .commit(&worktree, "parser.rs", "Parse the headers");
    space.board.look().await;
    space
        .repository
        .merge(&["--no-ff", "--message", "Merge agent-a", "agent-a"]);
    space.board.look().await;
    assert_eq!(space.status(&issue).await, ("merged".into(), Value::Null));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claimed_branch_lands_its_issue_when_another_branch_named_after_the_issue_has_the_same_tip()
 {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    let (agent, worktree) = space.agent_on("agent-a").await;
    space.claim(&agent, &issue).await;
    let tip = space
        .repository
        .commit(&worktree, "parser.rs", "Parse the headers");
    let copy = format!("{}-copy", issue.to_lowercase());
    git(&space.repository.root, &["branch", &copy, &tip]);
    space.board.look().await;

    space
        .repository
        .merge(&["--no-ff", "--message", "Merge agent-a", "agent-a"]);
    space.board.look().await;

    assert_eq!(space.status(&issue).await, ("merged".into(), Value::Null));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_look_that_began_before_the_default_branch_changed_settles_nothing() {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    space.board.look().await;
    let landing = space
        .repository
        .commit_beside("main", &format!("Parse the headers\n\nRefs: {issue}"));
    let stalled = space.repository.stall_ref_reads();
    space.repository.move_ref_by_hand("main", &landing);

    let rebind = async {
        let writer = stalled.await_git().await;
        space
            .board
            .ok(
                &space.repository.human(),
                "project.update",
                json!({ "project": "ABC", "defaultBranch": "trunk" }),
            )
            .await;
        stalled.release(writer);
    };
    tokio::join!(space.board.look(), rebind);

    assert_eq!(space.status(&issue).await.0, "todo");
    assert_eq!(space.merges(&issue).await, Vec::<Value>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_squash_merge_on_the_remote_settles_its_issue_once_this_clone_has_fetched_it() {
    let space = Workspace::new("ABC").await;
    let remote = space.repository.push_to_new_remote();
    let issue = space.issue_in_review("Parse the headers").await;
    space.board.look().await;

    let squash = remote.commit_and_push_to_main(
        "headers.rs",
        &format!("Parse the headers (#7)\n\nRefs: {issue}"),
    );
    space.repository.fetch();
    space.board.look().await;

    assert_eq!(space.status(&issue).await.0, "merged");
    let merges = space.merges(&issue).await;
    assert_eq!(merges.len(), 1, "{merges:?}");
    let reason = merges[0]["reason"].as_str().expect("a reason");
    assert!(
        reason.contains(&format!("landed in origin/main at {squash}")),
        "{reason}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pull_after_a_fetched_merge_writes_no_second_landing() {
    let space = Workspace::new("ABC").await;
    let remote = space.repository.push_to_new_remote();
    let issue = space.issue_in_review("Parse the headers").await;
    space.board.look().await;
    remote.commit_and_push_to_main(
        "headers.rs",
        &format!("Parse the headers (#7)\n\nRefs: {issue}"),
    );
    space.repository.fetch();
    space.board.look().await;
    assert_eq!(space.merges(&issue).await.len(), 1);

    space.repository.pull();
    space.board.look().await;

    assert_eq!(space.status(&issue).await.0, "merged");
    let merges = space.merges(&issue).await;
    assert_eq!(merges.len(), 1, "{merges:?}");
}

#[tokio::test]
async fn the_look_after_the_pull_of_a_fetched_merge_reads_no_commit() {
    let repository = Repository::new();
    let remote = repository.push_to_new_remote();
    let git_repository = GitRepository::at(&repository.common_dir(), HANG_GUARD);
    let look = async |known: Option<&GitWatch>| {
        git_repository
            .look("main", known)
            .await
            .expect("git answers")
            .expect("main exists")
    };
    let (_, known) = step(None, &look(None).await);
    remote.commit_and_push_to_main("headers.rs", "Parse the headers (#7)\n\nRefs: ABC-1");
    repository.fetch();
    let (_, known) = step(Some(&known), &look(Some(&known)).await);

    repository.pull();
    let seen = look(Some(&known)).await;

    assert_eq!(seen.local.reached, Reached::Since(Vec::new()));
    let upstream = seen.upstream.expect("main tracks origin/main");
    assert_eq!(upstream.reached, Reached::Since(Vec::new()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claimed_branch_merged_on_the_remote_settles_its_issue_once_this_clone_has_fetched_it() {
    let space = Workspace::new("ABC").await;
    let remote = space.repository.push_to_new_remote();
    let issue = space.issue("Parse the headers").await;
    let (agent, worktree) = space.agent_on("agent-a").await;
    space.claim(&agent, &issue).await;
    space
        .repository
        .commit(&worktree, "parser.rs", "Parse the headers");
    space.board.look().await;
    space.close(&agent, &issue).await;
    git(&worktree, &["push", "--quiet", "origin", "agent-a"]);

    let merge = remote.merge("agent-a");
    space.repository.fetch();
    space.board.look().await;

    assert_eq!(space.status(&issue).await, ("merged".into(), Value::Null));
    let merges = space.merges(&issue).await;
    assert_eq!(merges.len(), 1, "{merges:?}");
    assert_eq!(merges[0]["branch"], "agent-a");
    let reason = merges[0]["reason"].as_str().expect("a reason");
    assert!(reason.contains("agent-a landed in origin/main"), "{reason}");
    assert!(reason.contains(&merge), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_landing_after_the_remote_main_was_forced_back_and_moved_on_is_seen() {
    let space = Workspace::new("ABC").await;
    let remote = space.repository.push_to_new_remote();
    let before = space.repository.tip(&space.repository.root, "main");
    let first = space.issue_in_review("Parse the headers").await;
    let second = space.issue_in_review("Parse the body").await;
    space.board.look().await;
    remote.commit_and_push_to_main("headers.rs", &format!("Parse the headers\n\nRefs: {first}"));
    space.repository.fetch();
    space.board.look().await;
    assert_eq!(space.status(&first).await.0, "merged");

    remote.force_main_to(&before);
    space.repository.fetch();
    space.board.look().await;
    remote.commit_and_push_to_main("body.rs", &format!("Parse the body\n\nRefs: {second}"));
    space.repository.fetch();
    space.board.look().await;

    assert_eq!(space.status(&second).await.0, "merged");
    assert_eq!(space.status(&first).await.0, "merged");
    assert_eq!(space.merges(&first).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_squash_with_refs_before_close_leaves_the_claimed_issue_in_progress_with_its_landing() {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    let (agent, worktree) = space.agent_on("agent-a").await;
    space.claim(&agent, &issue).await;
    space
        .repository
        .commit(&worktree, "headers.rs", "Parse the headers");
    space.board.look().await;

    space.squash_with_refs("agent-a", &issue).await;
    space.board.look().await;

    let (status, claim) = space.status(&issue).await;
    assert_eq!(status, "in-progress");
    assert_eq!(claim["branch"], "agent-a");
    assert_eq!(space.merges(&issue).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_close_after_its_squash_with_refs_landed_puts_the_issue_in_review_where_later_looks_leave_it()
 {
    let space = Workspace::new("ABC").await;
    let issue = space.issue("Parse the headers").await;
    let (agent, worktree) = space.agent_on("agent-a").await;
    space.claim(&agent, &issue).await;
    space
        .repository
        .commit(&worktree, "headers.rs", "Parse the headers");
    space.board.look().await;
    space.squash_with_refs("agent-a", &issue).await;
    space.board.look().await;

    space.close(&agent, &issue).await;
    space.board.look().await;

    let (status, claim) = space.status(&issue).await;
    assert_eq!(status, "in-review");
    assert_eq!(claim["branch"], "agent-a");
}

impl Workspace {
    async fn rewrite_main_and_prune_while_stopped(
        self,
        gone: &str,
        then: impl FnOnce(&Repository),
    ) -> Workspace {
        let Workspace { board, repository } = self;
        let (home, code) = board.stop().await;
        let root = &repository.root;
        git(root, &["reset", "--quiet", "--hard", "HEAD~1"]);
        repository.commit(root, "rewrite.rs", "The rewrite");
        git(root, &["reflog", "expire", "--expire=now", "--all"]);
        git(root, &["gc", "--quiet", "--prune=now"]);
        assert!(!has_object(&root.join(".git"), gone));
        then(&repository);
        let board = Running::resume(home, code).await;
        Workspace { board, repository }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_look_after_the_kept_tip_was_pruned_by_a_rewrite_starts_again_and_sees_later_landings() {
    let space = Workspace::new("ABC").await;
    let issue = space.issue_in_review("Parse the headers").await;
    let root = space.repository.root.clone();
    let pruned = space.repository.commit(&root, "draft.rs", "A draft");
    space.board.look().await;
    let space = space
        .rewrite_main_and_prune_while_stopped(&pruned, |_| {})
        .await;
    space.board.look().await;

    space.repository.commit(
        &root,
        "parser.rs",
        &format!("Parse the headers\n\nRefs: {issue}"),
    );
    space.board.look().await;

    assert_eq!(space.status(&issue).await.0, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pruned_kept_tip_with_a_new_upstream_records_no_landing_from_history() {
    let space = Workspace::looking_only_when_asked("ABC").await;
    let issue = space.issue_in_review("Parse the headers").await;
    let root = space.repository.root.clone();
    space.repository.commit(
        &root,
        "parser.rs",
        &format!("Parse the headers\n\nRefs: {issue}"),
    );
    space.board.look().await;
    let pruned = space.repository.commit(&root, "draft.rs", "A draft");
    space.board.look().await;
    let mut remote = None;
    let space = space
        .rewrite_main_and_prune_while_stopped(&pruned, |repository| {
            remote = Some(repository.push_to_new_remote());
        })
        .await;

    space.board.look().await;

    assert_eq!(space.status(&issue).await.0, "in-review");
    assert_eq!(space.merges(&issue).await, Vec::<Value>::new());
}
