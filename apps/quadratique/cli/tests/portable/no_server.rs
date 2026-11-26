//! Commands that answer without a server: they must neither start one nor write in the home.

use std::path::Path;
use std::process::{Command, Output};

const BINARY: &str = env!("CARGO_BIN_EXE_quadratique");
const SKILL: &str = include_str!("../../../skill/quadratique/SKILL.md");
const PLANNING: &str = include_str!("../../../skill/quadratique/planning.md");
const LINE: &str = "We use Quadratique for issues: every task is an issue there. Use the `quadratique` skill (`.agents/skills/quadratique/SKILL.md`) for anything about tasks.";

fn quadratique(home: &Path, args: &[&str]) -> Output {
    let elsewhere = tempfile::tempdir().expect("a folder outside any repository");
    Command::new(BINARY)
        .env("QUADRATIQUE_HOME", home)
        .current_dir(elsewhere.path())
        .args(args)
        .output()
        .expect("quadratique runs")
}

fn assert_no_server(home: &Path) {
    for file in ["server.json", "workspace.db", "server.lock"] {
        assert!(!home.join(file).exists(), "{file} exists in the home");
    }
}

#[test]
fn init_print_writes_the_build_text_of_each_file_as_it_is_and_refuses_an_unknown_name() {
    let home = tempfile::tempdir().expect("a temporary home");

    for (name, text) in [
        ("SKILL.md", SKILL.to_owned()),
        ("planning.md", PLANNING.to_owned()),
        ("AGENTS.md", format!("{LINE}\n")),
    ] {
        let printed = quadratique(home.path(), &["init", "--print", name]);
        assert!(printed.status.success(), "{printed:?}");
        assert_eq!(String::from_utf8_lossy(&printed.stdout), text, "{name}");
    }

    let refused = quadratique(home.path(), &["init", "--print", "README.md"]);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(refused.stdout.is_empty(), "{refused:?}");
    assert!(
        stderr.contains("SKILL.md, planning.md, AGENTS.md") && stderr.contains("README.md"),
        "{stderr}"
    );
    assert_no_server(home.path());
}

#[test]
fn serve_refuses_a_port_that_is_not_a_number() {
    let home = tempfile::tempdir().expect("a temporary home");

    let run = quadratique(home.path(), &["serve", "--port", "web"]);

    assert!(!run.status.success(), "{run:?}");
    assert_no_server(home.path());
}

/// On Windows, `serve` refuses before it creates anything: the data folder is not even made.
#[cfg(windows)]
#[test]
fn serve_on_windows_refuses_before_it_creates_the_data_folder() {
    let parent = tempfile::tempdir().expect("a temporary folder");
    let home = parent.path().join("home");

    let run = quadratique(&home, &["serve"]);

    assert!(!run.status.success(), "{run:?}");
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("QUA-61"),
        "{run:?}"
    );
    assert!(
        !home.exists(),
        "serve created the data folder before it refused"
    );
}
