//! Rule 4 of `docs/decisions/0002-claims-of-agents-that-ended.md` on the command line: an agent
//! moves an issue assigned to the human with the human's words and its confidence.

use crate::support::{Home, Repository};

const WORDS: &str = "Yes, MIT. Close it.";

#[test]
fn rule_4_an_agent_on_the_command_line_moves_an_issue_assigned_to_the_human_only_at_high_confidence()
 {
    let home = Home::new();
    let repository = Repository::new();
    home.quadratique(&repository.root)
        .args(["project", "create", "--key", "DEM", "--name", "Demo"])
        .run()
        .ok();
    let created = home
        .quadratique(&repository.root)
        .args([
            "create",
            "--title",
            "Choose the license",
            "--acceptance",
            "The human chose a license.",
            "--assignee",
            r#"{"kind":"human"}"#,
        ])
        .run()
        .ok();
    let issue = created["issue"]["id"].as_str().expect("an issue id");
    let agent = |args: &[&str]| {
        home.quadratique(&repository.root)
            .env("CLAUDECODE", "1")
            .args(args)
            .run()
    };
    let revision = || {
        home.quadratique(&repository.root)
            .args(["show", "--issue", issue])
            .run()
            .ok()["issue"]["revision"]
            .to_string()
    };

    let refused = agent(&[
        "issue",
        "update",
        "--issue",
        issue,
        "--revision",
        &revision(),
        "--status",
        "done",
    ]);
    assert_eq!(refused.code, 1, "{refused:?}");
    assert_eq!(refused.json()["_tag"], "HumanOnly", "{refused:?}");

    let left = agent(&[
        "issue",
        "update",
        "--issue",
        issue,
        "--revision",
        &revision(),
        "--status",
        "done",
        "--humanWords",
        WORDS,
        "--confidence",
        "medium",
    ])
    .ok();
    assert_eq!(left["leftForHuman"], true, "{left}");
    assert_eq!(left["issue"]["status"], "todo", "{left}");

    let moved = agent(&[
        "issue",
        "update",
        "--issue",
        issue,
        "--revision",
        &revision(),
        "--status",
        "done",
        "--humanWords",
        WORDS,
        "--confidence",
        "high",
    ])
    .ok();
    assert_eq!(moved["issue"]["status"], "done", "{moved}");

    let too_long = "w".repeat(401);
    let refused = agent(&[
        "issue",
        "update",
        "--issue",
        issue,
        "--revision",
        &revision(),
        "--humanWords",
        &too_long,
        "--confidence",
        "high",
    ]);
    assert_ne!(refused.code, 0, "{refused:?}");
    let refused = agent(&[
        "issue",
        "update",
        "--issue",
        issue,
        "--revision",
        &revision(),
        "--humanWords",
        WORDS,
        "--confidence",
        "certain",
    ]);
    assert_ne!(refused.code, 0, "{refused:?}");
}

#[test]
fn rule_4_the_skill_says_the_words_are_what_the_human_said_to_the_agent_in_this_session() {
    let skill = include_str!("../../../skill/quadratique/SKILL.md");
    let line = skill
        .lines()
        .find(|line| line.contains("--humanWords"))
        .expect("the skill's line about the human's words");
    assert!(line.contains("said to you in this session"), "{line}");
}

#[test]
fn rule_4_the_old_name_issue_close_takes_the_humans_words_as_issue_finish_does() {
    let home = Home::new();
    let repository = Repository::new();
    let worktree = repository.worktree("audit");
    home.quadratique(&repository.root)
        .args(["project", "create", "--key", "DEM", "--name", "Demo"])
        .run()
        .ok();
    let decision = |title: &str| {
        let issue = home
            .quadratique(&repository.root)
            .args(["create", "--title", title, "--acceptance", "It is done."])
            .run()
            .ok()["issue"]["id"]
            .as_str()
            .expect("an issue id")
            .to_owned();
        home.quadratique(&worktree)
            .env("CLAUDECODE", "1")
            .args(["claim", "--issue", &issue])
            .run()
            .ok();
        let revision = home
            .quadratique(&repository.root)
            .args(["show", "--issue", &issue])
            .run()
            .ok()["issue"]["revision"]
            .to_string();
        home.quadratique(&repository.root)
            .args([
                "issue",
                "update",
                "--issue",
                &issue,
                "--revision",
                &revision,
                "--assignee",
                r#"{"kind":"human"}"#,
            ])
            .run()
            .ok();
        issue
    };
    let finish = |verb: &str, issue: &str| {
        home.quadratique(&worktree)
            .env("CLAUDECODE", "1")
            .args([
                "issue",
                verb,
                "--issue",
                issue,
                "--evidence",
                "read it",
                "--humanWords",
                WORDS,
                "--confidence",
                "high",
            ])
            .run()
            .ok()
    };

    let closed = finish("close", &decision("Through the old name"));
    let finished = finish("finish", &decision("Through the new name"));

    for answer in [&closed, &finished] {
        assert_eq!(answer["issue"]["status"], "in-review", "{answer}");
        let note = answer["issue"]["notes"][0]["text"]
            .as_str()
            .expect("a note");
        assert!(
            note.contains(&format!("with high confidence: \"{WORDS}\"")),
            "{note}"
        );
    }
}
