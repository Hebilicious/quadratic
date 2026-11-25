use std::io::Write;
use std::path::{Path, PathBuf};

use acid_cat_core::cli::{CliIo, exit};
use acid_cat_core::http::HttpClient;
use acid_cat_core::{Kind, Outcome};
use quadratique_server::app::input_refusal;
use quadratique_server::domain::Binding;
use quadratique_server::git::{default_branch, environment};
use serde_json::{Map, Value, json};

use crate::agents_line;
use crate::background::{self, Found};
use crate::caller;
use crate::commands::{self, print_json, report};
use crate::shipped;

pub const USAGE: &str = "init [--key <KEY>] [--name <name>] [--default-branch <branch>] [--dry-run] | init --print <SKILL.md|planning.md|AGENTS.md>: set up Quadratique in this git repository, or bring its setup to this build's text after an upgrade. Writes the skill to .agents/skills/quadratique/ (SKILL.md and planning.md), adds the Quadratique line to AGENTS.md, and creates the project bound to the repository (key from --key, else from the folder's name; default branch from --default-branch, else the remote's HEAD, else the branch checked out in the main worktree, else main; a project that has the repository already is left alone). A skill file that holds the text of an earlier build is replaced with this build's; a skill file that holds any other text, which a person wrote, is left as it is. The skill files and the AGENTS.md line of Quadratic, the earlier name, move to Quadratique's. The answer says per file what was done, and its warnings name each kept file with the command that prints this build's text of it. --dry-run writes nothing, starts no server, and answers what init would do. --print writes this build's text of one file (AGENTS.md: its line) to standard output as it is, to merge into a kept file; it needs no repository and no server. Its files stay inside the repository; the project is in the server's store. Writes no agent configuration and makes no commit: when it wrote a setup file, or one of its own is not committed yet, it prints the commit to run.";

struct SkillFile {
    path: &'static str,
    text: &'static str,
    shipped: &'static [&'static str],
}

const SKILL_FILES: [SkillFile; 2] = [
    SkillFile {
        path: ".agents/skills/quadratique/SKILL.md",
        text: include_str!("../../skill/quadratique/SKILL.md"),
        shipped: shipped::SKILL_MD,
    },
    SkillFile {
        path: ".agents/skills/quadratique/planning.md",
        text: include_str!("../../skill/quadratique/planning.md"),
        shipped: shipped::PLANNING_MD,
    },
];
const SKILL_FOLDER: &str = ".agents/skills/quadratique";
const AGENTS_PATH: &str = "AGENTS.md";
pub const AGENTS_LINE: &str = "We use Quadratique for issues: every task is an issue there. Use the `quadratique` skill (`.agents/skills/quadratique/SKILL.md`) for anything about tasks.";
const COMMIT_MESSAGE: &str = "Use Quadratique for issues";
const LINK_DEPTH: usize = 40;
const KEPT: &str = "different: yours is kept";

struct Request {
    key: Option<String>,
    name: Option<String>,
    default_branch: Option<String>,
    dry_run: bool,
}

/// The texts `init --print` gives, by the name an agent passes.
fn printable() -> [(&'static str, String); 3] {
    [
        ("SKILL.md", SKILL_FILES[0].text.to_owned()),
        ("planning.md", SKILL_FILES[1].text.to_owned()),
        (AGENTS_PATH, format!("{AGENTS_LINE}\n")),
    ]
}

/// Writes the build's text raw, so that an agent can merge it with a file a person changed. It
/// needs no repository and no server, and writes nothing.
fn print_text(name: &str, io: &mut CliIo) -> i32 {
    let texts = printable();
    let Some((_, text)) = texts.iter().find(|(known, _)| *known == name) else {
        let known: Vec<&str> = texts.iter().map(|(known, _)| *known).collect();
        return report(
            io,
            Kind::InvalidInput,
            format!("init --print knows {}, not {name:?}.", known.join(", ")),
        );
    };
    let mut stdout = std::io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => exit::VALUE,
        Err(error) => report(
            io,
            Kind::Internal,
            format!("Writing the text failed: {error}"),
        ),
    }
}

fn file_name(shown: &str) -> &str {
    Path::new(shown)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(shown)
}

/// The warning for a skill file kept because a person changed it: it names the file and the
/// command that prints the build's text to merge into it.
fn kept_warning(shown: &str) -> String {
    format!(
        "{shown} was kept: a person changed it, so it lacks this build's text. Merge that text into it: quadratique init --print {}",
        file_name(shown)
    )
}

