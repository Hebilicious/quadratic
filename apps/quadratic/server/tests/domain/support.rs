use proptest::prelude::{ProptestConfig, Strategy, TestCaseError};
use proptest::test_runner::{RngAlgorithm, RngSeed, TestRunner};
use quadratic_server::domain::{
    Actor, ActorKind, Assignee, Caller, Category, Claim, Issue, Moves, Project, Workflow,
    WorkflowStatus, WorkingCopy,
};

pub struct Board {
    pub label: &'static str,
    pub project: Project,
    pub backlog: &'static str,
    pub todo: &'static str,
    pub in_progress: &'static str,
    pub in_review: &'static str,
    pub merged: &'static str,
    pub done: &'static str,
    pub canceled: &'static str,
}

impl Board {
    pub fn default_workflow() -> Board {
        Board {
            label: "default workflow",
            project: project(code_workflow()),
            backlog: "backlog",
            todo: "todo",
            in_progress: "in-progress",
            in_review: "in-review",
            merged: "merged",
            done: "done",
            canceled: "canceled",
        }
    }

    pub fn done_at_merge() -> Board {
        Board {
            label: "workflow done at merge",
            project: project(done_at_merge_workflow()),
            backlog: "icebox",
            todo: "ready",
            in_progress: "doing",
            in_review: "checking",
            merged: "shipped",
            done: "shipped",
            canceled: "dropped",
        }
    }

    pub fn issue(&self, status: &str) -> Issue {
        Issue {
            id: "DEM-12".into(),
            status: status.into(),
            category: self
                .project
                .workflow
                .category_of(status)
                .expect("the fixture names a status of its workflow"),
            revision: 3,
            claim: None,
            assignee: None,
            bases: Vec::new(),
        }
    }
}

pub fn boards() -> [Board; 2] {
    [Board::default_workflow(), Board::done_at_merge()]
}

pub fn code_workflow() -> Workflow {
    let status = |key: &str, name: &str, category| WorkflowStatus {
        key: key.into(),
        name: name.into(),
        category,
    };
    Workflow {
        statuses: vec![
            status("backlog", "Backlog", Category::Backlog),
            status("todo", "Todo", Category::Unstarted),
            status("in-progress", "In progress", Category::Started),
            status("in-review", "In review", Category::Started),
            status("merged", "Merged", Category::Started),
            status("done", "Done", Category::Completed),
            status("canceled", "Canceled", Category::Canceled),
        ],
        moves: Moves {
            claim: "in-progress".into(),
            review: "in-review".into(),
            merge: "merged".into(),
            release: "todo".into(),
        },
    }
}

pub fn done_at_merge_workflow() -> Workflow {
    let status = |key: &str, category| WorkflowStatus {
        key: key.into(),
        name: key.into(),
        category,
    };
    Workflow {
        statuses: vec![
            status("icebox", Category::Backlog),
            status("ready", Category::Unstarted),
            status("doing", Category::Started),
            status("checking", Category::Started),
            status("shipped", Category::Completed),
            status("dropped", Category::Canceled),
        ],
        moves: Moves {
            claim: "doing".into(),
            review: "checking".into(),
            merge: "shipped".into(),
            release: "ready".into(),
        },
    }
}

fn project(workflow: Workflow) -> Project {
    Project {
        workflow,
        default_branch: "main".into(),
        repository: Some("/home/sam/code/demo/.git".into()),
    }
}

pub fn actor(kind: ActorKind, name: &str, role: Option<&str>) -> Actor {
    Actor {
        kind,
        name: name.into(),
        role: role.map(Into::into),
        harness: None,
        host: None,
        host_thread: None,
        parent: None,
        model: None,
        session: None,
    }
}

pub fn agent_in(worktree: &str, branch: &str) -> Caller {
    let directory = worktree.rsplit('/').next().unwrap_or(worktree);
    Caller {
        actor: actor(ActorKind::Agent, &format!("claude-code@{directory}"), None),
        working_copy: WorkingCopy {
            repository: Some("/home/sam/code/demo/.git".into()),
            worktree: Some(worktree.into()),
            branch: Some(branch.into()),
            contains: None,
        },
    }
}

pub fn agent() -> Caller {
    agent_in("/w/fix-login", "fix-login")
}

pub fn other_agent() -> Caller {
    agent_in("/w/add-search", "add-search")
}

pub fn agent_with_role(role: &str) -> Caller {
    let mut caller = agent();
    caller.actor.role = Some(role.into());
    caller.actor.name = format!("[{role}]claude-code@fix-login");
    caller
}

pub fn human() -> Caller {
    Caller {
        actor: actor(ActorKind::Human, "sam", None),
        working_copy: WorkingCopy::default(),
    }
}

pub fn claim_of(caller: &Caller) -> Claim {
    Claim {
        holder: caller.actor.clone(),
        branch: caller.working_copy.branch.clone().unwrap_or_default(),
        worktree: caller.working_copy.worktree.clone().unwrap_or_default(),
        since: "2026-11-18T09:30:00Z".into(),
    }
}

pub fn assigned(mut issue: Issue, assignee: Assignee) -> Issue {
    issue.assignee = Some(assignee);
    issue
}

pub fn claimed(mut issue: Issue, claim: Claim) -> Issue {
    issue.claim = Some(claim);
    issue
}

pub fn property<S: Strategy>(
    cases: u32,
    strategy: S,
    test: impl Fn(S::Value) -> Result<(), TestCaseError>,
) {
    let mut runner = TestRunner::new(ProptestConfig {
        cases,
        max_local_rejects: 65_536,
        max_global_rejects: 1024,
        max_flat_map_regens: 1_000_000,
        failure_persistence: None,
        fork: false,
        timeout: 0,
        max_shrink_time: 0,
        max_shrink_iters: 10_000,
        max_default_size_range: 100,
        verbose: 0,
        rng_algorithm: RngAlgorithm::ChaCha,
        rng_seed: RngSeed::Fixed(0x5155_4144),
        ..ProptestConfig::default()
    });
    if let Err(failure) = runner.run(&strategy, test) {
        panic!("{failure}");
    }
}
