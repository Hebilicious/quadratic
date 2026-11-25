use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::support::{Home, Repository, git, git_stdout};

const SKILL: &str = include_str!("../../../skill/quadratique/SKILL.md");
const PLANNING: &str = include_str!("../../../skill/quadratique/planning.md");
const LINE: &str = "We use Quadratique for issues: every task is an issue there. Use the `quadratique` skill (`.agents/skills/quadratique/SKILL.md`) for anything about tasks.";
const QUADRATIC_LINE: &str = "We use Quadratic for issues: every task is an issue there. Use the `quadratic` skill (`.agents/skills/quadratic/SKILL.md`) for anything about tasks.";
const SKILL_PATH: &str = ".agents/skills/quadratique/SKILL.md";
const PLANNING_PATH: &str = ".agents/skills/quadratique/planning.md";
const QUADRATIC_SKILL_PATH: &str = ".agents/skills/quadratic/SKILL.md";
const QUADRATIC_PLANNING_PATH: &str = ".agents/skills/quadratic/planning.md";
const KEPT: &str = "different: yours is kept";

/// The skill texts of earlier builds, taken from this repository's history: the folder's name is
/// the name of the build and the commit that shipped the text.
fn earlier_versions(name: &str) -> Vec<(String, String, String)> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skill");
    let mut versions: Vec<(String, String, String)> = std::fs::read_dir(&fixtures)
        .expect("the fixtures of earlier skill texts")
        .map(|entry| entry.expect("a fixture").path())
        .filter(|path| {
            path.file_name()
                .and_then(|file| file.to_str())
                .and_then(|file| file.rsplit_once('-'))
                .is_some_and(|(build, _)| build == name)
        })
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(path.join("SKILL.md")).unwrap(),
                std::fs::read_to_string(path.join("planning.md")).unwrap(),
            )
        })
        .collect();
    versions.sort();
    versions
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(root: &Path, path: &str) -> String {
    std::fs::read_to_string(root.join(path)).unwrap()
}

fn commit_all(root: &Path) {
    git(root, &["add", "--all"]);
    git(
        root,
        &["commit", "--quiet", "--message", "set up by an older build"],
    );
}

fn run_commit(root: &Path, answer: &Value) {
    let command = answer["commit"].as_str().expect("a commit to run");
    let ran = std::process::Command::new("sh")
        .args(["-c", command])
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("sh runs");
    assert!(ran.status.success(), "{ran:?}");
}

fn snapshot(root: &Path) -> Vec<(PathBuf, String)> {
    let listed = git_stdout(root, &["ls-files", "--cached", "--others"]);
    let mut files: Vec<(PathBuf, String)> = listed
        .lines()
        .map(|path| (PathBuf::from(path), read(root, path)))
        .collect();
    files.sort();
    files
}