fn changed_meanwhile(shown: &str) -> String {
    format!("{shown} was kept: it changed while init ran. Run quadratique init again.")
}

fn blocked_warning(shown: &str, reason: &str) -> String {
    format!("{shown} cannot be written: {reason}. init stops there until that is fixed.")
}

fn request(words: &[&str]) -> Option<Request> {
    let mut request = Request {
        key: None,
        name: None,
        default_branch: None,
        dry_run: false,
    };
    let mut rest = words.iter();
    while let Some(word) = rest.next() {
        let slot = match *word {
            "--dry-run" if !request.dry_run => {
                request.dry_run = true;
                continue;
            }
            "--key" => &mut request.key,
            "--name" => &mut request.name,
            "--default-branch" => &mut request.default_branch,
            _ => return None,
        };
        if slot.is_some() {
            return None;
        }
        *slot = Some((*rest.next()?).to_owned());
    }
    Some(request)
}

fn key_of(folder: &str) -> Option<String> {
    let key: String = folder
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .skip_while(|character| !character.is_ascii_alphabetic())
        .take(10)
        .collect::<String>()
        .to_ascii_uppercase();
    (key.len() >= 2).then_some(key)
}

/// What a setup file holds, in bytes as read.
enum Content {
    Absent,
    Same(Vec<u8>),
    Earlier(Vec<u8>),
    Different(Vec<u8>),
}

impl Content {
    fn bytes(&self) -> Option<&[u8]> {
        match self {
            Content::Absent => None,
            Content::Same(bytes) | Content::Earlier(bytes) | Content::Different(bytes) => {
                Some(bytes)
            }
        }
    }
}

enum Destination {
    Here(Content),
    LinkInside { target: PathBuf, content: Content },
    LinkOutside(PathBuf),
}

fn landing(path: &Path, depth: usize) -> std::io::Result<PathBuf> {
    if depth > LINK_DEPTH {
        return Err(std::io::Error::other(format!(
            "{} goes through more than {LINK_DEPTH} links",
            path.display()
        )));
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(path)?;
            let parent = path.parent().unwrap_or(Path::new("/"));
            landing(&parent.join(target), depth + 1)
        }
        Ok(_) => path.canonicalize(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match (path.parent(), path.file_name()) {
                (Some(parent), Some(name)) => Ok(landing(parent, depth + 1)?.join(name)),
                _ => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

fn read(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

type Classify<'a> = &'a dyn Fn(Vec<u8>) -> Content;

fn inspect(root: &Path, relative: &str, classify: Classify) -> std::io::Result<Destination> {
    let lexical = root.join(relative);
    let target = landing(&lexical, 0)?;
    if !target.starts_with(root) {
        return Ok(Destination::LinkOutside(target));
    }
    let content = read(&target)?.map_or(Content::Absent, classify);
    Ok(if target == lexical {
        Destination::Here(content)
    } else {
        Destination::LinkInside { target, content }
    })
}

fn writable(path: &Path) -> bool {
    rustix::fs::access(path, rustix::fs::Access::WRITE_OK).is_ok()
}

/// Why a write of `target` would fail now, when it can be told before trying: the file or the
/// folder that would receive it is not writable.
fn write_blocker(target: &Path) -> Option<String> {
    if std::fs::symlink_metadata(target).is_ok() && !writable(target) {
        return Some(format!("{} is not writable", target.display()));
    }
    let folder = target.ancestors().skip(1).find(|folder| folder.exists())?;
    (!writable(folder)).then(|| format!("{} is not writable", folder.display()))
}

/// What became of a write or a removal that the file's content allowed when init looked.
enum Done {
    Done,
    /// The file no longer held what init read, so it was left as it is now.
    ChangedMeanwhile,
}

struct File {
    shown: &'static str,
    target: PathBuf,
    linked: bool,
    content: Content,
}

impl File {
    fn plan(root: &Path, shown: &'static str, classify: Classify) -> Result<File, String> {
        let destination =
            inspect(root, shown, classify).map_err(|error| format!("{shown}: {error}"))?;
        let reached = match &destination {
            Destination::Here(_) => root.join(shown),
            Destination::LinkInside { target, .. } | Destination::LinkOutside(target) => {
                target.clone()
            }
        };
        if reached.to_str().is_none() {
            return Err(format!(
                "{shown} leads to {}, a path that is not UTF-8 text: quadratique init could not print the commit for it.",
                reached.display()
            ));
        }
        Ok(match destination {
            Destination::Here(content) => File {
                shown,
                target: root.join(shown),
                linked: false,
                content,
            },
            Destination::LinkInside { target, content } => File {
                shown,
                target,
                linked: true,
                content,
            },
            Destination::LinkOutside(target) => {
                return Err(format!(
                    "{shown} is a link to {}, outside this repository: quadratique init writes nothing outside the repository and nothing into an agent's configuration.",
                    target.display()
                ));
            }
        })
    }

    fn committed(&self, root: &Path) -> String {
        relative(&self.target, root)
    }

    fn uncommitted(&self, root: &Path) -> bool {
        uncommitted(root, &self.committed(root))
    }

    fn still_as_read(&self) -> std::io::Result<bool> {
        Ok(read(&self.target)?.as_deref() == self.content.bytes())
    }

    /// Writes `bytes` in the file's place. A new file is created only where none is; a file is
    /// replaced by a new one renamed over it, so another hard link to the old file keeps its
    /// bytes. Either is skipped when the file no longer holds what init read.
    fn write(&self, bytes: &[u8]) -> std::io::Result<Done> {
        let folder = self.target.parent().unwrap_or(Path::new("/"));
        std::fs::create_dir_all(folder)?;
        if matches!(self.content, Content::Absent) {
            let mut file = match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.target)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Ok(Done::ChangedMeanwhile);
                }
                Err(error) => return Err(error),
            };
            file.write_all(bytes)?;
            return Ok(Done::Done);
        }
        if !self.still_as_read()? {
            return Ok(Done::ChangedMeanwhile);
        }
        if !writable(&self.target) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("{} is not writable", self.target.display()),
            ));
        }
        let permissions = std::fs::metadata(&self.target)?.permissions();
        let temporary = folder.join(format!(
            ".{}.quadratique-{}",
            file_name(self.shown),
            std::process::id()
        ));
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .and_then(|mut file| file.write_all(bytes))
            .and_then(|()| std::fs::set_permissions(&temporary, permissions))
            .and_then(|()| std::fs::rename(&temporary, &self.target));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written.map(|()| Done::Done)
    }
}

fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn git(root: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    environment::without_trace(&mut command)
        .arg("-C")
        .arg(root)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}

fn uncommitted(root: &Path, path: &str) -> bool {
    git(root)
        .args([
            "--literal-pathspecs",
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            path,
        ])
        .output()
        .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
}

/// Whether git's index still holds `path` with one of the texts in `digests`.
fn indexed_as(root: &Path, path: &str, digests: &[&str]) -> bool {
    git(root)
        .args(["cat-file", "blob", &format!(":{path}")])
        .output()
        .is_ok_and(|output| output.status.success() && shipped::is_listed(&output.stdout, digests))
}

fn shell_word(path: &str) -> String {
    if path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    {
        path.to_owned()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

fn skill_content(file: &SkillFile) -> impl Fn(Vec<u8>) -> Content {
    |bytes| {
        if bytes == file.text.as_bytes() {
            Content::Same(bytes)
        } else if shipped::is_listed(&bytes, file.shipped) {
            Content::Earlier(bytes)
        } else {
            Content::Different(bytes)
        }
    }
}

fn agents_content(bytes: Vec<u8>) -> Content {
    match agents_line::find(&bytes, AGENTS_LINE) {
        agents_line::Found::Current => Content::Same(bytes),
        agents_line::Found::Earlier => Content::Earlier(bytes),
        agents_line::Found::Absent => Content::Different(bytes),
    }
}

enum ProjectStep {
    Bound(Value),
    Create {
        key: String,
        checked_input: Value,
        from: String,
    },
    /// A dry run found no running server to ask, and starts none.
    NotLookedUp(String),
}

/// What a path of Quadratic's skill folder, from before the rename, is.
enum Retired {
    /// A regular file that holds a text Quadratic wrote there.
    Remove {
        shown: &'static str,
        target: PathBuf,
        bytes: Vec<u8>,
    },
    Keep {
        shown: &'static str,
        what: Kept,
    },
    /// Nothing there now, though git's index may still hold Quadratic's text.
    Absent {
        shown: &'static str,
    },
}

enum Kept {
    Changed,
    Link,
    Directory,
    NotAFile,
}

impl Retired {
    /// Classifies the path without following any link: only a regular file, reached through no
    /// link, is read.
    fn plan(root: &Path, shown: &'static str, digests: &[&str]) -> std::io::Result<Retired> {
        let lexical = root.join(shown);
        let keep = |what| Ok(Retired::Keep { shown, what });
        let folder = lexical.parent().unwrap_or(root);
        match folder.canonicalize() {
            Ok(real) if real != folder => return keep(Kept::Link),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return match std::fs::symlink_metadata(folder) {
                    Ok(_) => keep(Kept::Link),
                    Err(_) => Ok(Retired::Absent { shown }),
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => {
                return keep(Kept::NotAFile);
            }
            Err(error) => return Err(error),
        }
        let meta = match std::fs::symlink_metadata(&lexical) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Retired::Absent { shown });
            }
            Err(error) => return Err(error),
        };
        let kind = meta.file_type();
        if kind.is_symlink() {
            return keep(Kept::Link);
        }
        if kind.is_dir() {
            return keep(Kept::Directory);
        }
        if !kind.is_file() {
            return keep(Kept::NotAFile);
        }
        let bytes = std::fs::read(&lexical)?;
        if shipped::is_listed(&bytes, digests) {
            Ok(Retired::Remove {
                shown,
                target: lexical,
                bytes,
            })
        } else {
            keep(Kept::Changed)
        }
    }

    fn kept_word(what: &Kept) -> &'static str {
        match what {
            Kept::Changed => KEPT,
            Kept::Link => "a link: kept",
            Kept::Directory => "a directory: kept",
            Kept::NotAFile => "not a regular file: kept",
        }
    }

    fn kept_warning(shown: &str, what: &Kept) -> String {
        let name = file_name(shown);
        match what {
            Kept::Changed => format!(
                "{shown} was kept: a person changed it. Quadratique reads {SKILL_FOLDER}/{name}, not this folder: merge what you need of {shown} into {SKILL_FOLDER}/{name} (this build's text: quadratique init --print {name}), then remove {shown}."
            ),
            Kept::Link | Kept::Directory | Kept::NotAFile => format!(
                "{shown} was kept: init removes only a regular file that holds Quadratic's text, and this is {}. Quadratique reads {SKILL_FOLDER}/; remove {shown} by hand when nothing needs it.",
                match what {
                    Kept::Link => "a link, or reached through one",
                    Kept::Directory => "a directory",
                    _ => "not a regular file",
                }
            ),
        }
    }
}

