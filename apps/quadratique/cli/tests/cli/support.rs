use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;
use rustix::process::{Pid, PidfdFlags, Signal, kill_process, pidfd_open};
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;

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

fn proc_entries() -> Vec<u32> {
    std::fs::read_dir("/proc")
        .expect("/proc lists the processes")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .collect()
}

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
                .permissions(std::fs::Permissions::from_mode(0o700))
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
        self.quadratique_at(Path::new(BINARY), directory)
    }

    /// The binary at `program` instead of the one under test, as installed elsewhere.
    pub fn quadratique_at(&self, program: &Path, directory: &Path) -> Quadratique {
        let mut command = Command::new(program);
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

    pub fn processes(&self) -> Vec<u32> {
        let marker = format!("HOME={}", self.path().display());
        proc_entries()
            .into_iter()
            .filter(|pid| {
                let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
                words(&environ).contains(&marker.as_bytes())
            })
            .collect()
    }

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
            kill(*pid);
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

/// Takes `listener` out of the listening state for every descriptor of it, then closes it: later connections are
/// refused. A plain drop closes one descriptor, while a child that another test spawns holds a copy of each descriptor
/// of this process between its `fork` and its `exec`, and so keeps the socket listening for that moment.
pub fn stop_listening(listener: std::os::unix::net::UnixListener) {
    rustix::net::shutdown(&listener, rustix::net::Shutdown::Both)
        .expect("the socket stops listening");
}

/// Lets go of the `flock` on `lock` for every descriptor of it, then closes it. A plain drop closes one descriptor,
/// while a child that another test spawns holds a copy of each descriptor of this process between its `fork` and its
/// `exec`, and so keeps the lock taken for that moment.
pub fn free(lock: std::fs::File) {
    lock.unlock().expect("the lock is freed");
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

/// What processes do in a home, as inotify reports it: a file opened, or renamed into place. A test waits on it for
/// the step it needs, such as a command that has looked at the home's lock, or a server that has published its
/// runtime file, instead of sleeping.
pub struct Watch {
    inotify: rustix::fd::OwnedFd,
    buffer: Vec<std::mem::MaybeUninit<u8>>,
    seen: std::collections::VecDeque<(Step<'static>, Vec<u8>)>,
}

/// A step that a [`Watch`] waits for. The name in `seen` is kept apart; here it is only the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step<'a> {
    /// A process opened the file.
    Opened(&'a str),
    /// A process renamed a file into place under this name, as a server publishes its runtime file.
    Published(&'a str),
}

impl<'a> Step<'a> {
    fn parts(self) -> (Step<'static>, &'a str) {
        match self {
            Step::Opened(name) => (Step::Opened(""), name),
            Step::Published(name) => (Step::Published(""), name),
        }
    }
}

impl Watch {
    pub fn of(home: &Home) -> Watch {
        use rustix::fs::inotify;

        let inotify = inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)
            .expect("an inotify instance");
        inotify::add_watch(
            &inotify,
            home.path(),
            inotify::WatchFlags::OPEN | inotify::WatchFlags::MOVED_TO,
        )
        .expect("a watch on the home");
        Watch {
            inotify,
            buffer: vec![std::mem::MaybeUninit::uninit(); 64 * 1024],
            seen: std::collections::VecDeque::new(),
        }
    }

    /// Forgets every step seen so far, so that the next wait counts only what happens from now on.
    pub fn clear(&mut self) {
        use rustix::fs::inotify::Reader;

        let mut reader = Reader::new(&self.inotify, &mut self.buffer);
        loop {
            match reader.next() {
                Ok(_) => {}
                Err(Errno::AGAIN) => break,
                Err(error) => panic!("reading the watch fails: {error}"),
            }
        }
        self.seen.clear();
    }

    /// Returns once each of `steps` has happened, in that order, after this watch was made or after the last step an
    /// earlier call waited for.
    pub fn await_steps(&mut self, steps: &[Step]) {
        let deadline = Instant::now() + HANG_GUARD;
        for step in steps {
            let (kind, name) = step.parts();
            loop {
                match self.seen.pop_front() {
                    Some((seen, file)) if seen == kind && file == name.as_bytes() => break,
                    Some(_) => {}
                    None => self.read_within(deadline, name),
                }
            }
        }
    }

    fn read_within(&mut self, deadline: Instant, name: &str) {
        use rustix::fs::inotify::ReadFlags;
        use rustix::fs::inotify::Reader;

        let mut reader = Reader::new(&self.inotify, &mut self.buffer);
        loop {
            match reader.next() {
                Ok(event) => {
                    let kind = if event.events().contains(ReadFlags::MOVED_TO) {
                        Step::Published("")
                    } else {
                        Step::Opened("")
                    };
                    if let Some(file) = event.file_name() {
                        self.seen.push_back((kind, file.to_bytes().to_vec()));
                    }
                }
                Err(Errno::AGAIN) if !self.seen.is_empty() => return,
                Err(Errno::AGAIN) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    let guard = Timespec {
                        tv_sec: left.as_secs().try_into().expect("seconds"),
                        tv_nsec: left.subsec_nanos().into(),
                    };
                    let mut watched = [PollFd::new(&self.inotify, PollFlags::IN)];
                    let ready = poll(&mut watched, Some(&guard)).expect("poll on the watch");
                    assert_eq!(ready, 1, "{name} was not opened within the hang guard");
                }
                Err(error) => panic!("reading the watch fails: {error}"),
            }
        }
    }
}