#[test]
fn every_earlier_quadratique_skill_is_reported_older_by_a_dry_run_and_replaced_by_init() {
    let home = Home::new();
    let versions = earlier_versions("quadratique");
    assert_eq!(versions.len(), 3, "{versions:?}");
    let started = Repository::new();
    home.quadratique(&started.root)
        .args(["project", "list"])
        .run()
        .ok();

    for (index, (version, skill, planning)) in versions.iter().enumerate() {
        let repository = Repository::new();
        let root = &repository.root;
        write(root, SKILL_PATH, skill);
        write(root, PLANNING_PATH, planning);
        write(root, "AGENTS.md", &format!("{LINE}\n"));
        commit_all(root);
        let key = format!("OLD{index}");
        let before = snapshot(root);

        let checked = home
            .quadratique(root)
            .args(["init", "--key", &key, "--dry-run"])
            .run()
            .ok();

        assert_eq!(checked["dryRun"], true, "{version}: {checked}");
        assert_eq!(
            checked["files"][SKILL_PATH], "would update: it holds an earlier Quadratique text",
            "{version}: {checked}"
        );
        assert_eq!(
            checked["files"][PLANNING_PATH], "unchanged",
            "{version}: {checked}"
        );
        assert_eq!(
            checked["files"]["AGENTS.md"], "unchanged",
            "{version}: {checked}"
        );
        assert_eq!(
            checked["project"]["wouldCreate"], true,
            "{version}: {checked}"
        );
        assert!(checked.get("commit").is_none(), "{version}: {checked}");
        assert_eq!(snapshot(root), before, "{version}: the dry run wrote");
        let listed = home.quadratique(root).args(["project", "list"]).run().ok();
        assert!(
            listed["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|project| project["key"] != key.as_str()),
            "{version}: the dry run created the project: {listed}"
        );

        let answer = home
            .quadratique(root)
            .args(["init", "--key", &key])
            .run()
            .ok();

        assert_eq!(
            answer["files"][SKILL_PATH], "updated: it held an earlier Quadratique text",
            "{version}: {answer}"
        );
        assert_eq!(read(root, SKILL_PATH), SKILL, "{version}");
        assert_eq!(read(root, PLANNING_PATH), PLANNING, "{version}");
        run_commit(root, &answer);
        assert_eq!(
            git_stdout(root, &["show", "--name-only", "--format=", "HEAD"]),
            SKILL_PATH,
            "{version}"
        );

        let again = home
            .quadratique(root)
            .args(["init", "--dry-run"])
            .run()
            .ok();
        assert_eq!(
            again["files"][SKILL_PATH], "unchanged",
            "{version}: {again}"
        );
    }
}

#[test]
fn every_quadratic_skill_and_line_moves_to_quadratique_and_the_printed_commit_records_the_move() {
    let home = Home::new();
    let versions = earlier_versions("quadratic");
    assert_eq!(versions.len(), 2, "{versions:?}");

    for (index, (version, skill, planning)) in versions.iter().enumerate() {
        let repository = Repository::new();
        let root = &repository.root;
        write(root, QUADRATIC_SKILL_PATH, skill);
        write(root, QUADRATIC_PLANNING_PATH, planning);
        write(
            root,
            "AGENTS.md",
            &format!("# Agents\n\n{QUADRATIC_LINE}\n\nRun the tests.\n"),
        );
        commit_all(root);
        let key = format!("OLD{index}");
        let before = snapshot(root);

        let checked = home
            .quadratique(root)
            .args(["init", "--key", &key, "--dry-run"])
            .run()
            .ok();

        assert_eq!(
            checked["files"][SKILL_PATH], "would create",
            "{version}: {checked}"
        );
        assert_eq!(
            checked["files"][PLANNING_PATH], "would create",
            "{version}: {checked}"
        );
        for path in [QUADRATIC_SKILL_PATH, QUADRATIC_PLANNING_PATH] {
            assert_eq!(
                checked["files"][path],
                "would remove: Quadratic's text, now in .agents/skills/quadratique/",
                "{version}: {checked}"
            );
        }
        assert_eq!(
            checked["files"]["AGENTS.md"], "would update: it holds Quadratic's line",
            "{version}: {checked}"
        );
        assert_eq!(snapshot(root), before, "{version}: the dry run wrote");

        let answer = home
            .quadratique(root)
            .args(["init", "--key", &key])
            .run()
            .ok();

        assert_eq!(
            answer["files"][SKILL_PATH], "created",
            "{version}: {answer}"
        );
        for path in [QUADRATIC_SKILL_PATH, QUADRATIC_PLANNING_PATH] {
            assert_eq!(
                answer["files"][path],
                "removed: Quadratic's text, now in .agents/skills/quadratique/",
                "{version}: {answer}"
            );
        }
        assert_eq!(
            answer["files"]["AGENTS.md"], "updated: it held Quadratic's line",
            "{version}: {answer}"
        );
        assert_eq!(read(root, SKILL_PATH), SKILL, "{version}");
        assert_eq!(read(root, PLANNING_PATH), PLANNING, "{version}");
        assert_eq!(
            read(root, "AGENTS.md"),
            format!("# Agents\n\n{LINE}\n\nRun the tests.\n"),
            "{version}"
        );
        assert!(!root.join(".agents/skills/quadratic").exists(), "{version}");
        run_commit(root, &answer);
        assert_eq!(
            git_stdout(root, &["show", "--name-status", "--format=", "HEAD"]),
            format!(
                "D\t{QUADRATIC_SKILL_PATH}\nD\t{QUADRATIC_PLANNING_PATH}\nA\t{SKILL_PATH}\nA\t{PLANNING_PATH}\nM\tAGENTS.md"
            ),
            "{version}"
        );
        assert_eq!(
            git_stdout(root, &["status", "--porcelain"]),
            "",
            "{version}"
        );
    }
}

#[test]
fn a_skill_file_a_person_changed_after_an_older_build_wrote_it_is_kept_and_reported_kept() {
    let home = Home::new();
    let repository = Repository::new();
    let root = &repository.root;
    let (_, older_skill, older_planning) = earlier_versions("quadratique").remove(0);
    let (_, quadratic_skill, _) = earlier_versions("quadratic").remove(0);
    let changed = format!("{older_skill}\nOur own rule: ask before a merge.\n");
    let quadratic_changed = format!("{quadratic_skill}\nOur own rule.\n");
    write(root, SKILL_PATH, &changed);
    write(root, PLANNING_PATH, &older_planning);
    write(root, QUADRATIC_SKILL_PATH, &quadratic_changed);
    write(root, "AGENTS.md", &format!("{LINE}\n"));
    commit_all(root);

    let checked = home
        .quadratique(root)
        .args(["init", "--key", "OWN", "--dry-run"])
        .run()
        .ok();
    let answer = home
        .quadratique(root)
        .args(["init", "--key", "OWN"])
        .run()
        .ok();

    for said in [&checked, &answer] {
        assert_eq!(said["files"][SKILL_PATH], KEPT, "{said}");
        assert_eq!(said["files"][QUADRATIC_SKILL_PATH], KEPT, "{said}");
        assert!(
            said["files"].get(QUADRATIC_PLANNING_PATH).is_none(),
            "{said}"
        );
        let warnings: Vec<&str> = said["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .map(|warning| warning.as_str().expect("a text"))
            .collect();
        assert_eq!(warnings.len(), 2, "{said}");
        assert!(
            warnings[0].starts_with(&format!("{SKILL_PATH} was kept"))
                && warnings[0].ends_with("quadratique init --print SKILL.md"),
            "{said}"
        );
        assert!(
            warnings[1].starts_with(&format!("{QUADRATIC_SKILL_PATH} was kept"))
                && warnings[1].contains("quadratique init --print SKILL.md"),
            "{said}"
        );
        assert_eq!(
            said.as_object().unwrap().keys().next().map(String::as_str),
            Some("warnings"),
            "the warnings come first: {said}"
        );
    }
    assert_eq!(read(root, SKILL_PATH), changed);
    assert_eq!(read(root, QUADRATIC_SKILL_PATH), quadratic_changed);
    assert!(answer.get("commit").is_none(), "{answer}");
    assert_eq!(git_stdout(root, &["status", "--porcelain"]), "");
}

#[test]
fn init_print_writes_the_build_text_of_each_file_as_it_is_and_refuses_an_unknown_name() {
    let home = Home::new();
    let elsewhere = crate::support::outside();

    for (name, text) in [
        ("SKILL.md", SKILL.to_owned()),
        ("planning.md", PLANNING.to_owned()),
        ("AGENTS.md", format!("{LINE}\n")),
    ] {
        let printed = home
            .quadratique(elsewhere.path())
            .args(["init", "--print", name])
            .run();
        assert_eq!(printed.code, 0, "{printed:?}");
        assert_eq!(printed.stdout, text, "{name}");
    }

    let refused = home
        .quadratique(elsewhere.path())
        .args(["init", "--print", "README.md"])
        .run();
    assert_ne!(refused.code, 0, "{refused:?}");
    assert!(refused.stdout.is_empty(), "{refused:?}");
    assert!(
        refused.stderr.contains("SKILL.md, planning.md, AGENTS.md")
            && refused.stderr.contains("README.md"),
        "{refused:?}"
    );
    assert!(home.servers().is_empty());
    assert!(!home.runtime_path().exists());
}

#[test]
fn a_repository_with_no_changed_file_gets_no_warnings() {
    let home = Home::new();
    let repository = Repository::new();

    let answer = home
        .quadratique(&repository.root)
        .args(["init", "--key", "NEW"])
        .run()
        .ok();

    assert!(answer.get("warnings").is_none(), "{answer}");
}

#[test]
fn in_quadratic_s_folder_a_link_and_a_person_s_own_file_are_never_removed() {
    let home = Home::new();
    let repository = Repository::new();
    let root = &repository.root;
    let (_, quadratic_skill, quadratic_planning) = earlier_versions("quadratic").remove(0);
    write(root, "docs/skill.md", &quadratic_skill);
    write(root, QUADRATIC_PLANNING_PATH, &quadratic_planning);
    write(root, ".agents/skills/quadratic/notes.md", "our notes\n");
    std::os::unix::fs::symlink("../../../docs/skill.md", root.join(QUADRATIC_SKILL_PATH)).unwrap();
    commit_all(root);

    let answer = home
        .quadratique(root)
        .args(["init", "--key", "LNK"])
        .run()
        .ok();

    assert_eq!(
        answer["files"][QUADRATIC_SKILL_PATH], "a link: kept",
        "{answer}"
    );
    assert_eq!(
        answer["files"][QUADRATIC_PLANNING_PATH],
        "removed: Quadratic's text, now in .agents/skills/quadratique/",
        "{answer}"
    );
    assert!(
        answer["warnings"][0]
            .as_str()
            .is_some_and(|warning| warning.starts_with(&format!("{QUADRATIC_SKILL_PATH} was kept"))),
        "{answer}"
    );
    assert!(
        std::fs::symlink_metadata(root.join(QUADRATIC_SKILL_PATH))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(read(root, "docs/skill.md"), quadratic_skill);
    assert_eq!(
        read(root, ".agents/skills/quadratic/notes.md"),
        "our notes\n"
    );
    assert!(!root.join(QUADRATIC_PLANNING_PATH).exists());
}

fn home_files(home: &Home) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(folder: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, files);
            } else {
                files.push((path.clone(), std::fs::read(&path).unwrap_or_default()));
            }
        }
    }
    let mut files = Vec::new();
    walk(home.path(), &mut files);
    files.sort();
    files
}