struct Plan {
    root: PathBuf,
    client: Option<HttpClient>,
    skills: Vec<(File, &'static str)>,
    retired: Vec<Retired>,
    agents: File,
    project: ProjectStep,
    dry_run: bool,
}

pub async fn init(words: &[&str], io: &mut CliIo) -> i32 {
    if let ["--print", name] = words {
        return print_text(name, io);
    }
    match check(words, io).await {
        Ok(plan) => apply(plan, io).await,
        Err(code) => code,
    }
}

/// The server to ask about projects. A dry run asks only a server that already runs, so that it
/// writes nothing; when there is none it says why.
async fn server(dry_run: bool) -> Result<Result<HttpClient, String>, i32> {
    if !dry_run {
        return crate::remote(crate::route::McpTool::Absent).await.map(Ok);
    }
    let home = match commands::home() {
        Ok(home) => home,
        Err(message) => return Ok(Err(message)),
    };
    let runtime = match background::find(&home).await {
        Ok(Found::Ours(runtime)) => runtime,
        Ok(Found::Nobody) => {
            return Ok(Err(
                "no Quadratique server runs, and a dry run starts none".to_owned()
            ));
        }
        Ok(Found::OtherInterface { .. }) => {
            return Ok(Err(
                "the server that runs is of another build, and a dry run replaces none".to_owned(),
            ));
        }
        Ok(Found::Refused { reason }) => return Ok(Err(reason)),
        Err(error) => return Ok(Err(error.to_string())),
    };
    let caller = caller::detect(Binding::Cli)
        .map_err(|error| report_code(Kind::InvalidInput, error.to_string()))?;
    Ok(caller::client(&caller, &runtime.url))
}

fn report_code(kind: Kind, message: String) -> i32 {
    commands::report_to(&mut |line| eprintln!("{line}"), kind, message)
}

async fn check(words: &[&str], io: &mut CliIo) -> Result<Plan, i32> {
    let Some(request) = request(words) else {
        return Err(report(
            io,
            Kind::InvalidInput,
            format!("Give: quadratique {USAGE}"),
        ));
    };
    let caller = match caller::detect(Binding::Cli) {
        Ok(caller) => caller,
        Err(error) => return Err(report(io, Kind::InvalidInput, error.to_string())),
    };
    let (Some(repository), Some(root)) = (
        caller.working_copy.repository.clone(),
        caller.working_copy.worktree.clone().map(PathBuf::from),
    ) else {
        return Err(report(
            io,
            Kind::InvalidInput,
            "quadratique init runs inside a git repository.",
        ));
    };
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(error) => return Err(report(io, Kind::Internal, error.to_string())),
    };
    let folder = Path::new(&repository)
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();

