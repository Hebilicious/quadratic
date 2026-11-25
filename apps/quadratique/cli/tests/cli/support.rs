use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;
use rustix::process::{Pid, Signal, kill_process};
#[cfg(target_os = "linux")]
use rustix::process::{PidfdFlags, pidfd_open};
use serde_json::Value;
use tempfile::TempDir;

pub const BINARY: &str = env!("CARGO_BIN_EXE_quadratique");
pub const HANG_GUARD: Duration = Duration::from_secs(120);

fn search_path() -> std::ffi::OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

pub fn kill(pid: u32) {
    let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
        return;
    };
    match kill_process(pid, Signal::KILL) {
        Ok(()) | Err(Errno::SRCH) => {}
        Err(error) => panic!("kill {pid:?} fails: {error}"),
    }
}

#[cfg(target_os = "linux")]
pub fn await_exit(pid: u32) {
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .expect("a process id");
    let descriptor = match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(descriptor) => descriptor,
        Err(Errno::SRCH) => return,
        Err(error) => panic!("pidfd_open {pid:?} fails: {error}"),
    };
    let guard = Timespec {
        tv_sec: HANG_GUARD.as_secs().try_into().expect("seconds"),
        tv_nsec: 0,
    };
    let mut watched = [PollFd::new(&descriptor, PollFlags::IN)];
    let ready = poll(&mut watched, Some(&guard)).expect("poll on the process");
    assert_eq!(
        ready, 1,
        "process {pid:?} did not exit within the hang guard"
    );
}

/// macOS has no pidfd: the process has ended once the kernel no longer knows it, or knows it as a zombie.
#[cfg(target_os = "macos")]
pub fn await_exit(pid: u32) {
    await_until(&format!("the exit of process {pid}"), || {
        macos::has_ended(pid)
    });
}

/// The processes of this account on macOS, with their arguments and environment as the kernel keeps them
/// (`KERN_PROCARGS2`), one string each, so a space in a path changes nothing.
#[cfg(target_os = "macos")]
pub mod macos {
    use std::process::Command;

    use rustix::io::Errno;
    use rustix::process::{Pid, test_kill_process};
    use sysctl::{Ctl, CtlValue, Sysctl};

    const CTL_KERN: i32 = 1;
    const KERN_PROCARGS2: i32 = 49;

    pub struct Arguments {
        pub argv: Vec<Vec<u8>>,
        pub environment: Vec<Vec<u8>>,
    }

    /// The process ids of this account whose command line and environment, as `ps -E` flattens them into one
    /// line, contain `text`. A process that holds `text` in one of its strings holds it in that line too, so this
    /// finds every such process, and some more, which `arguments` then tells apart; reading the arguments of every
    /// process instead would cost a megabyte each (`ARG_MAX`). A `ps` that fails fails the test.
    pub fn pids_mentioning(text: &str) -> Vec<u32> {
        let listed = Command::new("ps")
            .args(["-E", "-ww", "-x", "-o", "pid=", "-o", "command="])
            .output()
            .expect("ps runs");
        assert!(listed.status.success(), "ps fails: {listed:?}");
        String::from_utf8_lossy(&listed.stdout)
            .lines()
            .filter(|line| line.contains(text))
            .map(|line| {
                let pid = line.split_whitespace().next().unwrap_or_default();
                pid.parse().expect("a process id")
            })
            .collect()
    }

    /// `None` when the process has ended, or the kernel does not show its arguments.
    pub fn arguments(pid: u32) -> Option<Arguments> {
        let control = Ctl::Oid(vec![CTL_KERN, KERN_PROCARGS2, i32::try_from(pid).ok()?]);
        let (CtlValue::Node(bytes) | CtlValue::Struct(bytes)) = control.value().ok()? else {
            return None;
        };
        parse(&bytes)
    }

    /// `argc`, the path of the executable, NUL padding, then `argc` arguments and the environment, each ending with
    /// a NUL.
    fn parse(bytes: &[u8]) -> Option<Arguments> {
        let count = usize::try_from(i32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?)).ok()?;
        let mut strings = bytes[4..].split(|byte| *byte == 0);
        strings.next()?;
        let mut rest = strings.skip_while(|string| string.is_empty());
        let argv: Vec<Vec<u8>> = rest.by_ref().take(count).map(<[u8]>::to_vec).collect();
        if argv.len() != count {
            return None;
        }
        let environment = rest
            .take_while(|string| !string.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        Some(Arguments { argv, environment })
    }