#[test]
fn a_dry_run_with_no_server_running_starts_none_and_writes_nothing_in_the_home_or_the_index() {
    let home = Home::new();
    let repository = Repository::new();
    let root = &repository.root;
    let (_, older_skill, planning) = earlier_versions("quadratique").remove(0);
    write(root, SKILL_PATH, &older_skill);
    write(root, PLANNING_PATH, &planning);
    write(root, "AGENTS.md", &format!("{LINE}\n"));
    commit_all(root);
    let touched = std::fs::File::options()
        .write(true)
        .open(root.join(PLANNING_PATH))
        .unwrap();
    touched
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
        .unwrap();
    drop(touched);
    let index = std::fs::read(root.join(".git/index")).unwrap();
    let worktree = snapshot(root);
    let home_before = home_files(&home);

    let checked = home
        .quadratique(root)
        .args(["init", "--dry-run"])
        .run()
        .ok();

    assert_eq!(
        checked["files"][SKILL_PATH], "would update: it holds an earlier Quadratique text",
        "{checked}"
    );
    assert_eq!(
        checked["project"],
        serde_json::json!({
            "lookedUp": false,
            "reason": "no Quadratique server runs, and a dry run starts none"
        })
    );
    assert!(home.servers().is_empty(), "the dry run started a server");
    assert_eq!(
        home_files(&home),
        home_before,
        "the dry run wrote in the home"
    );
    assert_eq!(
        std::fs::read(root.join(".git/index")).unwrap(),
        index,
        "the dry run rewrote the index"
    );
    assert_eq!(snapshot(root), worktree);
}

