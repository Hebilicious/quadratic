use std::path::Path;

use super::environment::{self, Configuration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    RemoteHead,
    MainWorktree,
}

impl Source {
    pub fn describe(self) -> &'static str {
        match self {
            Source::RemoteHead => "the remote's HEAD (refs/remotes/origin/HEAD)",
            Source::MainWorktree => "the branch checked out in the main worktree",
        }
    }
}

/// The default branch `repository` (its git directory, shared by every worktree) names: the
/// remote's HEAD, else the branch checked out in the main worktree. `None` when neither names
/// a branch (no remote HEAD and a detached main worktree).
pub fn named_by(repository: &Path) -> Option<(String, Source)> {
    symbolic(
        repository,
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/",
    )
    .map(|branch| (branch, Source::RemoteHead))
    .or_else(|| {
        symbolic(repository, "HEAD", "refs/heads/").map(|branch| (branch, Source::MainWorktree))
    })
}

fn symbolic(repository: &Path, reference: &str, prefix: &str) -> Option<String> {
    let mut command = std::process::Command::new("git");
    environment::prepare(&mut command, &Configuration::User);
    let output = command
        .arg("--git-dir")
        .arg(repository)
        .args(["symbolic-ref", "--quiet", reference])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .strip_prefix(prefix)
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{Source, named_by};
    use std::path::Path;
    use std::process::Command;

    fn git(directory: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(directory)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn a_reftable_repository_names_the_branch_its_remote_head_points_to() {
        let folder = tempfile::tempdir().expect("a temporary folder");
        let root = folder.path();
        git(
            root,
            &[
                "init",
                "--quiet",
                "--ref-format=reftable",
                "--initial-branch=work",
            ],
        );
        git(
            root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/stable",
            ],
        );

        assert_eq!(
            named_by(&root.join(".git")),
            Some(("stable".to_owned(), Source::RemoteHead))
        );
    }

    #[test]
    fn without_a_remote_head_the_main_worktrees_branch_is_named() {
        let folder = tempfile::tempdir().expect("a temporary folder");
        let root = folder.path();
        git(
            root,
            &[
                "init",
                "--quiet",
                "--ref-format=reftable",
                "--initial-branch=work",
            ],
        );

        assert_eq!(
            named_by(&root.join(".git")),
            Some(("work".to_owned(), Source::MainWorktree))
        );
    }
}