    pub fn has_ended(pid: u32) -> bool {
        let Some(raw) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
            return true;
        };
        match test_kill_process(raw) {
            Err(Errno::SRCH) => return true,
            Ok(()) | Err(Errno::PERM) => {}
            Err(error) => panic!("kill -0 {pid} fails: {error}"),
        }
        let state = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps runs");
        let stat = String::from_utf8_lossy(&state.stdout);
        match state.status.code() {
            Some(0) => stat.trim_start().starts_with('Z'),
            Some(1) if stat.trim().is_empty() => true,
            _ => panic!("ps -p {pid} fails: {state:?}"),
        }
    }
}

#[cfg(target_os = "linux")]
fn proc_entries() -> Vec<u32> {
    std::fs::read_dir("/proc")
        .expect("/proc lists the processes")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .collect()
}

#[cfg(target_os = "linux")]
fn words(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split(|byte| *byte == 0).collect()
}

pub struct Home {
    directory: TempDir,
}

impl Home {
    pub fn new() -> Home {
        Home {
            directory: tempfile::Builder::new()
                .prefix("quadratique-home-")
                .tempdir()
                .expect("a temporary Quadratique home"),
        }
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    pub fn runtime_path(&self) -> PathBuf {
        self.path().join("server.json")
    }

    pub fn runtime(&self) -> Option<Value> {
        let text = std::fs::read(self.runtime_path()).ok()?;
        serde_json::from_slice(&text).ok()
    }

    pub fn quadratique(&self, directory: &Path) -> Quadratique {
        let mut command = Command::new(BINARY);
        command
            .current_dir(directory)
            .env_clear()
            .env("PATH", search_path())
            .env("HOME", self.path())
            .env("QUADRATIQUE_HOME", self.path())
            .env("USER", "tester")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null());
        Quadratique { command }
    }

    fn marker(&self) -> String {
        format!("HOME={}", self.path().display())
    }

    #[cfg(target_os = "macos")]
    fn is_ours(&self, pid: u32) -> bool {
        let marker = self.marker();
        macos::arguments(pid).is_some_and(|arguments| {
            arguments
                .environment
                .iter()
                .any(|variable| variable == marker.as_bytes())
        })
    }

    #[cfg(target_os = "macos")]
    pub fn processes(&self) -> Vec<u32> {
        macos::pids_mentioning(&self.marker())
            .into_iter()
            .filter(|pid| self.is_ours(*pid))
            .collect()
    }

    #[cfg(target_os = "macos")]
    pub fn servers(&self) -> Vec<u32> {
        self.processes()
            .into_iter()
            .filter(|pid| {
                macos::arguments(*pid).is_some_and(|arguments| {
                    arguments.argv.len() >= 2
                        && arguments.argv[0].ends_with(b"quadratique")
                        && arguments.argv[1] == b"serve"
                })
            })
            .collect()
    }

    #[cfg(target_os = "linux")]
    fn is_ours(&self, pid: u32) -> bool {
        let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
        words(&environ).contains(&self.marker().as_bytes())
    }

    #[cfg(target_os = "linux")]
    pub fn processes(&self) -> Vec<u32> {
        proc_entries()
            .into_iter()
            .filter(|pid| self.is_ours(*pid))
            .collect()
    }

    #[cfg(target_os = "linux")]
    pub fn servers(&self) -> Vec<u32> {
        self.processes()
            .into_iter()
            .filter(|pid| {
                let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
                let words = words(&cmdline);
                words.len() >= 2 && words[0].ends_with(b"quadratique") && words[1] == b"serve"
            })
            .collect()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let left = self.processes();
        for pid in &left {
            // The list is a snapshot: a process id that ended since may name another process now.
            if self.is_ours(*pid) {
                kill(*pid);
            }
        }
        for pid in left {
            await_exit(pid);
        }
    }
}

pub struct Quadratique {
    command: Command,
}