    let planned = SKILL_FILES
        .iter()
        .map(|file| {
            File::plan(&root, file.path, &skill_content(file)).map(|planned| (planned, file.text))
        })
        .collect::<Result<Vec<_>, _>>()
        .and_then(|skills| {
            let retired = shipped::QUADRATIC_SKILL_FILES
                .iter()
                .map(|&(path, digests)| {
                    Retired::plan(&root, path, digests).map_err(|error| format!("{path}: {error}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok((
                skills,
                retired,
                File::plan(&root, AGENTS_PATH, &agents_content)?,
            ))
        });
    let (skills, retired, agents) = match planned {
        Ok(planned) => planned,
        Err(refusal) => {
            return Err(report(
                io,
                Kind::InvalidInput,
                format!("{refusal} Nothing was changed."),
            ));
        }
    };
    let targets: Vec<(&str, &Path)> = skills
        .iter()
        .map(|(file, _)| (file.shown, file.target.as_path()))
        .chain(retired.iter().filter_map(|retired| match retired {
            Retired::Remove { shown, target, .. } => Some((*shown, target.as_path())),
            Retired::Keep { .. } | Retired::Absent { .. } => None,
        }))
        .chain([(agents.shown, agents.target.as_path())])
        .collect();
    for (index, (shown, target)) in targets.iter().enumerate() {
        if let Some((other, _)) = targets[..index].iter().find(|(_, other)| other == target) {
            return Err(report(
                io,
                Kind::InvalidInput,
                format!(
                    "{other} and {shown} both lead to {}: quadratique init writes them as separate files. Nothing was changed.",
                    target.display()
                ),
            ));
        }
    }
    let client = match server(request.dry_run).await? {
        Ok(client) => client,
        Err(reason) => {
            return Ok(Plan {
                root,
                client: None,
                skills,
                retired,
                agents,
                project: ProjectStep::NotLookedUp(reason),
                dry_run: request.dry_run,
            });
        }
    };
    let projects = match client.invoke("project.list", &json!({})).await {
        Outcome::Value(value) => value,
        Outcome::Failure(failure) => {
            print_json(io, &failure);
            return Err(exit::FAILURE);
        }
        Outcome::CallFailure(failure) => return Err(report(io, failure.kind, failure.message)),
    };
    let bound = projects["items"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|project| project["repository"] == json!(repository))
        .cloned();
    let project = match bound {
        Some(project) => ProjectStep::Bound(project),
        None => {
            let Some(key) = request.key.or_else(|| key_of(&folder)) else {
                return Err(report(
                    io,
                    Kind::InvalidInput,
                    format!(
                        "The folder's name {folder:?} gives no project key: give --key ABC. Nothing was changed."
                    ),
                ));
            };
            let taken = projects["items"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|project| project["key"] == json!(key));
            if let Some(project) = taken {
                return Err(report(
                    io,
                    Kind::InvalidInput,
                    format!(
                        "The key {key} is taken by the project of {}: give another --key. Nothing was changed.",
                        project["repository"]
                            .as_str()
                            .unwrap_or("another repository")
                    ),
                ));
            }
            let (default_branch, from) = match request.default_branch {
                Some(branch) => (branch, "--default-branch".to_owned()),
                None => match default_branch::named_by(Path::new(&repository)) {
                    Some((branch, source)) => (branch, source.describe().to_owned()),
                    None => (
                        "main".to_owned(),
                        "nothing in the repository names one: give --default-branch".to_owned(),
                    ),
                },
            };
            let name = request.name.unwrap_or_else(|| folder.clone());
            let input = json!({ "key": key, "name": name, "defaultBranch": default_branch });
            if let Some(refusal) = input_refusal("project.create", &input) {
                return Err(report(
                    io,
                    Kind::InvalidInput,
                    format!("The project cannot be created: {refusal}. Nothing was changed."),
                ));
            }
            ProjectStep::Create {
                key,
                checked_input: input,
                from,
            }
        }
    };
    Ok(Plan {
        root,
        client: Some(client),
        skills,
        retired,
        agents,
        project,
        dry_run: request.dry_run,
    })
}

#[derive(Default)]
struct Progress {
    done: Vec<String>,
    committed: Vec<String>,
    warnings: Vec<String>,
}

impl Progress {
    fn failed(&self, io: &mut CliIo, step: &str, error: String) -> i32 {
        let before = if self.done.is_empty() {
            "Nothing was changed".to_owned()
        } else {
            format!("Done before it: {}", self.done.join("; "))
        };
        report(
            io,
            Kind::Internal,
            format!(
                "{step} failed: {error}. {before}. Run quadratique init again once that is fixed: it does only what is left."
            ),
        )
    }
}

async fn apply(plan: Plan, io: &mut CliIo) -> i32 {
    let mut progress = Progress::default();
    let skill_words = match write_skills(&plan, &mut progress) {
        Ok(words) => words,
        Err((step, error)) => return progress.failed(io, &step, error),
    };
    let retired_words = match remove_retired(&plan, &mut progress) {
        Ok(words) => words,
        Err((step, error)) => return progress.failed(io, &step, error),
    };
    let project = match create_project(&plan, &mut progress).await {
        Ok(project) => project,
        Err((step, error)) => return progress.failed(io, &step, error),
    };
    let agents_word = match add_agents_line(&plan, &mut progress) {
        Ok(word) => word,
        Err((step, error)) => return progress.failed(io, &step, error),
    };

    let mut files: Map<String, Value> = skill_words
        .into_iter()
        .chain(retired_words)
        .map(|(path, word)| (path.to_owned(), json!(word)))
        .collect();
    files.insert(AGENTS_PATH.into(), json!(agents_word));
    let mut answer = if progress.warnings.is_empty() {
        json!({ "files": files, "project": project })
    } else {
        json!({ "warnings": progress.warnings, "files": files, "project": project })
    };
    if plan.dry_run {
        answer["dryRun"] = json!(true);
    }
    let links: Map<String, Value> = plan
        .skills
        .iter()
        .map(|(file, _)| file)
        .chain([&plan.agents])
        .filter(|file| file.linked)
        .map(|file| (file.shown.to_owned(), json!(file.committed(&plan.root))))
        .collect();
    if !links.is_empty() {
        let field = if plan.dry_run {
            "wouldWriteThroughLinks"
        } else {
            "writtenThroughLinks"
        };
        answer[field] = Value::Object(links);
    }
    if !plan.dry_run && !progress.committed.is_empty() {
        let paths = progress
            .committed
            .iter()
            .map(|path| shell_word(path))
            .collect::<Vec<_>>()
            .join(" ");
        let git = format!(
            "git -C {} --literal-pathspecs",
            shell_word(&plan.root.display().to_string())
        );
        answer["commit"] = json!(format!(
            "{git} add -- {paths} && {git} commit -m \"{COMMIT_MESSAGE}\" -- {paths}"
        ));
    }
    print_json(io, &answer);
    exit::VALUE
}

type StepFailure = (String, String);

/// The word for a change: what was done, or with --dry-run what would be done, or why it would
/// fail.
fn said(
    plan: &Plan,
    target: &Path,
    done: &str,
    would: &str,
    shown: &str,
    progress: &mut Progress,
) -> String {
    if !plan.dry_run {
        return done.to_owned();
    }
    match write_blocker(target) {
        Some(reason) => {
            progress.warnings.push(blocked_warning(shown, &reason));
            format!("would fail: {reason}")
        }
        None => would.to_owned(),
    }
}

fn write_skills(
    plan: &Plan,
    progress: &mut Progress,
) -> Result<Vec<(&'static str, String)>, StepFailure> {
    plan.skills
        .iter()
        .map(|(skill, text)| Ok((skill.shown, write_skill(skill, text, plan, progress)?)))
        .collect()
}

fn write_skill(
    skill: &File,
    text: &str,
    plan: &Plan,
    progress: &mut Progress,
) -> Result<String, StepFailure> {
    let shown = skill.shown;
    let root = &plan.root;
    let (done, would) = match &skill.content {
        Content::Same(_) => {
            if !plan.dry_run && skill.uncommitted(root) {
                progress.committed.push(skill.committed(root));
            }
            return Ok("unchanged".to_owned());
        }
        Content::Different(_) => {
            progress.warnings.push(kept_warning(shown));
            return Ok(KEPT.to_owned());
        }
        Content::Absent => ("created", "would create"),
        Content::Earlier(_) => (
            "updated: it held an earlier Quadratique text",
            "would update: it holds an earlier Quadratique text",
        ),
    };
    if !plan.dry_run {
        let written = skill
            .write(text.as_bytes())
            .map_err(|error| (format!("Writing {shown}"), error.to_string()))?;
        if let Done::ChangedMeanwhile = written {
            progress.warnings.push(changed_meanwhile(shown));
            return Ok(KEPT.to_owned());
        }
        progress.done.push(format!("wrote {shown}"));
        progress.committed.push(skill.committed(root));
    }
    Ok(said(plan, &skill.target, done, would, shown, progress))
}

/// Removes `target` when it still holds `bytes`, and then its folder when that is empty.
fn remove_if_unchanged(target: &Path, bytes: &[u8]) -> std::io::Result<Done> {
    if read(target)?.as_deref() != Some(bytes) {
        return Ok(Done::ChangedMeanwhile);
    }
    std::fs::remove_file(target)?;
    if let Some(folder) = target.parent() {
        let _ = std::fs::remove_dir(folder);
    }
    Ok(Done::Done)
}

const REMOVED: &str = "removed: Quadratic's text, now in .agents/skills/quadratique/";

fn remove_retired(
    plan: &Plan,
    progress: &mut Progress,
) -> Result<Vec<(&'static str, String)>, StepFailure> {
    let mut words = Vec::new();
    for (retired, (_, digests)) in plan.retired.iter().zip(shipped::QUADRATIC_SKILL_FILES) {
        match retired {
            Retired::Keep { shown, what } => {
                progress.warnings.push(Retired::kept_warning(shown, what));
                words.push((*shown, Retired::kept_word(what).to_owned()));
            }
            Retired::Absent { shown } => {
                if !plan.dry_run && indexed_as(&plan.root, shown, digests) {
                    progress.committed.push((*shown).to_owned());
                    words.push((*shown, "removed earlier: the commit records it".to_owned()));
                }
            }
            Retired::Remove {
                shown,
                target,
                bytes,
            } => {
                if plan.dry_run {
                    let folder = target.parent().unwrap_or(&plan.root);
                    let word = match (!writable(folder))
                        .then(|| format!("{} is not writable", folder.display()))
                    {
                        Some(reason) => {
                            progress.warnings.push(blocked_warning(shown, &reason));
                            format!("would fail: {reason}")
                        }
                        None => {
                            "would remove: Quadratic's text, now in .agents/skills/quadratique/"
                                .to_owned()
                        }
                    };
                    words.push((*shown, word));
                    continue;
                }
                let failed =
                    |error: std::io::Error| (format!("Removing {shown}"), error.to_string());
                if let Done::ChangedMeanwhile =
                    remove_if_unchanged(target, bytes).map_err(failed)?
                {
                    progress.warnings.push(changed_meanwhile(shown));
                    words.push((*shown, KEPT.to_owned()));
                    continue;
                }
                progress.done.push(format!("removed {shown}"));
                if uncommitted(&plan.root, shown) {
                    progress.committed.push((*shown).to_owned());
                }
                words.push((*shown, REMOVED.to_owned()));
            }
        }
    }
    Ok(words)
}

async fn create_project(plan: &Plan, progress: &mut Progress) -> Result<Value, StepFailure> {
    let failed = |message: String| ("Creating the project".to_owned(), message);
    match (&plan.project, &plan.client) {
        (ProjectStep::NotLookedUp(reason), _) => Ok(json!({
            "lookedUp": false,
            "reason": reason,
        })),
        (ProjectStep::Bound(project), _) => Ok(json!({
            "key": project["key"],
            "created": false,
            "defaultBranch": project["defaultBranch"],
        })),
        (
            ProjectStep::Create {
                key,
                checked_input,
                from,
            },
            _,
        ) if plan.dry_run => Ok(json!({
            "key": key,
            "created": false,
            "wouldCreate": true,
            "defaultBranch": checked_input["defaultBranch"],
            "defaultBranchFrom": from,
        })),
        (ProjectStep::Create { .. }, None) => Err(failed("no server to ask".to_owned())),
        (
            ProjectStep::Create {
                key,
                checked_input,
                from,
            },
            Some(client),
        ) => match client.invoke("project.create", checked_input).await {
            Outcome::Value(value) => {
                progress.done.push(format!("created the project {key}"));
                Ok(json!({
                    "key": value["key"],
                    "created": true,
                    "defaultBranch": value["defaultBranch"],
                    "defaultBranchFrom": from,
                }))
            }
            Outcome::Failure(failure) => {
                Err(failed(serde_json::to_string(&failure).unwrap_or_default()))
            }
            Outcome::CallFailure(failure) => Err(failed(failure.message)),
        },
    }
}

fn add_agents_line(plan: &Plan, progress: &mut Progress) -> Result<String, StepFailure> {
    let agents = &plan.agents;
    let (bytes, done, would) = match &agents.content {
        Content::Same(_) => {
            if !plan.dry_run && agents.uncommitted(&plan.root) {
                progress.committed.push(agents.committed(&plan.root));
            }
            return Ok("unchanged".to_owned());
        }
        Content::Earlier(bytes) => (
            agents_line::updated(bytes, AGENTS_LINE),
            "updated: it held Quadratic's line",
            "would update: it holds Quadratic's line",
        ),
        Content::Different(bytes) => (
            agents_line::updated(bytes, AGENTS_LINE),
            "changed",
            "would change",
        ),
        Content::Absent => (
            agents_line::updated(b"", AGENTS_LINE),
            "created",
            "would create",
        ),
    };
    if !plan.dry_run {
        let written = agents
            .write(&bytes)
            .map_err(|error| (format!("Writing {AGENTS_PATH}"), error.to_string()))?;
        if let Done::ChangedMeanwhile = written {
            progress.warnings.push(changed_meanwhile(AGENTS_PATH));
            return Ok("not written: it changed while init ran".to_owned());
        }
        progress.committed.push(agents.committed(&plan.root));
    }
    Ok(said(
        plan,
        &agents.target,
        done,
        would,
        AGENTS_PATH,
        progress,
    ))
}

#[cfg(test)]
mod tests {
    use super::{Content, Done, File, key_of, remove_if_unchanged};

    fn file(target: std::path::PathBuf, content: Content) -> File {
        File {
            shown: "SKILL.md",
            target,
            linked: false,
            content,
        }
    }

    #[test]
    fn a_file_that_changed_after_init_read_it_is_not_replaced() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("SKILL.md");
        std::fs::write(&target, "older text\n").unwrap();
        let planned = file(target.clone(), Content::Earlier(b"older text\n".to_vec()));
        std::fs::write(&target, "older text\nand a person's line\n").unwrap();

        assert!(matches!(
            planned.write(b"new text\n").unwrap(),
            Done::ChangedMeanwhile
        ));
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "older text\nand a person's line\n"
        );
    }