#[test]
fn an_update_replaces_the_file_so_a_hard_link_outside_the_repository_keeps_its_bytes() {
    let home = Home::new();
    let repository = Repository::new();
    let root = &repository.root;
    let (_, older_skill, planning) = earlier_versions("quadratique").remove(0);
    write(root, SKILL_PATH, &older_skill);
    write(root, PLANNING_PATH, &planning);
    let elsewhere = crate::support::outside();
    let outside = elsewhere.path().join("SKILL.md");
    std::fs::hard_link(root.join(SKILL_PATH), &outside).unwrap();

    let answer = home
        .quadratique(root)
        .args(["init", "--key", "HRD"])
        .run()
        .ok();

    assert_eq!(
        answer["files"][SKILL_PATH], "updated: it held an earlier Quadratique text",
        "{answer}"
    );
    assert_eq!(read(root, SKILL_PATH), SKILL);
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), older_skill);
}

#[test]
fn a_copy_of_the_current_skill_in_quadratic_s_folder_is_kept_not_removed() {
    let home = Home::new();
    let repository = Repository::new();
    let root = &repository.root;
    write(root, QUADRATIC_SKILL_PATH, SKILL);

    let answer = home
        .quadratique(root)
        .args(["init", "--key", "CUR"])
        .run()
        .ok();

    assert_eq!(answer["files"][QUADRATIC_SKILL_PATH], KEPT, "{answer}");
    assert_eq!(read(root, QUADRATIC_SKILL_PATH), SKILL);
    assert!(
        answer["warnings"][0]
            .as_str()
            .is_some_and(|warning| warning.contains(&format!(
                "merge what you need of {QUADRATIC_SKILL_PATH} into {SKILL_PATH}"
            ))),
        "{answer}"
    );
}