impl Quadratique {
    pub fn args<I, S>(mut self, args: I) -> Quadratique
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command.args(args);
        self
    }

    pub fn env(mut self, name: &str, value: &str) -> Quadratique {
        self.command.env(name, value);
        self
    }

    pub fn without_env(mut self, name: &str) -> Quadratique {
        self.command.env_remove(name);
        self
    }

    pub fn run(self) -> Run {
        self.spawn().finish()
    }

    pub fn spawn(mut self) -> Process {
        Process::of(
            self.command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("the quadratique binary starts"),
        )
    }

    pub fn spawn_interactive(mut self) -> Process {
        self.command.stdin(Stdio::piped());
        self.spawn()
    }
}

pub struct Process {
    child: Option<Child>,
}

impl Process {
    fn of(child: Child) -> Process {
        Process { child: Some(child) }
    }

    fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("the process is still owned")
    }

    pub fn stdout(&mut self) -> ChildStdout {
        self.child().stdout.take().expect("a piped standard output")
    }

    pub fn stdin(&mut self) -> &mut ChildStdin {
        self.child().stdin.as_mut().expect("a piped standard input")
    }

    pub fn id(&mut self) -> u32 {
        self.child().id()
    }

    pub fn close_stdin(&mut self) {
        drop(self.child().stdin.take());
    }

    pub fn finish(mut self) -> Run {
        let child = self.child.take().expect("the process is still owned");
        let pid = child.id();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(child.wait_with_output());
        });
        let output: Output = receiver
            .recv_timeout(HANG_GUARD)
            .unwrap_or_else(|_| {
                kill(pid);
                panic!("quadratique (pid {pid}) did not finish within the hang guard");
            })
            .expect("the output of quadratique");
        Run {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Debug)]
pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|error| {
            panic!("standard output is not JSON ({error}): {self:?}");
        })
    }

    pub fn ok(self) -> Value {
        assert_eq!(self.code, 0, "the command fails: {self:?}");
        self.json()
    }
}

pub struct Lines {
    receiver: mpsc::Receiver<String>,
    deadline: Instant,
}

impl Lines {
    pub fn of(stdout: ChildStdout) -> Lines {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Lines {
            receiver,
            deadline: Instant::now() + HANG_GUARD,
        }
    }

    pub fn next_json(&self) -> Value {
        let left = self.deadline.saturating_duration_since(Instant::now());
        let line = self
            .receiver
            .recv_timeout(left)
            .expect("a line on standard output within the hang guard");
        serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("the line is not JSON ({error}): {line}"))
    }
}

pub fn send_line(process: &mut Process, message: &Value) {
    let stdin = process.stdin();
    writeln!(stdin, "{message}").expect("the message is written");
    stdin.flush().expect("the message is sent");
}

pub struct Repository {
    _directory: TempDir,
    pub root: PathBuf,
    parent: PathBuf,
}

impl Repository {
    pub fn new() -> Repository {
        let directory = tempfile::Builder::new()
            .prefix("quadratique-repository-")
            .tempdir()
            .expect("a temporary directory");
        let parent = directory.path().canonicalize().expect("a real path");
        let root = parent.join("main");
        std::fs::create_dir(&root).expect("the main worktree");
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        git(
            &root,
            &["commit", "--quiet", "--allow-empty", "--message", "init"],
        );
        Repository {
            _directory: directory,
            root,
            parent,
        }
    }

    pub fn worktree(&self, name: &str) -> PathBuf {
        let path = self.parent.join(name);
        self.add_worktree(name, &path);
        path
    }

    pub fn add_worktree(&self, branch: &str, path: &Path) {
        git(
            &self.root,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                branch,
                &path.display().to_string(),
            ],
        );
    }
}

pub fn git_stdout(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .env_clear()
        .env("PATH", search_path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::null())
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} fails in {}: {}",
        directory.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git prints UTF-8")
        .trim()
        .to_owned()
}

pub fn git(directory: &Path, args: &[&str]) {
    let status = Command::new("git")
        .env_clear()
        .env("PATH", search_path())
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
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .expect("git runs");
    assert!(
        status.success(),
        "git {args:?} fails in {}",
        directory.display()
    );
}

pub fn outside() -> TempDir {
    tempfile::Builder::new()
        .prefix("quadratique-outside-")
        .tempdir()
        .expect("a directory outside any repository")
}

pub fn await_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + HANG_GUARD;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "{what} did not happen within the hang guard"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
