//! How the running binary was installed, told from where its file is, never from the
//! environment:
//!
//! - mise: the file is inside `<data>/installs/<tool>/`, where mise keeps `.mise.backend.toml`
//!   naming the tool (`short = "github:Hebilicious/quadratique"`);
//! - cargo: the file is `<cargo home>/bin/quadratique`, and `<cargo home>/.crates2.json`, which
//!   `cargo install` and `cargo binstall` keep, lists a `quadratique` package with that binary;
//! - else a file that `quadratique upgrade` may replace.

use std::path::{Path, PathBuf};

use serde_json::Value;

pub enum Route {
    File,
    Mise { tool: String },
    Cargo,
}

impl Route {
    pub fn of(binary: &Path) -> Route {
        if let Some(tool) = mise_tool(binary) {
            return Route::Mise { tool };
        }
        if installed_by_cargo(binary) {
            return Route::Cargo;
        }
        Route::File
    }

    pub fn manager(&self) -> Option<&'static str> {
        match self {
            Route::File => None,
            Route::Mise { .. } => Some("mise"),
            Route::Cargo => Some("cargo"),
        }
    }

    /// The command that upgrades the binary of this route.
    pub fn command(&self) -> String {
        match self {
            Route::File => "quadratique upgrade".to_owned(),
            Route::Mise { tool } => format!("mise upgrade --bump {tool}"),
            Route::Cargo => "cargo install quadratique".to_owned(),
        }
    }
}

fn mise_tool(binary: &Path) -> Option<String> {
    binary.ancestors().skip(1).find_map(|folder| {
        let installs = folder.parent()?;
        if installs.file_name()? != "installs" {
            return None;
        }
        let backend = std::fs::read_to_string(folder.join(".mise.backend.toml")).ok()?;
        backend.lines().find_map(|line| {
            let value = line.strip_prefix("short")?.trim_start().strip_prefix('=')?;
            let tool = value.trim().strip_prefix('"')?.strip_suffix('"')?;
            tool.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":/@._-".contains(&byte))
                .then(|| tool.to_owned())
        })
    })
}

fn installed_by_cargo(binary: &Path) -> bool {
    let Some(bin) = binary
        .parent()
        .filter(|bin| bin.file_name() == Some("bin".as_ref()))
    else {
        return false;
    };
    let Some(cargo_home) = bin.parent() else {
        return false;
    };
    let Some(name) = binary.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let record: PathBuf = cargo_home.join(".crates2.json");
    let Ok(text) = std::fs::read(record) else {
        return false;
    };
    let Ok(record) = serde_json::from_slice::<Value>(&text) else {
        return false;
    };
    record["installs"]
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
        })
}

#[cfg(test)]
mod tests {
    use super::Route;

    #[test]
    fn a_binary_under_a_mise_install_folder_is_managed_by_mise_and_one_in_a_cargo_home_by_cargo() {
        let folder = tempfile::tempdir().unwrap();
        let tool = folder
            .path()
            .join("mise/installs/github-hebilicious-quadratique");
        let binary = tool.join("1.0.0/quadratique-1.0.0-t/bin/quadratique");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, "").unwrap();
        assert!(matches!(Route::of(&binary), Route::File));
        std::fs::write(
            tool.join(".mise.backend.toml"),
            "short = \"github:Hebilicious/quadratique\"\nfull = \"github:Hebilicious/quadratique\"\n",
        )
        .unwrap();
        assert_eq!(
            Route::of(&binary).command(),
            "mise upgrade --bump github:Hebilicious/quadratique"
        );

        let cargo = folder.path().join("cargo");
        let binary = cargo.join("bin/quadratique");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, "").unwrap();
        assert!(matches!(Route::of(&binary), Route::File));
        std::fs::write(
            cargo.join(".crates2.json"),
            r#"{"installs":{"quadratique 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["quadratique"]}}}"#,
        )
        .unwrap();
        assert!(matches!(Route::of(&binary), Route::Cargo));
    }
}