#[test]
fn only_the_standalone_agents_line_is_updated_and_a_second_run_changes_nothing() {
    let home = Home::new();
    let repository = Repository::new();
    let root = &repository.root;
    let text = format!(
        "# Agents\r\n\r\n> {QUADRATIC_LINE}\r\n\r\n```\r\n{QUADRATIC_LINE}\r\n```\r\n\r\n{QUADRATIC_LINE}  \r\n{QUADRATIC_LINE}\r\nend"
    );
    write(root, "AGENTS.md", &text);

    let first = home
        .quadratique(root)
        .args(["init", "--key", "AGT"])
        .run()
        .ok();
    let second = home.quadratique(root).args(["init"]).run().ok();

    assert_eq!(
        first["files"]["AGENTS.md"], "updated: it held Quadratic's line",
        "{first}"
    );
    assert_eq!(
        read(root, "AGENTS.md"),
        format!(
            "# Agents\r\n\r\n> {QUADRATIC_LINE}\r\n\r\n```\r\n{QUADRATIC_LINE}\r\n```\r\n\r\n{LINE}\r\nend"
        )
    );
    assert_eq!(second["files"]["AGENTS.md"], "unchanged", "{second}");
}

#[test]
fn removals_of_quadratic_s_files_stay_in_the_printed_commit_after_a_second_run() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let repository = Repository::new();
    let root = &repository.root;
    let (_, skill, planning) = earlier_versions("quadratic").remove(0);
    write(root, QUADRATIC_SKILL_PATH, &skill);
    write(root, QUADRATIC_PLANNING_PATH, &planning);
    write(root, "AGENTS.md", "# Agents\n");
    commit_all(root);
    let agents = root.join("AGENTS.md");
    std::fs::set_permissions(&agents, std::fs::Permissions::from_mode(0o444)).unwrap();

    let failed = home.quadratique(root).args(["init", "--key", "TWC"]).run();
    assert_ne!(failed.code, 0, "{failed:?}");
    assert!(!root.join(QUADRATIC_SKILL_PATH).exists());

    std::fs::set_permissions(&agents, std::fs::Permissions::from_mode(0o644)).unwrap();
    let first = home.quadratique(root).args(["init"]).run().ok();
    let second = home.quadratique(root).args(["init"]).run().ok();

    for answer in [&first, &second] {
        assert_eq!(
            answer["files"][QUADRATIC_SKILL_PATH], "removed earlier: the commit records it",
            "{answer}"
        );
    }
    run_commit(root, &second);
    assert_eq!(
        git_stdout(root, &["show", "--name-status", "--format=", "HEAD"]),
        format!(
            "D\t{QUADRATIC_SKILL_PATH}\nD\t{QUADRATIC_PLANNING_PATH}\nA\t{SKILL_PATH}\nA\t{PLANNING_PATH}\nM\tAGENTS.md"
        )
    );
    assert_eq!(git_stdout(root, &["status", "--porcelain"]), "");
}

