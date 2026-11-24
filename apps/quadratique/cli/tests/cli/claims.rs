use crate::support::{Home, Process, Repository, outside};

#[test]
fn agents_in_several_worktrees_claiming_one_issue_at_once_get_one_success_and_held_failures() {
    const WORKERS: usize = 3;
    let home = Home::new();
    let repository = Repository::new();
    let worktrees: Vec<_> = (0..WORKERS)
        .map(|worker| repository.worktree(&format!("racer-{worker}")))
        .collect();
    home.quadratique(&repository.root)
        .args(["project", "create", "--key", "RACE", "--name", "Race"])
        .run()
        .ok();
    let issue = home
        .quadratique(&repository.root)
        .args([
            "create",
            "--title",
            "Contested",
            "--acceptance",
            "It is done.",
        ])
        .run()
        .ok()["issue"]["id"]
        .as_str()
        .expect("an issue id")
        .to_owned();

    let claims: Vec<_> = worktrees
        .iter()
        .map(|worktree| {
            home.quadratique(worktree)
                .env("CLAUDECODE", "1")
                .args(["claim", "--issue", &issue])
                .spawn()
        })
        .collect();
    let runs: Vec<_> = claims.into_iter().map(Process::finish).collect();

    let winners: Vec<_> = runs.iter().filter(|run| run.code == 0).collect();
    assert_eq!(winners.len(), 1, "{runs:?}");
    let holder = winners[0].json()["issue"]["claim"]["holder"]["name"].clone();
    for loser in runs.iter().filter(|run| run.code != 0) {
        assert_eq!(loser.code, 1, "{loser:?}");
        let failure = loser.json();
        assert_eq!(failure["_tag"], "Held", "{loser:?}");
        assert_eq!(failure["issue"], issue.as_str());
        assert_eq!(failure["claim"]["holder"]["name"], holder);
    }
}

#[test]
fn an_agent_claims_under_the_name_its_environment_builds_in_its_own_worktree_and_branch() {
    let home = Home::new();
    let repository = Repository::new();
    let worktree = repository.worktree("audit");
    home.quadratique(&repository.root)
        .args(["project", "create", "--key", "SEC", "--name", "Security"])
        .run()
        .ok();
    home.quadratique(&repository.root)
        .args([
            "create",
            "--title",
            "Audit the parser",
            "--acceptance",
            "It is done.",
        ])
        .run()
        .ok();

    let claimed = home
        .quadratique(&worktree)
        .env("CLAUDECODE", "1")
        .env("QUADRATIQUE_ROLE", "security")
        .env("T3CODE_THREAD_ID", "thread-42")
        .args(["claim", "--issue", "SEC-1"])
        .run()
        .ok();
    let holder = &claimed["issue"]["claim"]["holder"];
    assert_eq!(holder["name"], "[security]claude-code(t3)@audit");
    assert_eq!(holder["kind"], "agent");
    assert_eq!(holder["role"], "security");
    assert_eq!(holder["host"], "t3");
    assert_eq!(holder["hostThread"], "thread-42");
    assert_eq!(claimed["issue"]["claim"]["branch"], "audit");
    assert_eq!(
        claimed["issue"]["claim"]["worktree"],
        worktree.display().to_string()
    );
}

#[test]
fn an_invalid_role_fails_before_any_call_to_the_server() {
    let home = Home::new();
    let repository = Repository::new();

    let refused = home
        .quadratique(&repository.root)
        .env("QUADRATIQUE_ROLE", "Security Team")
        .args(["claim", "--issue", "SEC-1"])
        .run();

    assert_eq!(refused.code, 3, "{refused:?}");
    assert!(refused.stderr.contains("QUADRATIQUE_ROLE"), "{refused:?}");
    assert!(home.runtime().is_none());
    assert!(home.servers().is_empty());
}

#[test]
fn an_agent_in_a_worktree_whose_name_holds_a_line_break_claims_under_that_name() {
    let home = Home::new();
    let repository = Repository::new();
    let worktree = repository.root.with_file_name("line\nbreak");
    repository.add_worktree("line-break", &worktree);
    home.quadratique(&repository.root)
        .args(["project", "create", "--key", "NL", "--name", "Newline"])
        .run()
        .ok();
    let issue = home
        .quadratique(&repository.root)
        .args([
            "create",
            "--title",
            "Odd paths",
            "--acceptance",
            "It is done.",
        ])
        .run()
        .ok()["issue"]["id"]
        .as_str()
        .expect("an issue id")
        .to_owned();

    let claimed = home
        .quadratique(&worktree)
        .env("CLAUDECODE", "1")
        .args(["claim", "--issue", &issue])
        .run()
        .ok();

    assert_eq!(
        claimed["issue"]["claim"]["holder"]["name"],
        "claude-code@line\nbreak"
    );
    assert_eq!(
        claimed["issue"]["claim"]["worktree"],
        worktree.display().to_string()
    );
}