    #[test]
    fn a_file_created_after_init_found_none_is_not_overwritten() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("SKILL.md");
        let planned = file(target.clone(), Content::Absent);
        std::fs::write(&target, "a person's file\n").unwrap();

        assert!(matches!(
            planned.write(b"new text\n").unwrap(),
            Done::ChangedMeanwhile
        ));
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "a person's file\n"
        );
    }

    #[test]
    fn a_file_that_changed_after_init_read_it_is_not_removed() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("SKILL.md");
        std::fs::write(&target, "a person's file\n").unwrap();

        assert!(matches!(
            remove_if_unchanged(&target, b"Quadratic's text\n").unwrap(),
            Done::ChangedMeanwhile
        ));
        assert!(target.exists());
        assert!(matches!(
            remove_if_unchanged(&target, b"a person's file\n").unwrap(),
            Done::Done
        ));
        assert!(!folder.path().exists());
    }

    #[test]
    fn a_folder_name_gives_its_letters_and_digits_in_upper_case_from_the_first_letter() {
        assert_eq!(key_of("tally").as_deref(), Some("TALLY"));
        assert_eq!(key_of("2-my_repo.rs").as_deref(), Some("MYREPORS"));
        assert_eq!(key_of("quadratique-tracker").as_deref(), Some("QUADRATIQU"));
        assert_eq!(key_of("x"), None);
        assert_eq!(key_of("--"), None);
    }
}
