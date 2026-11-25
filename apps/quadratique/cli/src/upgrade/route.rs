//! How the running binary was installed, told from where its file is, never from the
//! environment:
//!
//! - mise: the file is inside `<data>/installs/<tool>/<version>/`, and `<tool>/.mise.backend.toml`,
//!   which mise writes for each tool, names this repository (`github:Hebilicious/quadratique`) in a
//!   backend whose folder name is `<tool>`;
//! - cargo: the file is `<root>/bin/quadratique`, and `<root>/.crates2.json`, which `cargo install`
//!   and `cargo binstall` keep, lists a `quadratique` package with that binary;
//! - else a file that `quadratique upgrade` may replace.
//!
//! When there is a sign of a manager that does not hold up (a mise marker that cannot be read or
//! names another tool, a cargo record that cannot be read), the route is unclear: `upgrade`
//! replaces nothing and says why.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::init::shell_word;

const REPOSITORY: &str = "hebilicious/quadratique";

pub enum Route {
    File,
    Mise { tool: String },
    Cargo { root: PathBuf },
    Unclear(String),
}

impl Route {
    pub fn of(binary: &Path) -> Route {
        if let Some(route) = mise(binary) {
            return route;
        }
        if let Some(route) = cargo(binary) {
            return route;
        }
        Route::File
    }

    pub fn manager(&self) -> Option<&'static str> {
        match self {
            Route::File | Route::Unclear(_) => None,
            Route::Mise { .. } => Some("mise"),
            Route::Cargo { .. } => Some("cargo"),
        }
    }

    /// The command that upgrades the binary of a manager's route.
    pub fn command(&self) -> String {
        match self {
            Route::File | Route::Unclear(_) => "quadratique upgrade".to_owned(),
            Route::Mise { tool } => format!("mise upgrade --bump {tool}"),
            Route::Cargo { root } => format!(
                "cargo install --root {} quadratique",
                shell_word(&root.display().to_string())
            ),
        }
    }
}

/// The folder name mise gives a tool: `github:Hebilicious/quadratique` is
/// `github-hebilicious-quadratique`.
fn mise_folder(tool: &str) -> String {
    tool.to_ascii_lowercase().replace([':', '/'], "-")
}

fn mise(binary: &Path) -> Option<Route> {
    let tool_folder = binary
        .ancestors()
        .skip(2)
        .find(|folder| folder.parent().and_then(Path::file_name) == Some("installs".as_ref()))?;
    let marker = tool_folder.join(".mise.backend.toml");
    let under_mise = tool_folder
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        == Some("mise".as_ref());
    if !marker.exists() && !under_mise {
        return None;
    }
    let unclear = |why: String| Some(Route::Unclear(why));
    let text = match std::fs::read_to_string(&marker) {
        Ok(text) => text,
        Err(error) => {
            return unclear(format!(
                "{} looks installed by mise, but {} cannot be read ({error})",
                binary.display(),
                marker.display()
            ));
        }
    };
    let field = |name: &str| {
        text.lines().find_map(|line| {
            let value = line.strip_prefix(name)?.trim_start().strip_prefix('=')?;
            Some(
                value
                    .trim()
                    .strip_prefix('"')?
                    .strip_suffix('"')?
                    .to_owned(),
            )
        })
    };
    let names_this = |tool: &str| {
        tool.split_once(':')
            .is_some_and(|(_, name)| name.eq_ignore_ascii_case(REPOSITORY))
    };
    let short = field("short").unwrap_or_default();
    let full = field("full").unwrap_or_default();
    let named = tool_folder
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == mise_folder(&short));
    let safe = short
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b":/@._-".contains(&byte));
    if names_this(&short) && names_this(&full) && named && safe {
        Some(Route::Mise { tool: short })
    } else {
        unclear(format!(
            "{} is inside the mise folder {}, whose {} names {short:?}, not this repository's tool: the binary was left as it is",
            binary.display(),
            tool_folder.display(),
            marker.display()
        ))
    }
}

fn cargo(binary: &Path) -> Option<Route> {
    let bin = binary.parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    let root = bin.parent()?;
    let record = root.join(".crates2.json");
    if !record.exists() && !root.join(".crates.toml").exists() {
        return None;
    }
    let unclear = |why: &str| {
        Some(Route::Unclear(format!(
            "{} is in a cargo install root, but {} {why}",
            binary.display(),
            record.display()
        )))
    };
    let Ok(text) = std::fs::read(&record) else {
        return unclear("cannot be read");
    };
    let Ok(record) = serde_json::from_slice::<Value>(&text) else {
        return unclear("is not the JSON that cargo writes");
    };
    let name = binary.file_name()?.to_str()?;
    let listed = record["installs"]
        .as_object()
        .into_iter()
        .flatten()
        .any(|(package, install)| {
            package.split(' ').next() == Some("quadratique")
                && install["bins"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|bin| bin.as_str() == Some(name))
        });
    listed.then(|| Route::Cargo {
        root: root.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::Route;

    const MARKER: &str =
        "short = \"github:Hebilicious/quadratique\"\nfull = \"github:Hebilicious/quadratique\"\n";

    fn binary_in(folder: &std::path::Path, relative: &str) -> std::path::PathBuf {
        let binary = folder.join(relative);
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, "").unwrap();
        binary
    }

    #[test]
    fn mise_is_taken_only_from_its_marker_naming_this_tool_in_its_own_folder() {
        let folder = tempfile::tempdir().unwrap();
        let tool = folder
            .path()
            .join("mise/installs/github-hebilicious-quadratique");
        let binary = binary_in(&tool, "1.0.0/bin/quadratique");

        assert!(
            matches!(Route::of(&binary), Route::Unclear(_)),
            "no marker under mise/"
        );
        std::fs::write(tool.join(".mise.backend.toml"), MARKER).unwrap();
        assert_eq!(
            Route::of(&binary).command(),
            "mise upgrade --bump github:Hebilicious/quadratique"
        );

        let other = folder.path().join("opt/installs/other-tool");
        let binary = binary_in(&other, "1.0.0/bin/quadratique");
        assert!(matches!(Route::of(&binary), Route::File));
        std::fs::write(other.join(".mise.backend.toml"), MARKER).unwrap();
        assert!(
            matches!(Route::of(&binary), Route::Unclear(why) if why.contains("not this repository's tool"))
        );
    }

    #[test]
    fn cargo_is_taken_from_its_record_and_its_command_names_the_root() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("my cargo");
        let binary = binary_in(&root, "bin/quadratique");
        assert!(matches!(Route::of(&binary), Route::File));

        std::fs::write(root.join(".crates2.json"), "{ not json").unwrap();
        assert!(matches!(Route::of(&binary), Route::Unclear(_)));

        std::fs::write(
            root.join(".crates2.json"),
            r#"{"installs":{"quadratique 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["quadratique"]}}}"#,
        )
        .unwrap();
        assert_eq!(
            Route::of(&binary).command(),
            format!("cargo install --root '{}' quadratique", root.display())
        );
    }
}
