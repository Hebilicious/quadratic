use std::sync::Arc;

use acid_cat_core::{Outcome, Session};
use quadratique_server::app::{Context, application};
use quadratique_server::domain::{Actor, Caller, WorkingCopy};
use quadratique_server::git::repository::GIT_LIMIT;
use quadratique_server::git::watcher::Watcher;
use quadratique_server::store::{Author, Call, NewIssue, Project, Store, Workflow, WorkflowStatus};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::TryRecvError;
use tokio_util::sync::CancellationToken;

use crate::repository::{Repository, git};
use crate::support::{HANG_GUARD, Running, Who, start_looking_every, within};

const AFTER_THE_MERGE: [(&str, &str); 8] = [
    ("backlog", "backlog"),
    ("todo", "unstarted"),
    ("in-progress", "started"),
    ("in-review", "started"),
    ("merged", "started"),
    ("deployed", "started"),
    ("done", "completed"),
    ("canceled", "canceled"),
];

const DONE_AT_THE_MERGE: [(&str, &str); 6] = [
    ("todo", "unstarted"),
    ("in-progress", "started"),
    ("in-review", "started"),
    ("merged", "completed"),
    ("deployed", "completed"),
    ("canceled", "canceled"),
];

struct Workspace {
    board: Running,
    repository: Repository,
}

impl Workspace {
    async fn new(statuses: &[(&str, &str)]) -> Workspace {
        let board = start_looking_every(HANG_GUARD * 10).await;
        let repository = Repository::new();
        let human = repository.human();
        board
            .ok(
                &human,
                "project.create",
                json!({ "key": "ABC", "name": "Project ABC", "defaultBranch": "main" }),
            )
            .await;
        let statuses: Vec<Value> = statuses
            .iter()
            .map(|(key, category)| json!({ "key": key, "name": key, "category": category }))
            .collect();
        let moves = json!({ "claim": "in-progress", "review": "in-review", "merge": "merged", "release": "todo" });
        board
            .ok(
                &human,
                "project.update",
                json!({ "project": "ABC", "workflow": { "statuses": statuses, "moves": moves } }),
            )
            .await;
        let space = Workspace { board, repository };
        space.board.look().await;
        space
    }

    fn human(&self) -> Who {
        self.repository.human()
    }

    async fn issue(&self, input: Value) -> String {
        let created = self.board.ok(&self.human(), "issue.create", input).await;
        created["issue"]["id"].as_str().expect("an id").to_owned()
    }

    fn land_on_main(&self, issue: &str) -> String {
        let root = &self.repository.root;
        self.repository.commit(
            root,
            &format!("{issue}.rs"),
            &format!("Work\n\nRefs: {issue}"),
        )
    }

    async fn merged(&self, title: &str) -> (String, String) {
        let issue = self
            .issue(json!({ "acceptance": "It is done.", "title": title }))
            .await;
        let landing = self.land_on_main(&issue);
        self.board.look().await;
        assert_eq!(self.status(&issue).await, "merged");
        (issue, landing)
    }

    async fn advance(&self, who: &Who, commit: &str, status: &str) -> Result<Value, Value> {
        self.board
            .call(
                who,
                "issue.advance",
                json!({ "project": "ABC", "commit": commit, "status": status }),
            )
            .await
    }

    async fn advanced(&self, commit: &str) -> (Vec<String>, Vec<String>) {
        let answer = self
            .advance(&self.human(), commit, "deployed")
            .await
            .unwrap_or_else(|failure| panic!("issue.advance failed: {failure}"));
        (ids(&answer["moved"]), ids(&answer["left"]))
    }

    async fn refused(&self, commit: &str, status: &str) -> Value {
        match self.advance(&self.human(), commit, status).await {
            Ok(answer) => panic!("issue.advance succeeded: {answer}"),
            Err(failure) => failure,
        }
    }

    async fn status(&self, issue: &str) -> String {
        self.board.get(issue).await["status"]
            .as_str()
            .expect("a status")
            .to_owned()
    }