#[test]
fn a_directory_a_link_to_one_and_a_dangling_link_in_quadratic_s_folder_are_kept_and_warned() {
    let home = Home::new();
    for (index, make) in [
        (|root: &Path| std::fs::create_dir_all(root.join(QUADRATIC_SKILL_PATH)).unwrap())
            as fn(&Path),
        |root| {
            std::fs::create_dir_all(root.join("docs")).unwrap();
            std::fs::create_dir_all(root.join(".agents/skills/quadratic")).unwrap();
            std::os::unix::fs::symlink("../../../docs", root.join(QUADRATIC_SKILL_PATH)).unwrap();
        },
        |root| {
            std::fs::create_dir_all(root.join(".agents/skills/quadratic")).unwrap();
            std::os::unix::fs::symlink("gone.md", root.join(QUADRATIC_SKILL_PATH)).unwrap();
        },
    ]
    .into_iter()
    .enumerate()
    {
        let repository = Repository::new();
        let root = &repository.root;
        make(root);

        let answer = home
            .quadratique(root)
            .args(["init", "--key", &format!("DIR{index}")])
            .run()
            .ok();

        let word = answer["files"][QUADRATIC_SKILL_PATH]
            .as_str()
            .unwrap_or_default();
        assert!(word.ends_with(": kept"), "{index}: {answer}");
        assert!(
            answer["warnings"][0].as_str().is_some_and(
                |warning| warning.starts_with(&format!("{QUADRATIC_SKILL_PATH} was kept"))
            ),
            "{index}: {answer}"
        );
        assert!(std::fs::symlink_metadata(root.join(QUADRATIC_SKILL_PATH)).is_ok());
    }
}

#[test]
fn a_dry_run_reports_a_read_only_skill_and_a_skill_that_is_not_utf8_is_kept_with_a_warning() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let repository = Repository::new();
    let root = &repository.root;
    let (_, older_skill, _) = earlier_versions("quadratique").remove(0);
    write(root, SKILL_PATH, &older_skill);
    std::fs::write(root.join(PLANNING_PATH), b"# planning \xff\n").unwrap();
    std::fs::set_permissions(
        root.join(SKILL_PATH),
        std::fs::Permissions::from_mode(0o444),
    )
    .unwrap();

    let checked = home
        .quadratique(root)
        .args(["init", "--key", "ROF", "--dry-run"])
        .run()
        .ok();
    let failed = home.quadratique(root).args(["init", "--key", "ROF"]).run();

    let skill_word = checked["files"][SKILL_PATH].as_str().unwrap_or_default();
    assert!(
        skill_word.starts_with("would fail: ") && skill_word.ends_with("is not writable"),
        "{checked}"
    );
    assert_eq!(checked["files"][PLANNING_PATH], KEPT, "{checked}");
    let warnings = checked["warnings"].to_string();
    assert!(
        warnings.contains(&format!("{SKILL_PATH} cannot be written"))
            && warnings.contains(&format!("{PLANNING_PATH} was kept")),
        "{checked}"
    );
    assert_ne!(failed.code, 0, "{failed:?}");
    assert_eq!(read(root, SKILL_PATH), older_skill);
}
