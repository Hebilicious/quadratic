use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

use quadratique_server::git::environment;
use quadratique_server::server::{self, Paths};
use serde_json::json;
use tempfile::TempDir;

use crate::support::{Running, Who, within};

pub struct Repository {
    _directory: TempDir,
    pub root: PathBuf,
    parent: PathBuf,
}

impl Repository {
    pub fn new() -> Repository {
        environment::read_no_user_configuration();
        let directory = tempfile::Builder::new()
            .prefix("quadratique-repository-")
            .tempdir()
            .expect("a temporary directory");
        let parent = directory.path().canonicalize().expect("a real path");
        let root = parent.join("main");
        std::fs::create_dir(&root).expect("the main worktree");
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        let repository = Repository {
            _directory: directory,
            root,
            parent,
        };
        repository.commit(&repository.root, "README", "init");
        repository
    }

    pub fn common_dir(&self) -> String {
        self.root.join(".git").display().to_string()
    }

    pub fn worktree(&self, branch: &str) -> PathBuf {
        self.worktree_from(branch, "HEAD")
    }

    pub fn worktree_from(&self, branch: &str, start: &str) -> PathBuf {
        let path = self.parent.join(branch);
        git(
            &self.root,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                branch,
                &path.display().to_string(),
                start,
            ],
        );
        path
    }

    pub fn commit(&self, directory: &Path, file: &str, message: &str) -> String {
        std::fs::write(directory.join(file), message).expect("a file to commit");
        git(directory, &["add", file]);
        git(directory, &["commit", "--quiet", "--message", message]);
        self.tip(directory, "HEAD")
    }

    pub fn tip(&self, directory: &Path, revision: &str) -> String {
        git_output(directory, &["rev-parse", revision])
    }

    pub fn merge(&self, args: &[&str]) -> String {
        let mut command = vec!["merge", "--quiet"];
        command.extend_from_slice(args);
        git(&self.root, &command);
        self.tip(&self.root, "HEAD")
    }

    pub fn commit_raw_message_on_main(&self, message: &[u8]) -> String {
        let mut object = format!(
            "tree {}\nparent {}\nauthor Test <test@example.invalid> 1 +0000\ncommitter Test <test@example.invalid> 1 +0000\n\n",
            self.tip(&self.root, "main^{tree}"),
            self.tip(&self.root, "main"),
        )
        .into_bytes();
        object.extend_from_slice(message);
        let mut command = git_command(
            &self.root,
            &["hash-object", "-t", "commit", "-w", "--stdin"],
        );
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("git runs");
        child
            .stdin
            .take()
            .expect("a piped stdin")
            .write_all(&object)
            .expect("the commit object is written");
        let output = child.wait_with_output().expect("git ends");
        assert!(output.status.success(), "git hash-object fails");
        let commit = String::from_utf8(output.stdout)
            .expect("a hash")
            .trim()
            .to_owned();
        git(&self.root, &["reset", "--quiet", "--hard", &commit]);
        commit
    }

    pub fn commit_beside(&self, branch: &str, message: &str) -> String {
        let tree = self.tip(&self.root, &format!("{branch}^{{tree}}"));
        let parent = self.tip(&self.root, branch);
        git_output(
            &self.root,
            &["commit-tree", &tree, "-p", &parent, "-m", message],
        )
    }

    pub fn move_ref_by_hand(&self, branch: &str, commit: &str) {
        let file = self.root.join(".git/refs/heads").join(branch);
        std::fs::write(file, format!("{commit}\n")).expect("the ref file is written");
    }

    pub fn stall_ref_reads(&self) -> StalledRefs {
        let fifo = self.root.join(".git/packed-refs");
        make_fifo(&fifo);
        StalledRefs { fifo }
    }

    pub fn push_to_new_remote(&self) -> Remote {
        let directory = tempfile::Builder::new()
            .prefix("quadratique-remote-")
            .tempdir()
            .expect("a temporary directory");
        let parent = directory.path().canonicalize().expect("a real path");
        git(
            &parent,
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch=main",
                "origin.git",
            ],
        );
        let bare = parent.join("origin.git").display().to_string();
        git(&self.root, &["remote", "add", "origin", &bare]);
        git(
            &self.root,
            &["push", "--quiet", "--set-upstream", "origin", "main"],
        );
        git(&parent, &["clone", "--quiet", &bare, "other"]);
        Remote {
            _directory: directory,
            other: parent.join("other"),
        }
    }

    pub fn fetch(&self) {
        git(&self.root, &["fetch", "--quiet", "origin"]);
    }

    pub fn pull(&self) {
        git(&self.root, &["pull", "--quiet", "--ff-only"]);
    }

    pub fn human(&self) -> Who {
        Who {
            actor: Some(json!({ "kind": "human", "name": "sam" })),
            repository: Some(self.common_dir()),
            worktree: Some(self.root.display().to_string()),
            branch: Some("main".into()),
        }
    }

    pub fn agent(&self, worktree: &Path, branch: &str) -> Who {
        Who {
            actor: Some(json!({
                "kind": "agent",
                "name": format!("claude-code@{branch}"),
                "harness": "claude-code",
            })),
            repository: Some(self.common_dir()),
            worktree: Some(worktree.display().to_string()),
            branch: Some(branch.into()),
        }
    }
}

pub struct Remote {
    _directory: TempDir,
    other: PathBuf,
}

impl Remote {
    pub fn commit_and_push_to_main(&self, file: &str, message: &str) -> String {
        git(&self.other, &["pull", "--quiet", "--ff-only"]);
        std::fs::write(self.other.join(file), message).expect("a file to commit");
        git(&self.other, &["add", file]);
        git(&self.other, &["commit", "--quiet", "--message", message]);
        git(&self.other, &["push", "--quiet", "origin", "HEAD:main"]);
        git_output(&self.other, &["rev-parse", "HEAD"])
    }

