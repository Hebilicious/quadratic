//! Which repositories of the server's projects hold a skill older than this build, and what to
//! run in each. It reads each project's main checkout as `init --dry-run` does and writes into no
//! repository: a repository may take changes only through a branch and a pull request.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use acid_cat_core::Outcome;
use quadratique_server::domain::Binding;
use quadratique_server::git::environment;
use serde_json::{Value, json};

use crate::init::{setup_of, shell_word};
use crate::{background, caller, commands};

const NOTE: &str = "Nothing was written into these repositories. Run each command in the repository, or in the worktree of a new branch where it takes changes through pull requests; init prints the commit to make.";
const NO_SKILL: &str = "The main checkout holds no Quadratique skill. When another branch or worktree holds it, run quadratique init --dry-run there.";

pub async fn report() -> Result<Value, String> {
    let home = commands::home()?;
    let runtime = background::ensure(&home)
        .await
        .map_err(|error| error.to_string())?;
    let caller = caller::detect(Binding::Cli).map_err(|error| error.to_string())?;
    let client = caller::client(&caller, &runtime.url)?;
    let projects = match client.invoke("project.list", &json!({})).await {
        Outcome::Value(value) => value,
        Outcome::Failure(failure) => return Err(failure.to_string()),
        Outcome::CallFailure(failure) => return Err(failure.message),
    };
    let repositories = projects["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|project| project["repository"].as_str().map(PathBuf::from));
    Ok(report_of(repositories))
}

fn report_of(repositories: impl Iterator<Item = PathBuf>) -> Value {
    let mut current = Vec::new();
    let mut older = Vec::new();
    let mut kept = Vec::new();
    let mut no_skill = Vec::new();
    for repository in repositories.filter(|repository| repository.exists()) {
        let Some(checkout) = main_checkout(&repository) else {
            no_skill.push(json!(repository));
            continue;
        };
        let shown = checkout.display().to_string();
        let setup = match setup_of(&checkout) {
            Ok(setup) => setup,
            Err(problem) => {
                kept.push(json!({ "repository": shown, "warnings": [problem] }));
                continue;
            }
        };
        if !setup.present {
            no_skill.push(json!(shown));
            continue;
        }
        if setup.older {
            older.push(json!({
                "repository": shown,
                "run": format!("cd {} && quadratique init", shell_word(&shown)),
            }));
        } else if setup.warnings.is_empty() {
            current.push(json!(shown));
        }
        if !setup.warnings.is_empty() {
            kept.push(json!({ "repository": shown, "warnings": setup.warnings }));
        }
    }
    let mut report = json!({ "current": current, "older": older });
    if !older.is_empty() {
        report["note"] = json!(NOTE);
    }
    if !kept.is_empty() {
        report["kept"] = json!(kept);
    }
    if !no_skill.is_empty() {
        report["noSkill"] = json!({ "repositories": no_skill, "note": NO_SKILL });
    }
    report
}

/// The main worktree of a repository, from its git directory; `None` for a bare repository.
fn main_checkout(repository: &Path) -> Option<PathBuf> {
    let output = environment::without_trace(&mut Command::new("git"))
        .arg("--git-dir")
        .arg(repository)
        .args(["worktree", "list", "--porcelain"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let first: Vec<&str> = text.split("\n\n").next()?.lines().collect();
    if first.contains(&"bare") {
        return None;
    }
    let path = PathBuf::from(first.first()?.strip_prefix("worktree ")?);
    path.is_dir().then_some(path)
}
