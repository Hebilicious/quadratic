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
    }
    assert_eq!(read(root, SKILL_PATH), changed);
    assert_eq!(read(root, QUADRATIC_SKILL_PATH), quadratic_changed);
    assert!(answer.get("commit").is_none(), "{answer}");
    assert_eq!(git_stdout(root, &["status", "--porcelain"]), "");
}
