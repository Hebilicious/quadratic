use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::time::Duration;

use acid_cat_core::{Outcome, Session};
use quadratique_server::app::{Context, application};
use quadratique_server::domain::{Actor, Caller, WorkingCopy};
use quadratique_server::git::watcher::{LookFailed, Watcher};
use quadratique_server::store::{
    AsOf, Author, Call, NewIssue, Project, Store, Workflow, WorkflowStatus,
};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::process::{Pid, PidfdFlags, pidfd_open};
use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use crate::repository::Repository;
use crate::support::{HANG_GUARD, within};

const GIT_LIMIT: Duration = Duration::from_secs(3);

struct Board {
    store: Store,
    _directory: TempDir,
}

fn author() -> Author {
    Author::new(r#"{"kind":"human","name":"sam"}"#).expect("a valid actor")
}

impl Board {
    fn open() -> Board {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(directory.path().join("workspace.db")).expect("the store opens");
        Board {
            store,
            _directory: directory,
        }
    }

    async fn project(&self, key: &str, repository: &Repository) {
        let status = |key: &str, category: &str| WorkflowStatus {
            key: key.into(),
            name: key.into(),
            category: category.into(),
        };
        let project = Project {
            key: key.into(),
            name: format!("Project {key}"),
            repository: Some(repository.common_dir()),
            default_branch: "main".into(),
            extra: "{}".into(),
            workflow: Workflow {
                statuses: vec![
                    status("todo", "unstarted"),
                    status("in-progress", "started"),
                    status("in-review", "started"),
                    status("merged", "started"),
                    status("done", "completed"),
                ],
                moves: r#"{"claim":"in-progress","review":"in-review","merge":"merged","release":"todo"}"#
                    .into(),
            },
        };
        self.store
            .write(Call::new(author(), "project.create"), move |tx| {
                tx.create_project(&project)
            })
            .await
            .expect("the project is created");
    }

    async fn issue(&self, project: &str, title: &str) -> String {
        self.issue_in(project, title, "todo").await
    }

    async fn issue_in(&self, project: &str, title: &str, status: &str) -> String {
        let issue = NewIssue {
            project: project.into(),
            title: title.into(),
            status: status.into(),
            created_by: r#"{"kind":"human","name":"sam"}"#.into(),
            ..NewIssue::default()
        };
        self.store
            .write(Call::new(author(), "issue.create"), move |tx| {
                tx.create_issue(&issue)
                    .map(|created| created.id().to_owned())
            })
            .await
            .expect("the issue is created")
            .value
    }

    async fn status(&self, id: &str) -> String {
        let id = id.to_owned();
        self.store
            .read(AsOf::Now, move |view| {
                Ok::<_, quadratique_server::store::StoreError>(
                    view.issue(&id)?.expect("the issue exists").status,
                )
            })
            .await
            .expect("the issue is readable")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_git_command_that_stalls_is_ended_and_the_other_projects_are_still_watched() {
    let board = Board::open();
    let stalled = Repository::new();
    let _stalled_refs = stalled.stall_ref_reads();
    let watched = Repository::new();
    board.project("AAA", &stalled).await;
    board.project("ABC", &watched).await;
    let issue = board.issue("ABC", "Parse the headers").await;
    let watcher = Watcher::start(board.store.clone(), HANG_GUARD, GIT_LIMIT, HANG_GUARD);
    within("the first look", watcher.look()).await;

    watched.commit(
        &watched.root,
        "parser.rs",
        &format!("Parse the headers\n\nRefs: {issue}"),
    );
    within("a look past the stalled repository", watcher.look()).await;

    assert_eq!(board.status(&issue).await, "merged");
    within("the watcher's stop", watcher.stop()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_whose_repository_is_gone_does_not_stop_the_others() {
    let board = Board::open();
    let gone = Repository::new();
    let watched = Repository::new();
    board.project("AAA", &gone).await;
    board.project("ABC", &watched).await;
    drop(gone);
    let issue = board.issue("ABC", "Parse the headers").await;
    let watcher = Watcher::start(board.store.clone(), HANG_GUARD, GIT_LIMIT, HANG_GUARD);
    within("the first look", watcher.look()).await;

    watched.commit(
        &watched.root,
        "parser.rs",
        &format!("Parse the headers\n\nRefs: {issue}"),
    );
    within("a look past the missing repository", watcher.look()).await;

    assert_eq!(board.status(&issue).await, "merged");
    within("the watcher's stop", watcher.stop()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confirmed_look_whose_git_passes_its_limit_answers_timed_out() {
    let board = Board::open();
    let stalled = Repository::new();
    board.project("AAA", &stalled).await;
    let watcher = Watcher::start(board.store.clone(), HANG_GUARD * 10, GIT_LIMIT, HANG_GUARD);
    within("the first look", watcher.look()).await;
    let _stalled_refs = stalled.stall_ref_reads();

    let outcome = within("a confirmed look", watcher.looks().confirm("AAA")).await;

    assert!(
        matches!(outcome, Err(LookFailed::TimedOut(_))),
        "{outcome:?}"
    );
    within("the watcher's stop", watcher.stop()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confirmed_look_at_a_repository_git_cannot_read_answers_failed() {
    let board = Board::open();
    let gone = Repository::new();
    board.project("AAA", &gone).await;
    let watcher = Watcher::start(board.store.clone(), HANG_GUARD * 10, GIT_LIMIT, HANG_GUARD);
    within("the first look", watcher.look()).await;
    drop(gone);

    let outcome = within("a confirmed look", watcher.looks().confirm("AAA")).await;

    assert!(
        matches!(&outcome, Err(LookFailed::Failed(reason)) if reason.contains("git for-each-ref fails") && reason.contains("not a git repository")),
        "{outcome:?}"
    );
    within("the watcher's stop", watcher.stop()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn confirmations_asked_during_a_look_are_answered_by_the_next_look_each_with_its_own_outcome()
{
    let board = Board::open();
    let paused = Repository::new();
    let readable = Repository::new();
    let gone = Repository::new();
    board.project("AAA", &paused).await;
    board.project("BBB", &readable).await;
    board.project("CCC", &gone).await;
    let watcher = Watcher::start(board.store.clone(), HANG_GUARD * 10, HANG_GUARD, HANG_GUARD);
    within("the first look", watcher.look()).await;
    drop(gone);
    let looks = watcher.looks();
    let stalled = paused.stall_ref_reads();
    let running = looks.confirm("AAA");
    let writer = stalled.await_git().await;

    let readable_outcome = looks.confirm("BBB");
    let gone_outcome = looks.confirm("CCC");
    stalled.release(writer);

    assert!(within("AAA's look", running).await.is_ok());
    let readable_tip = readable.tip(&readable.root, "main");
    match within("BBB's look", readable_outcome).await {
        Ok(confirmed) => assert_eq!(confirmed.tips.local, Some(readable_tip)),
        Err(failed) => panic!("BBB was not looked at by the next look: {failed}"),
    }
    let gone_outcome = within("CCC's look", gone_outcome).await;
    assert!(
        matches!(&gone_outcome, Err(LookFailed::Failed(reason)) if reason.contains("git for-each-ref fails")),
        "{gone_outcome:?}"
    );
    within("the watcher's stop", watcher.stop()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_during_a_look_ends_it_without_waiting_for_git() {
    let board = Board::open();
    let stalled = Repository::new();
    let _stalled_refs = stalled.stall_ref_reads();
    board.project("AAA", &stalled).await;
    let watcher = Watcher::start(board.store.clone(), HANG_GUARD, HANG_GUARD * 10, HANG_GUARD);
    let repository = stalled.common_dir();
    within("git starting on the stalled repository", async {
        while git_running_on(&repository).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    let stalled_git: Vec<OwnedFd> = git_running_on(&repository)
        .iter()
        .filter_map(|pid| pidfd(pid))
        .collect();
    assert!(!stalled_git.is_empty(), "no stalled git to wait on");

    within("the watcher's stop", watcher.stop()).await;

    for git in &stalled_git {
        assert!(
            ended_within(git, HANG_GUARD),
            "the stalled git still runs {HANG_GUARD:?} after the stop"
        );
    }
}

fn pidfd(pid: &str) -> Option<OwnedFd> {
    let pid = Pid::from_raw(pid.parse().ok()?)?;
    pidfd_open(pid, PidfdFlags::empty()).ok()
}

fn ended_within(process: &OwnedFd, limit: Duration) -> bool {
    let limit = Timespec::try_from(limit).expect("a limit poll can take");
    let mut fds = [PollFd::new(process, PollFlags::IN)];
    poll(&mut fds, Some(&limit)).expect("the process can be waited on") == 1
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_of_a_started_issue_asks_for_a_look_and_answers_while_git_still_stalls() {
    let board = Board::open();
    let stalled = Repository::new();
    board.project("ABC", &stalled).await;
    let issue = board
        .issue_in("ABC", "Parse the headers", "in-review")
        .await;
    let read_wait = Duration::from_millis(200);
    let git_limit = HANG_GUARD * 10;
    let watcher = Watcher::start(board.store.clone(), HANG_GUARD * 10, git_limit, read_wait);
    within("the first look", watcher.look()).await;
    let _stalled_refs = stalled.stall_ref_reads();
    let repository = stalled.common_dir();
    assert_eq!(git_running_on(&repository), Vec::<String>::new());
    let app = application().expect("the application");
    let human = Caller {
        actor: Actor::without_header(),
        working_copy: WorkingCopy::default(),
    };
    let context = Context::watching(board.store.clone(), human, watcher.looks());
    let session = Session::open(&app, "cli", Arc::new(context));

    let read = within(
        "a read shorter than the git limit",
        session.invoke(
            "issue.get",
            json!({ "issue": issue }),
            &CancellationToken::new(),
        ),
    )
    .await;

    match read {
        Outcome::Value(answer) => assert_eq!(answer["issue"]["status"], "in-review", "{answer}"),
        other => panic!("the read failed: {other:?}"),
    }
    within("git started by the read's look", async {
        while git_running_on(&repository).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    within("the watcher's stop", watcher.stop()).await;
}

fn git_running_on(repository: &str) -> Vec<String> {
    let wanted = format!("--git-dir={repository}");
    std::fs::read_dir("/proc")
        .expect("the process list is readable")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let arguments = std::fs::read(entry.path().join("cmdline")).ok()?;
            arguments
                .split(|byte| *byte == 0)
                .any(|argument| argument == wanted.as_bytes())
                .then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect()
}