    async fn history(&self, issue: &str) -> Vec<Value> {
        let page = self
            .board
            .ok(&self.human(), "history.query", json!({ "issue": issue }))
            .await;
        page["items"].as_array().expect("a list of changes").clone()
    }
}

fn ids(issue_ids: &Value) -> Vec<String> {
    let issues: Vec<String> = issue_ids["issues"]
        .as_array()
        .expect("a list of issues")
        .iter()
        .map(|issue| issue.as_str().expect("an issue id").to_owned())
        .collect();
    assert_eq!(issue_ids["count"], issues.len(), "{issue_ids}");
    issues
}

#[tokio::test(flavor = "multi_thread")]
async fn each_deploy_moves_the_issues_that_landed_up_to_its_commit_once() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let (first, at_a) = space.merged("Parse the headers").await;
    let (second, at_b) = space.merged("Parse the body").await;

    assert_eq!(
        space.advanced(&at_a).await,
        (vec![first.clone()], vec![second.clone()])
    );
    assert_eq!(space.status(&second).await, "merged");
    assert_eq!(space.advanced(&at_b).await, (vec![second.clone()], vec![]));

    assert_eq!(space.advanced(&at_b).await, (vec![], vec![]));
    assert_eq!(space.status(&first).await, "deployed");
    assert_eq!(space.status(&second).await, "deployed");
    let advances: Vec<Value> = space
        .history(&first)
        .await
        .into_iter()
        .filter(|change| change["operation"] == "issue.advance")
        .collect();
    assert_eq!(advances.len(), 1, "{advances:?}");
    assert_eq!(advances[0]["author"]["name"], "sam");
    let reason = advances[0]["reason"].as_str().expect("a reason");
    assert!(reason.contains(&at_a), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_merged_with_no_ff_moves_at_the_merge_and_not_at_a_commit_before_it() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let issue = space
        .issue(json!({ "acceptance": "It is done.", "title": "Parse the headers" }))
        .await;
    let worktree = space.repository.worktree("agent-a");
    let agent = space.repository.agent(&worktree, "agent-a");
    space.board.look().await;
    space
        .board
        .ok(&agent, "issue.claim", json!({ "issue": issue }))
        .await;
    space
        .repository
        .commit(&worktree, "parser.rs", "Parse the headers");
    space.board.look().await;
    space
        .board
        .ok(
            &agent,
            "issue.close",
            json!({ "issue": issue, "evidence": "the tests pass" }),
        )
        .await;
    let root = &space.repository.root;
    let before = space.repository.commit(root, "notes.md", "Other work");
    git(root, &["checkout", "--quiet", "--detach"]);
    git(
        root,
        &[
            "merge",
            "--quiet",
            "--no-ff",
            "--message",
            "Merge agent-a",
            "agent-a",
        ],
    );
    let merge = space.repository.tip(root, "HEAD");
    let later = space
        .repository
        .commit(root, "later.md", "Work after the merge");
    git(root, &["update-ref", "refs/heads/main", &later, &before]);
    git(root, &["checkout", "--quiet", "main"]);
    space.board.look().await;
    assert_eq!(space.status(&issue).await, "merged");

    assert_eq!(space.advanced(&before).await, (vec![], vec![issue.clone()]));
    assert_eq!(space.status(&issue).await, "merged");
    assert_eq!(space.advanced(&merge).await, (vec![issue.clone()], vec![]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_commit_of_another_branch_that_contains_a_landing_is_not_on_the_default_branch() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let (issue, _) = space.merged("Parse the headers").await;
    let preview = space.repository.worktree("preview");
    let deployed = space
        .repository
        .commit(&preview, "preview.md", "A preview deploy");

    let failure = space.refused(&deployed, "deployed").await;

    assert_eq!(
        failure,
        json!({ "_tag": "NotOnDefaultBranch", "commit": deployed })
    );
    assert_eq!(space.status(&issue).await, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_landing_no_look_has_seen_yet_is_settled_by_the_call_and_moved() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let issue = space
        .issue(json!({ "acceptance": "It is done.", "title": "Parse the headers", "status": "in-review" }))
        .await;
    let landing = space.land_on_main(&issue);

    assert_eq!(
        space.advanced(&landing).await,
        (vec![issue.clone()], vec![])
    );

    let operations: Vec<Value> = space
        .history(&issue)
        .await
        .iter()
        .map(|change| change["operation"].clone())
        .collect();
    assert_eq!(
        operations[..2],
        [json!("issue.advance"), json!("git.merged")]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repository_git_cannot_read_fails_the_scan_and_moves_nothing() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let (issue, landing) = space.merged("Parse the headers").await;
    let root = &space.repository.root;
    std::fs::rename(root.join(".git"), root.join("away")).expect("the repository is moved");

    let failure = space.refused(&landing, "deployed").await;

    assert_eq!(failure["_tag"], "ScanFailed", "{failure}");
    assert_eq!(space.status(&issue).await, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rebind_between_the_look_and_the_write_moves_nothing() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let (issue, landing) = space.merged("Parse the headers").await;
    space.board.look().await;
    let stalled = space.repository.stall_ref_reads();

    let rebind = async {
        let writer = stalled.await_git().await;
        space
            .board
            .ok(
                &space.human(),
                "project.update",
                json!({ "project": "ABC", "defaultBranch": "trunk" }),
            )
            .await;
        stalled.release(writer);
    };
    let human = space.human();
    let (advanced, ()) = tokio::join!(space.advance(&human, &landing, "deployed"), rebind);

    assert_eq!(
        advanced,
        Err(json!({ "_tag": "RepositoryChanged", "project": "ABC" }))
    );
    assert_eq!(space.status(&issue).await, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_squash_with_refs_before_the_holders_close_waits_in_review_and_is_left() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let issue = space
        .issue(json!({ "acceptance": "It is done.", "title": "Parse the headers" }))
        .await;
    let worktree = space.repository.worktree("agent-a");
    let agent = space.repository.agent(&worktree, "agent-a");
    space.board.look().await;
    space
        .board
        .ok(&agent, "issue.claim", json!({ "issue": issue }))
        .await;
    space
        .repository
        .commit(&worktree, "headers.rs", "Parse the headers");
    space.board.look().await;
    let root = &space.repository.root;
    git(root, &["merge", "--quiet", "--squash", "agent-a"]);
    let message = format!("Parse the headers (#7)\n\nRefs: {issue}");
    git(root, &["commit", "--quiet", "--message", &message]);
    let squash = space.repository.tip(root, "HEAD");
    space.board.look().await;
    space
        .board
        .ok(
            &agent,
            "issue.close",
            json!({ "issue": issue, "evidence": "the tests pass" }),
        )
        .await;
    assert_eq!(space.status(&issue).await, "in-review");

    assert_eq!(space.advanced(&squash).await, (vec![], vec![issue.clone()]));
    assert_eq!(space.status(&issue).await, "in-review");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_issue_moved_back_after_its_landing_is_left() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let (issue, landing) = space.merged("Parse the headers").await;
    let revision = space.board.get(&issue).await["revision"].clone();
    space
        .board
        .ok(
            &space.human(),
            "issue.update",
            json!({ "issue": issue, "revision": revision, "status": "in-progress" }),
        )
        .await;

    assert_eq!(
        space.advanced(&landing).await,
        (vec![], vec![issue.clone()])
    );
    assert_eq!(space.status(&issue).await, "in-progress");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_issue_of_a_workflow_done_at_the_merge_is_neither_moved_nor_left() {
    let space = Workspace::new(&DONE_AT_THE_MERGE).await;
    let (issue, landing) = space.merged("Parse the headers").await;

    assert_eq!(space.advanced(&landing).await, (vec![], vec![]));
    assert_eq!(space.status(&issue).await, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_issue_assigned_to_the_human_is_left_when_an_agent_deploys() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let issue = space
        .issue(json!({ "acceptance": "It is done.", "title": "Choose the license", "assignee": { "kind": "human" } }))
        .await;
    let landing = space.land_on_main(&issue);
    space.board.look().await;
    let worktree = space.repository.worktree("deployer");
    let agent = space.repository.agent(&worktree, "deployer");

    let answer = space
        .advance(&agent, &landing, "deployed")
        .await
        .unwrap_or_else(|failure| panic!("issue.advance failed: {failure}"));

    assert_eq!(
        (ids(&answer["moved"]), ids(&answer["left"])),
        (vec![], vec![issue.clone()])
    );
    assert_eq!(space.status(&issue).await, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_status_not_after_the_merge_an_unknown_status_and_a_commit_the_clone_lacks_are_refused() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let (issue, landing) = space.merged("Parse the headers").await;
    let after_the_merge = json!(["deployed", "done"]);

    for status in ["in-review", "canceled"] {
        assert_eq!(
            space.refused(&landing, status).await,
            json!({ "_tag": "NotAfterMerge", "status": status, "statuses": after_the_merge })
        );
    }
    assert_eq!(
        space.refused(&landing, "shipped").await["_tag"],
        "UnknownStatus"
    );
    let lacking = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(
        space.refused(lacking, "deployed").await,
        json!({ "_tag": "NotInRepository", "commit": lacking })
    );
    assert_eq!(space.status(&issue).await, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_history_git_cannot_list_fails_the_scan_and_moves_nothing() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let root = &space.repository.root;
    let first = space.repository.tip(root, "main");
    let (issue, _) = space.merged("Parse the headers").await;
    let deployed = space.repository.commit(root, "notes.md", "Other work");
    space.board.look().await;
    let object = root
        .join(".git/objects")
        .join(&first[..2])
        .join(&first[2..]);
    std::fs::remove_file(object).expect("the first commit's object is removed");

    let failure = space.refused(&deployed, "deployed").await;

    assert_eq!(failure["_tag"], "ScanFailed", "{failure}");
    let problem = failure["problem"].as_str().expect("a problem");
    assert!(problem.contains("rev-list"), "{problem}");
    assert_eq!(space.status(&issue).await, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_old_deploy_does_not_advance_an_issue_that_landed_again_after_it() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let (issue, at_a) = space.merged("Parse the headers").await;
    assert_eq!(space.advanced(&at_a).await, (vec![issue.clone()], vec![]));
    let root = &space.repository.root;
    space.repository.commit(
        root,
        "follow-up.rs",
        &format!("A follow-up\n\nRefs: {issue}"),
    );
    space.board.look().await;
    assert_eq!(space.status(&issue).await, "merged");

    assert_eq!(space.advanced(&at_a).await, (vec![], vec![issue.clone()]));
    assert_eq!(space.status(&issue).await, "merged");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_annotated_tag_is_not_a_commit_to_deploy() {
    let space = Workspace::new(&AFTER_THE_MERGE).await;
    let (issue, landing) = space.merged("Parse the headers").await;
    let root = &space.repository.root;
    git(
        root,
        &[
            "tag",
            "--annotate",
            "--message",
            "Release 1",
            "v1",
            &landing,
        ],
    );
    let tag = space.repository.tip(root, "v1");
    assert_ne!(tag, landing);

    let failure = space.refused(&tag, "deployed").await;

    assert_eq!(failure, json!({ "_tag": "NotInRepository", "commit": tag }));
    assert_eq!(space.status(&issue).await, "merged");
}

struct Ledger {
    store: Store,
    watcher: Watcher,
    session: Session<Context>,
    repository: Repository,
    _home: tempfile::TempDir,
}

impl Ledger {
    async fn open() -> Ledger {
        let home = crate::support::private_home();
        let store = Store::open(home.path().join("workspace.db")).expect("the store opens");
        let repository = Repository::new();
        let author = Author::new(r#"{"kind":"human","name":"sam"}"#).expect("a valid actor");
        let project = Project {
            key: "ABC".into(),
            name: "Project ABC".into(),
            repository: Some(repository.common_dir()),
            default_branch: "main".into(),
            extra: "{}".into(),
            workflow: Workflow {
                statuses: AFTER_THE_MERGE
                    .iter()
                    .map(|(key, category)| WorkflowStatus {
                        key: (*key).into(),
                        name: (*key).into(),
                        category: (*category).into(),
                    })
                    .collect(),
                moves: r#"{"claim":"in-progress","review":"in-review","merge":"merged","release":"todo"}"#
                    .into(),
            },
        };
        store
            .write(Call::new(author, "project.create"), move |tx| {
                tx.create_project(&project)
            })
            .await
            .expect("the project is created");
        let watcher = Watcher::start(store.clone(), HANG_GUARD * 10, GIT_LIMIT, HANG_GUARD);
        within("the first look", watcher.look()).await;
        let human = Caller {
            actor: Actor::without_header(),
            working_copy: WorkingCopy::default(),
        };
        let context = Context::watching(store.clone(), human, watcher.looks());
        let session = Session::open(
            &application().expect("the application"),
            "cli",
            Arc::new(context),
        );
        Ledger {
            store,
            watcher,
            session,
            repository,
            _home: home,
        }
    }

    async fn landed(&self, title: &str) -> String {
        let issue = NewIssue {
            project: "ABC".into(),
            title: title.into(),
            status: "in-review".into(),
            created_by: r#"{"kind":"human","name":"sam"}"#.into(),
            ..NewIssue::default()
        };
        let author = Author::new(r#"{"kind":"human","name":"sam"}"#).expect("a valid actor");
        let id = self
            .store
            .write(Call::new(author, "issue.create"), move |tx| {
                tx.create_issue(&issue)
                    .map(|created| created.id().to_owned())
            })
            .await
            .expect("the issue is created")
            .value;
        let root = &self.repository.root;
        self.repository
            .commit(root, &format!("{id}.rs"), &format!("Work\n\nRefs: {id}"));
        id
    }

    async fn advance(&self, commit: &str) -> Value {
        let outcome = within(
            "issue.advance",
            self.session.invoke(
                "issue.advance",
                json!({ "project": "ABC", "commit": commit, "status": "deployed" }),
                &CancellationToken::new(),
            ),
        )
        .await;
        match outcome {
            Outcome::Value(answer) => answer,
            other => panic!("issue.advance failed: {other:?}"),
        }
    }

    fn head(&self) -> String {
        self.repository.tip(&self.repository.root, "main")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_issues_one_call_moves_share_one_database_commit() {
    let ledger = Ledger::open().await;
    let first = ledger.landed("Parse the headers").await;
    let second = ledger.landed("Parse the body").await;
    within("a look", ledger.watcher.look()).await;
    let mut commits = ledger.store.subscribe();

    let answer = ledger.advance(&ledger.head()).await;

    assert_eq!(
        answer["moved"],
        json!({ "count": 2, "issues": [first, second] })
    );
    let commit = commits.try_recv().expect("the call made a commit");
    assert_eq!(commit.operation, "issue.advance");
    assert_eq!(commit.issues, [first, second]);
    assert_eq!(commits.try_recv().err(), Some(TryRecvError::Empty));
    within("the watcher's stop", ledger.watcher.stop()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_deploy_again_makes_no_database_commit() {
    let ledger = Ledger::open().await;
    ledger.landed("Parse the headers").await;
    within("a look", ledger.watcher.look()).await;
    let deployed = ledger.head();
    ledger.advance(&deployed).await;
    let mut commits = ledger.store.subscribe();

    let answer = ledger.advance(&deployed).await;

    assert_eq!(answer["moved"]["count"], 0, "{answer}");
    assert_eq!(commits.try_recv().err(), Some(TryRecvError::Empty));
    within("the watcher's stop", ledger.watcher.stop()).await;
}
