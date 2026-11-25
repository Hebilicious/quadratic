use crate::support::{Home, Repository, git};

const SKILL: &str = include_str!("../../../skill/quadratique/SKILL.md");
const PLANNING: &str = include_str!("../../../skill/quadratique/planning.md");
const LINE: &str = "We use Quadratique for issues: every task is an issue there. Use the `quadratique` skill (`.agents/skills/quadratique/SKILL.md`) for anything about tasks.";

#[test]
fn init_writes_both_skill_files_and_the_line_and_creates_the_bound_project_and_a_second_run_changes_nothing()
 {
    let home = Home::new();
    let repository = Repository::new();

    let first = home
        .quadratique(&repository.root)
        .args(["init", "--key", "TAL", "--name", "tally"])
        .run();
    assert_eq!(
        first.stdout.trim_end().lines().count(),
        1,
        "standard output is not a terminal, so the answer is one line: {first:?}"
    );
    let first = first.ok();

    assert_eq!(
        first["files"][".agents/skills/quadratique/SKILL.md"], "created",
        "{first}"
    );
    assert_eq!(
        first["files"][".agents/skills/quadratique/planning.md"], "created",
        "{first}"
    );
    assert_eq!(first["files"]["AGENTS.md"], "created", "{first}");
    assert_eq!(first["project"]["key"], "TAL", "{first}");
    assert_eq!(first["project"]["created"], true, "{first}");
    assert!(
        first["commit"]
            .as_str()
            .is_some_and(|commit| commit.starts_with("git -C ")),
        "{first}"
    );
    let skill = repository.root.join(".agents/skills/quadratique/SKILL.md");
    let agents = repository.root.join("AGENTS.md");
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), SKILL);
    assert_eq!(
        std::fs::read_to_string(
            repository
                .root
                .join(".agents/skills/quadratique/planning.md")
        )
        .unwrap(),
        PLANNING
    );
    assert_eq!(
        std::fs::read_to_string(&agents).unwrap(),
        format!("{LINE}\n")
    );
    let listed = home
        .quadratique(&repository.root)
        .args(["project", "list"])
        .run()
        .ok();
    assert_eq!(
        listed["items"][0]["repository"],
        repository.root.join(".git").display().to_string(),
        "{listed}"
    );
    let committed = sh(
        &repository.root,
        first["commit"].as_str().expect("a commit"),
    );
    assert!(committed.status.success(), "{committed:?}");

    let second = home.quadratique(&repository.root).args(["init"]).run().ok();

    assert_eq!(
        second["files"][".agents/skills/quadratique/SKILL.md"], "unchanged",
        "{second}"
    );
    assert_eq!(
        second["files"][".agents/skills/quadratique/planning.md"], "unchanged",
        "{second}"
    );
    assert_eq!(second["files"]["AGENTS.md"], "unchanged", "{second}");
    assert_eq!(
        second["project"],
        serde_json::json!({ "key": "TAL", "created": false, "defaultBranch": "main" })
    );
    assert!(second.get("commit").is_none(), "{second}");
    assert_eq!(
        std::fs::read_to_string(&agents).unwrap(),
        format!("{LINE}\n")
    );
    let listed = home
        .quadratique(&repository.root)
        .args(["project", "list"])
        .run()
        .ok();
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
}

#[test]
fn init_appends_the_line_to_an_agents_file_and_takes_the_key_from_the_folder_and_commits_nothing() {
    let home = Home::new();
    let repository = Repository::new();
    std::fs::write(
        repository.root.join("AGENTS.md"),
        "# Agents\nRun the tests.",
    )
    .unwrap();
    git(&repository.root, &["add", "AGENTS.md"]);
    git(
        &repository.root,
        &["commit", "--quiet", "--message", "agents"],
    );

    let answer = home.quadratique(&repository.root).args(["init"]).run().ok();

    assert_eq!(answer["files"]["AGENTS.md"], "changed", "{answer}");
    assert_eq!(answer["project"]["key"], "MAIN", "{answer}");
    assert_eq!(
        std::fs::read_to_string(repository.root.join("AGENTS.md")).unwrap(),
        format!("# Agents\nRun the tests.\n\n{LINE}\n")
    );
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(&repository.root)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&status.stdout),
        " M AGENTS.md\n?? .agents/skills/quadratique/SKILL.md\n?? .agents/skills/quadratique/planning.md\n"
    );
}

fn sh(directory: &std::path::Path, command: &str) -> std::process::Output {
    std::process::Command::new("sh")
        .args(["-c", command])
        .current_dir(directory)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("sh runs")
}

fn stdout_of(directory: &std::path::Path, command: &str) -> String {
    String::from_utf8(sh(directory, command).stdout).expect("text")
}