    pub fn merge(&self, branch: &str) -> String {
        git(&self.other, &["pull", "--quiet", "--ff-only"]);
        git(&self.other, &["fetch", "--quiet", "origin"]);
        let theirs = format!("origin/{branch}");
        git(
            &self.other,
            &["merge", "--quiet", "--no-ff", "--message", "Merge", &theirs],
        );
        git(&self.other, &["push", "--quiet", "origin", "HEAD:main"]);
        git_output(&self.other, &["rev-parse", "HEAD"])
    }

    pub fn force_main_to(&self, commit: &str) {
        git(&self.other, &["reset", "--quiet", "--hard", commit]);
        git(
            &self.other,
            &["push", "--quiet", "--force", "origin", "HEAD:main"],
        );
    }
}

pub struct StalledRefs {
    fifo: PathBuf,
}

impl StalledRefs {
    fn writer(&self) -> Option<OwnedFd> {
        match rustix::fs::open(
            &self.fifo,
            OFlags::WRONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(writer) => Some(writer),
            Err(Errno::NXIO | Errno::NOENT) => None,
            Err(error) => panic!("the stalled refs do not open for writing: {error}"),
        }
    }

    pub async fn await_git(&self) -> OwnedFd {
        within("git opening the stalled refs", async {
            loop {
                if let Some(writer) = self.writer() {
                    return writer;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
    }

    pub fn release(&self, writer: OwnedFd) {
        let _ = std::fs::remove_file(&self.fifo);
        drop(writer);
    }
}

impl Drop for StalledRefs {
    fn drop(&mut self) {
        let writer = self.writer();
        let _ = std::fs::remove_file(&self.fifo);
        drop(writer);
    }
}

/// git by its full path. When a command names its program without a path and its environment
/// changes PATH, as `env_clear` does, std forks this whole test process instead of spawning the
/// program. Each fork copies the page tables of every test that runs at that moment and stalls
/// their threads, so the tests that allocate most slowed down twenty times or more.
pub fn git_program() -> &'static Path {
    static GIT: OnceLock<PathBuf> = OnceLock::new();
    GIT.get_or_init(|| {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let here = std::env::current_dir().expect("the test's directory");
        program_on(&path, &here, "git").expect("git is on the PATH")
    })
}

/// The first executable `name` on `path`, as an absolute path: the commands that run it start in
/// other directories. A relative entry is taken from `here`, and an empty entry means `here`,
/// as a shell takes them.
fn program_on(path: &std::ffi::OsStr, here: &Path, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|directory| here.join(directory).join(name))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|file| file.is_file() && file.permissions().mode() & 0o111 != 0)
        })
}

#[test]
fn a_relative_path_entry_gives_git_by_its_full_path() {
    let here = tempfile::tempdir().expect("a temporary directory");
    let tools = here.path().join("tools");
    std::fs::create_dir(&tools).expect("the tools folder");
    let program = tools.join("git");
    std::fs::write(&program, "#!/bin/sh\n").expect("the program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("the program is executable");
    std::fs::write(here.path().join("git"), "not executable").expect("a plain file");

    let path = std::env::join_paths(["nowhere", "tools", "/usr/bin"]).expect("a PATH");
    assert_eq!(program_on(&path, here.path(), "git"), Some(program.clone()));
    let empty_first = std::ffi::OsString::from(":tools");
    assert_eq!(program_on(&empty_first, here.path(), "git"), Some(program));
}

fn git_command(directory: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(git_program());
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::null());
    command
}

pub fn has_object(git_dir: &Path, hash: &str) -> bool {
    git_command(git_dir, &["cat-file", "-e", hash])
        .env("GIT_NO_LAZY_FETCH", "1")
        .output()
        .expect("git runs")
        .status
        .success()
}

pub fn git(directory: &Path, args: &[&str]) {
    let output = git_command(directory, args).output().expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} fails in {}: {}",
        directory.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(directory: &Path, args: &[&str]) -> String {
    let output = git_command(directory, args).output().expect("git runs");
    assert!(output.status.success(), "git {args:?} fails");
    String::from_utf8(output.stdout)
        .expect("git prints UTF-8")
        .trim()
        .to_owned()
}

impl Running {
    pub async fn look(&self) {
        within(
            "a look at the repositories",
            self.server.look_at_repositories(),
        )
        .await;
    }

    pub async fn stop(self) -> (TempDir, PathBuf) {
        let (server, code, home) = self.into_parts();
        within("the server's shutdown", server.shutdown()).await;
        (home, code)
    }

    pub async fn resume(home: TempDir, code: PathBuf) -> Running {
        let server = within(
            "the server's start",
            server::start(
                &Paths::in_home(home.path()).expect("an absolute home"),
                server::loopback(),
            ),
        )
        .await
        .expect("the server starts again");
        Running::of(server, code, home)
    }

    pub async fn restart(self) -> Running {
        let (home, code) = self.stop().await;
        Running::resume(home, code).await
    }
}

#[cfg(target_os = "linux")]
fn make_fifo(path: &Path) {
    rustix::fs::mkfifoat(rustix::fs::CWD, path, Mode::RUSR | Mode::WUSR)
        .expect("the stalled refs are made");
}

/// rustix has no `mkfifoat` on macOS.
#[cfg(target_os = "macos")]
fn make_fifo(path: &Path) {
    let made = std::process::Command::new("mkfifo")
        .args(["-m", "600"])
        .arg(path)
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "the stalled refs are made");
}