#[test]
fn outside_a_repository_a_claim_fails_with_no_working_copy_and_exit_code_1() {
    let home = Home::new();
    let elsewhere = outside();
    let run = |args: &[&str]| home.quadratique(elsewhere.path()).args(args).run();
    run(&["project", "create", "--key", "SOLO", "--name", "Solo"]).ok();
    run(&[
        "create",
        "--project",
        "SOLO",
        "--title",
        "Loose task",
        "--acceptance",
        "It is done.",
    ])
    .ok();

    let refused = run(&["claim", "--issue", "SOLO-1"]);

    assert_eq!(refused.code, 1, "{refused:?}");
    assert_eq!(
        refused.json(),
        serde_json::json!({ "_tag": "NoWorkingCopy" })
    );
}

#[test]
fn a_claim_on_the_command_line_names_the_worktree_it_runs_in_or_is_refused() {
    let home = Home::new();
    let repository = Repository::new();
    let worktree = repository.worktree("dem-12");
    home.quadratique(&repository.root)
        .args(["project", "create", "--key", "DEM", "--name", "Demo"])
        .run()
        .ok();
    let issue = home
        .quadratique(&repository.root)
        .args(["create", "--title", "Named", "--acceptance", "It is done."])
        .run()
        .ok()["issue"]["id"]
        .as_str()
        .expect("an issue id")
        .to_owned();
    let path = worktree.display().to_string();
    let claim_from = |directory: &std::path::Path| {
        home.quadratique(directory)
            .env("AI_AGENT", "codex")
            .args(["claim", "--issue", &issue, "--worktree", &path])
            .run()
    };

    let refused = claim_from(&repository.root);
    assert_eq!(refused.code, 1, "{refused:?}");
    assert_eq!(refused.json()["_tag"], "NoWorkingCopy", "{refused:?}");
    let untouched = home
        .quadratique(&repository.root)
        .args(["show", "--issue", &issue])
        .run()
        .ok();
    assert!(untouched["issue"]["claim"].is_null(), "{untouched}");

    let claimed = claim_from(&worktree).ok();
    let held = &claimed["issue"]["claim"];
    assert_eq!(held["worktree"], path, "{claimed}");
    assert_eq!(held["branch"], "dem-12", "{claimed}");
    assert_eq!(held["holder"]["name"], "codex@dem-12", "{claimed}");
}

#[test]
fn claim_mine_lists_the_claims_of_the_callers_session_in_every_worktree() {
    let home = Home::new();
    let repository = Repository::new();
    let lead = repository.worktree("lead");
    let audit = repository.worktree("audit");
    let other = repository.worktree("other");
    home.quadratique(&repository.root)
        .args(["project", "create", "--key", "SES", "--name", "Sessions"])
        .run()
        .ok();
    for title in ["Here", "There", "Theirs"] {
        home.quadratique(&repository.root)
            .args(["create", "--title", title, "--acceptance", "It is done."])
            .run()
            .ok();
    }
    let claim = |worktree: &std::path::Path, session: &str, issue: &str| {
        home.quadratique(worktree)
            .env("CLAUDECODE", "1")
            .env("CLAUDE_CODE_SESSION_ID", session)
            .args(["claim", "--issue", issue])
            .run()
            .ok();
    };
    claim(&lead, "session-1", "SES-1");
    claim(&audit, "session-1", "SES-2");
    claim(&other, "session-2", "SES-3");

    let mine = home
        .quadratique(&lead)
        .env("CLAUDECODE", "1")
        .env("CLAUDE_CODE_SESSION_ID", "session-1")
        .args(["issue", "list", "--claim", "mine"])
        .run()
        .ok();

    let listed: Vec<_> = mine["items"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|item| (item["id"].clone(), item["claim"]["worktree"].clone()))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("SES-1".into(), lead.display().to_string().into()),
            ("SES-2".into(), audit.display().to_string().into()),
        ]
    );
}