#[test]
fn the_printed_commit_commits_only_the_three_files_and_leaves_other_staged_work_staged() {
    let home = Home::new();
    let repository = Repository::new();
    std::fs::write(repository.root.join("notes.txt"), "staged, not mine\n").unwrap();
    git(&repository.root, &["add", "notes.txt"]);

    let answer = home.quadratique(&repository.root).args(["init"]).run().ok();
    let ran = sh(
        &repository.root,
        answer["commit"].as_str().expect("a commit"),
    );

    assert!(ran.status.success(), "{ran:?}");
    assert_eq!(
        stdout_of(&repository.root, "git show --name-only --format= HEAD"),
        ".agents/skills/quadratique/SKILL.md\n.agents/skills/quadratique/planning.md\nAGENTS.md\n"
    );
    assert_eq!(
        stdout_of(&repository.root, "git diff --cached --name-only"),
        "notes.txt\n"
    );
}

#[test]
fn a_skill_file_the_person_changed_is_left_as_it_is_and_reported_different() {
    let home = Home::new();
    let repository = Repository::new();
    let skill = repository.root.join(".agents/skills/quadratique/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, "my own words\n").unwrap();

    let answer = home.quadratique(&repository.root).args(["init"]).run().ok();

    assert_eq!(
        answer["files"][".agents/skills/quadratique/SKILL.md"], "different: yours is kept",
        "{answer}"
    );
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), "my own words\n");
    let ran = sh(
        &repository.root,
        answer["commit"].as_str().expect("a commit"),
    );
    assert!(ran.status.success(), "{ran:?}");
    assert_eq!(
        stdout_of(&repository.root, "git show --name-only --format= HEAD"),
        ".agents/skills/quadratique/planning.md\nAGENTS.md\n"
    );
}

#[test]
fn the_printed_commit_runs_from_a_subdirectory_and_reads_its_paths_literally() {
    let home = Home::new();
    let repository = Repository::new();
    let docs = repository.root.join("docs");
    std::fs::create_dir(&docs).unwrap();
    std::fs::write(docs.join("[ab].md"), "# Agents\n").unwrap();
    std::fs::write(docs.join("a.md"), "not for this commit\n").unwrap();
    std::os::unix::fs::symlink("docs/[ab].md", repository.root.join("AGENTS.md")).unwrap();

    let answer = home.quadratique(&repository.root).args(["init"]).run().ok();
    let ran = sh(&docs, answer["commit"].as_str().expect("a commit"));

    assert!(ran.status.success(), "{ran:?}");
    assert_eq!(
        stdout_of(&repository.root, "git show --name-only --format= HEAD"),
        ".agents/skills/quadratique/SKILL.md\n.agents/skills/quadratique/planning.md\ndocs/[ab].md\n"
    );
}

#[test]
fn an_agents_file_that_links_outside_the_repository_is_refused_before_any_change() {
    let home = Home::new();
    let repository = Repository::new();
    let outside = tempfile::tempdir().unwrap();
    let configuration = outside.path().join("CLAUDE.md");
    std::fs::write(&configuration, "an agent's configuration\n").unwrap();
    std::os::unix::fs::symlink(&configuration, repository.root.join("AGENTS.md")).unwrap();

    let refused = home.quadratique(&repository.root).args(["init"]).run();

    assert_ne!(refused.code, 0, "{refused:?}");
    assert_eq!(
        std::fs::read_to_string(&configuration).unwrap(),
        "an agent's configuration\n"
    );
    assert!(!repository.root.join(".agents").exists());
    let listed = home
        .quadratique(&repository.root)
        .args(["project", "list"])
        .run()
        .ok();
    assert_eq!(listed["items"], serde_json::json!([]), "{listed}");
}

#[test]
fn a_project_key_the_catalog_refuses_is_refused_before_any_change() {
    let home = Home::new();
    let repository = Repository::new();

    let refused = home
        .quadratique(&repository.root)
        .args(["init", "--key", "tal"])
        .run();

    assert_ne!(refused.code, 0, "{refused:?}");
    assert!(
        refused.stderr.contains("Nothing was changed"),
        "{refused:?}"
    );
    assert!(!repository.root.join(".agents").exists());
    assert!(!repository.root.join("AGENTS.md").exists());
    let listed = home
        .quadratique(&repository.root)
        .args(["project", "list"])
        .run()
        .ok();
    assert_eq!(listed["items"], serde_json::json!([]), "{listed}");
}

#[test]
fn a_project_key_that_another_repository_has_is_refused_before_any_change() {
    let home = Home::new();
    let first = Repository::new();
    let second = Repository::new();
    home.quadratique(&first.root)
        .args(["init", "--key", "TAL"])
        .run()
        .ok();

    let refused = home
        .quadratique(&second.root)
        .args(["init", "--key", "TAL"])
        .run();

    assert_ne!(refused.code, 0, "{refused:?}");
    assert!(
        refused.stderr.contains("Nothing was changed"),
        "{refused:?}"
    );
    assert!(!second.root.join(".agents").exists());
    assert!(!second.root.join("AGENTS.md").exists());
}

