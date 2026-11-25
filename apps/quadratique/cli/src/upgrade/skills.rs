//! Which repositories of the home's projects hold a skill older than this build, and what to run
//! in each. The projects are read from the store file, read-only: no server is asked or started,
//! and no store is created or migrated. Each project's main checkout is read as
//! `init --dry-run` reads it, and no repository is written: a repository may take changes only
//! through a branch and a pull request.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use quadratique_server::git::environment;
use quadratique_server::store::{SCHEMA_VERSION, read_recorded};
use serde_json::{Value, json};

use crate::commands;
use crate::init::{setup_of, shell_word};

const NOTE: &str = "Nothing was written into these repositories. Run each command in the repository, or in the worktree of a new branch where it takes changes through pull requests; init prints the commit to make.";
const NO_SKILL: &str = "The main checkout holds no Quadratique skill. When another branch or worktree holds it, run quadratique init --dry-run there.";

pub fn report() -> Value {
    let repositories = match projects() {
        Ok(repositories) => repositories,
        Err(problem) => {
            return json!({
                "error": format!("the projects could not be listed: {problem}"),
                "run": "quadratique upgrade --skills",
            });
        }
    };
    report_of(repositories)
}

fn projects() -> Result<Vec<PathBuf>, String> {
    let home = commands::home()?;
    let database = &home.paths.database;
    match read_recorded(database).map_err(|error| error.to_string())? {
        None => Ok(Vec::new()),
        Some(recorded) if recorded.schema > SCHEMA_VERSION => Err(format!(
            "the store {} has schema {}, newer than this build knows ({SCHEMA_VERSION})",
            database.display(),
            recorded.schema
        )),
        Some(recorded) => Ok(recorded.repositories),
    }
}

#[derive(Default)]
struct Report {
    current: Vec<Value>,
    older: Vec<Value>,
    kept: Vec<Value>,
    no_skill: Vec<Value>,
    bare: Vec<Value>,
    missing: Vec<Value>,
    errors: Vec<Value>,
}

fn report_of(repositories: Vec<PathBuf>) -> Value {
    let mut report = Report::default();
    for repository in repositories {
        if !repository.exists() {
            report.missing.push(json!(repository));
            continue;
        }
        let checkout = match main_checkout(&repository) {
            Ok(Some(checkout)) => checkout,
            Ok(None) => {
                report.bare.push(json!(repository));
                continue;
            }
            Err(problem) => {
                report
                    .errors
                    .push(json!({ "repository": repository, "error": problem }));
                continue;
            }
        };
        let shown = checkout.display().to_string();
        let setup = match setup_of(&checkout) {
            Ok(setup) => setup,
            Err(problem) => {
                report
                    .errors
                    .push(json!({ "repository": shown, "error": problem }));
                continue;
            }
        };
        if !setup.errors.is_empty() {
            report
                .errors
                .push(json!({ "repository": shown, "errors": setup.errors }));
        }
        if !setup.warnings.is_empty() {
            report
                .kept
                .push(json!({ "repository": shown, "warnings": setup.warnings }));
        }
        if setup.older {
            report.older.push(json!({
                "repository": shown,
                "run": format!("cd {} && quadratique init", shell_word(&shown)),
            }));
        } else if !setup.present && setup.errors.is_empty() {
            report.no_skill.push(json!(shown));
        } else if setup.present && setup.warnings.is_empty() && setup.errors.is_empty() {
            report.current.push(json!(shown));
        }
    }
    let mut answer = json!({ "current": report.current, "older": report.older });
    if !report.older.is_empty() {
        answer["note"] = json!(NOTE);
    }
    for (field, entries) in [
        ("kept", report.kept),
        ("bare", report.bare),
        ("missing", report.missing),
        ("errors", report.errors),
    ] {
        if !entries.is_empty() {
            answer[field] = json!(entries);
        }
    }
    if !report.no_skill.is_empty() {
        answer["noSkill"] = json!({ "repositories": report.no_skill, "note": NO_SKILL });
    }
    answer
}

/// The main worktree of a repository, from its git directory; `None` for a bare repository.
fn main_checkout(repository: &Path) -> Result<Option<PathBuf>, String> {
    let output = environment::without_trace(&mut Command::new("git"))
        .arg("--git-dir")
        .arg(repository)
        .args(["worktree", "list", "--porcelain", "-z"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("git does not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    main_of(&output.stdout)
}

/// The first record of `git worktree list --porcelain -z`: fields end with NUL, and an empty
/// field ends the record.
fn main_of(listed: &[u8]) -> Result<Option<PathBuf>, String> {
    use std::os::unix::ffi::OsStrExt;

    let fields: Vec<&[u8]> = listed
        .split(|byte| *byte == 0)
        .take_while(|field| !field.is_empty())
        .collect();
    if fields.contains(&&b"bare"[..]) {
        return Ok(None);
    }
    let path = fields
        .first()
        .and_then(|field| field.strip_prefix(b"worktree "))
        .map(|path| PathBuf::from(std::ffi::OsStr::from_bytes(path)))
        .ok_or("git worktree list named no main worktree")?;
    if !path.is_dir() {
        return Err(format!("the main worktree {} is missing", path.display()));
    }
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::main_of;

    #[test]
    fn a_main_worktree_whose_path_holds_a_newline_is_read_whole() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("two\nlines");
        std::fs::create_dir(&path).unwrap();
        let listed = format!(
            "worktree {}\0HEAD 0123\0branch refs/heads/main\0\0worktree /elsewhere\0HEAD 4567\0\0",
            path.display()
        );
        assert_eq!(main_of(listed.as_bytes()).unwrap(), Some(path));
        assert_eq!(main_of(b"worktree /repo.git\0bare\0\0").unwrap(), None);
    }
}