/// macOS file systems refuse a name that is not UTF-8 (`EILSEQ`), so the path cannot exist there.
#[cfg(target_os = "linux")]
#[test]
fn an_agents_file_that_leads_to_a_path_that_is_not_utf8_is_refused_before_any_change() {
    use std::os::unix::ffi::OsStrExt;
    let home = Home::new();
    let repository = Repository::new();
    let target = repository
        .root
        .join(std::ffi::OsStr::from_bytes(b"agents-\xff.md"));
    std::fs::write(&target, "# Agents\n").unwrap();
    std::os::unix::fs::symlink(&target, repository.root.join("AGENTS.md")).unwrap();

    let refused = home.quadratique(&repository.root).args(["init"]).run();

    assert_ne!(refused.code, 0, "{refused:?}");
    assert!(
        refused.stderr.contains("Nothing was changed"),
        "{refused:?}"
    );
    assert!(!repository.root.join(".agents").exists());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "# Agents\n");
}

#[test]
fn an_agents_file_that_links_to_the_skills_path_is_refused_before_any_change() {
    let home = Home::new();
    let repository = Repository::new();
    std::os::unix::fs::symlink(
        ".agents/skills/quadratique/SKILL.md",
        repository.root.join("AGENTS.md"),
    )
    .unwrap();

    let refused = home.quadratique(&repository.root).args(["init"]).run();

    assert_ne!(refused.code, 0, "{refused:?}");
    assert!(
        refused.stderr.contains("Nothing was changed"),
        "{refused:?}"
    );
    assert!(!repository.root.join(".agents").exists());
    let listed = home
        .quadratique(&repository.root)
        .args(["project", "list"])
        .run()
        .ok();
    assert_eq!(listed["items"], serde_json::json!([]), "{listed}");
}

#[test]
fn an_agents_file_that_links_inside_the_repository_is_written_through() {
    let home = Home::new();
    let repository = Repository::new();
    std::fs::create_dir(repository.root.join("docs")).unwrap();
    std::fs::write(repository.root.join("docs/AGENTS.md"), "# Agents\n").unwrap();
    std::os::unix::fs::symlink("docs/AGENTS.md", repository.root.join("AGENTS.md")).unwrap();

    let answer = home.quadratique(&repository.root).args(["init"]).run().ok();

    assert_eq!(answer["files"]["AGENTS.md"], "changed", "{answer}");
    assert_eq!(
        std::fs::read_to_string(repository.root.join("docs/AGENTS.md")).unwrap(),
        format!("# Agents\n\n{LINE}\n")
    );
    assert!(
        std::fs::symlink_metadata(repository.root.join("AGENTS.md"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn when_the_last_write_fails_the_answer_says_what_was_done_and_a_second_run_finishes() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let repository = Repository::new();
    let agents = repository.root.join("AGENTS.md");
    std::fs::write(&agents, "# Agents\n").unwrap();
    std::fs::set_permissions(&agents, std::fs::Permissions::from_mode(0o444)).unwrap();

    let failed = home
        .quadratique(&repository.root)
        .args(["init", "--key", "TAL"])
        .run();

    assert_ne!(failed.code, 0, "{failed:?}");
    let said = format!("{}{}", failed.stdout, failed.stderr);
    assert!(
        said.contains("wrote .agents/skills/quadratique/SKILL.md")
            && said.contains("created the project TAL"),
        "{said}"
    );
    assert_eq!(std::fs::read_to_string(&agents).unwrap(), "# Agents\n");

    std::fs::set_permissions(&agents, std::fs::Permissions::from_mode(0o644)).unwrap();
    let second = home.quadratique(&repository.root).args(["init"]).run().ok();
    assert_eq!(second["files"]["AGENTS.md"], "changed", "{second}");
    assert_eq!(second["project"]["created"], false, "{second}");
    let ran = sh(
        &repository.root,
        second["commit"].as_str().expect("a commit"),
    );
    assert!(ran.status.success(), "{ran:?}");
    assert_eq!(
        stdout_of(&repository.root, "git show --name-only --format= HEAD"),
        ".agents/skills/quadratique/SKILL.md\n.agents/skills/quadratique/planning.md\nAGENTS.md\n"
    );
}

#[test]
fn the_default_branch_comes_from_the_flag_else_from_the_branch_of_the_main_worktree() {
    let home = Home::new();
    let repository = Repository::new();
    git(&repository.root, &["branch", "-m", "main", "master"]);

    let derived = home.quadratique(&repository.root).args(["init"]).run().ok();

    assert_eq!(derived["project"]["defaultBranch"], "master", "{derived}");
    assert_eq!(
        derived["project"]["defaultBranchFrom"], "the branch checked out in the main worktree",
        "{derived}"
    );

    let other = Repository::new();
    let flagged = home
        .quadratique(&other.root)
        .args(["init", "--key", "OTH", "--default-branch", "trunk"])
        .run()
        .ok();
    assert_eq!(flagged["project"]["defaultBranch"], "trunk", "{flagged}");

    let by_hand = Repository::new();
    git(&by_hand.root, &["branch", "-m", "main", "master"]);
    let created = home
        .quadratique(&by_hand.root)
        .args(["project", "create", "--key", "HND", "--name", "by hand"])
        .run()
        .ok();
    assert_eq!(created["defaultBranch"], "master", "{created}");
}
