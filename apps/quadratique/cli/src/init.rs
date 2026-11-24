use std::path::{Path, PathBuf};

use acid_cat_core::cli::{CliIo, exit};
use acid_cat_core::http::HttpClient;
use acid_cat_core::{Kind, Outcome};
use quadratique_server::app::input_refusal;
use quadratique_server::domain::Binding;
use quadratique_server::git::{default_branch, environment};
use serde_json::{Map, Value, json};

use crate::caller;
use crate::commands::{print_json, report};
use crate::shipped;

pub const USAGE: &str = "init [--key <KEY>] [--name <name>] [--default-branch <branch>] [--dry-run] | init --print <SKILL.md|planning.md|AGENTS.md>: set up Quadratique in this git repository, or bring its setup to this build's text after an upgrade. Writes the skill to .agents/skills/quadratique/ (SKILL.md and planning.md), adds the Quadratique line to AGENTS.md, and creates the project bound to the repository (key from --key, else from the folder's name; default branch from --default-branch, else the remote's HEAD, else the branch checked out in the main worktree, else main; a project that has the repository already is left alone). A skill file that holds the text of an earlier build is replaced with this build's; a skill file that holds any other text, which a person wrote, is left as it is. The skill files and the AGENTS.md line of Quadratic, the earlier name, move to Quadratique's. The answer says per file what was done, and its warnings name each kept file with the command that prints this build's text of it. --dry-run changes nothing and answers what init would do. --print writes this build's text of one file (AGENTS.md: its line) to standard output as it is, to merge into a kept file; it needs no repository and no server. Its files stay inside the repository; the project is in the server's store. Writes no agent configuration and makes no commit: when it wrote a setup file, or one of its own is not committed yet, it prints the commit to run.";

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
const AGENTS_PATH: &str = "AGENTS.md";
pub const AGENTS_LINE: &str = "We use Quadratique for issues: every task is an issue there. Use the `quadratique` skill (`.agents/skills/quadratique/SKILL.md`) for anything about tasks.";
const COMMIT_MESSAGE: &str = "Use Quadratique for issues";
const LINK_DEPTH: usize = 40;

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
    use std::io::Write;
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

/// The warning for a skill file kept because a person changed it: it names the file and the
/// command that prints the build's text to merge into it.
fn kept_warning(shown: &str) -> String {
    let name = Path::new(shown)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(shown);
    format!(
        "{shown} was kept: a person changed it, so it lacks this build's text. Merge that text into it: quadratique init --print {name}"
    )
}

fn warnings(plan: &Plan) -> Vec<String> {
    let skills = plan
        .skills
        .iter()
        .filter(|(file, _)| matches!(file.content, Content::Different(_)))
        .map(|(file, _)| kept_warning(file.shown));
    let retired = plan.retired.iter().filter_map(|retired| match retired {
        Retired::Keep(shown, word) if *word == KEPT => Some(format!(
            "{} Quadratique reads .agents/skills/quadratique/, not this folder.",
            kept_warning(shown)
        )),
        Retired::Keep(shown, _) => Some(format!(
            "{shown} was kept: it is a link, which init does not follow to remove. Quadratique reads .agents/skills/quadratique/; remove it by hand when nothing needs it."
        )),
        Retired::Remove(_) => None,
    });
    skills.chain(retired).collect()
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

enum Content {
    Absent,
    Same,
    Earlier(String),
    Different(String),
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

type Classify<'a> = &'a dyn Fn(String) -> Content;

fn inspect(root: &Path, relative: &str, classify: Classify) -> std::io::Result<Destination> {
    let lexical = root.join(relative);
    let target = landing(&lexical, 0)?;
    if !target.starts_with(root) {
        return Ok(Destination::LinkOutside(target));
    }
    let content = match std::fs::read_to_string(&target) {
        Ok(text) => classify(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Content::Absent,
        Err(error) => return Err(error),
    };
    Ok(if target == lexical {
        Destination::Here(content)
    } else {
        Destination::LinkInside { target, content }
    })
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
        self.target
            .strip_prefix(root)
            .unwrap_or(&self.target)
            .display()
            .to_string()
    }

    fn uncommitted(&self, root: &Path) -> bool {
        environment::without_trace(&mut std::process::Command::new("git"))
            .arg("-C")
            .arg(root)
            .args([
                "--literal-pathspecs",
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--",
            ])
            .arg(self.committed(root))
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
    }

    fn remove(&self, root: &Path) -> std::io::Result<()> {
        std::fs::remove_file(&self.target)?;
        if let Some(folder) = self.target.parent()
            && folder == root.join(shipped::QUADRATIC_SKILL_FOLDER)
        {
            let _ = std::fs::remove_dir(folder);
        }
        Ok(())
    }

    fn write(&self, text: &str) -> std::io::Result<()> {
        if let Some(folder) = self.target.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(&self.target, text)
    }
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

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|found| found.trim() == line)
}

fn skill_content(file: &SkillFile) -> impl Fn(String) -> Content {
    |text| {
        if text == file.text {
            Content::Same
        } else if shipped::is_listed(&text, file.shipped) {
            Content::Earlier(text)
        } else {
            Content::Different(text)
        }
    }
}

fn agents_content(text: String) -> Content {
    if has_line(&text, AGENTS_LINE) {
        Content::Same
    } else if shipped::AGENTS_LINES
        .iter()
        .any(|line| has_line(&text, line))
    {
        Content::Earlier(text)
    } else {
        Content::Different(text)
    }
}

fn with_line_updated(text: &str) -> String {
    shipped::AGENTS_LINES
        .iter()
        .find(|line| has_line(text, line))
        .map_or_else(
            || with_line(text),
            |line| text.replacen(line, AGENTS_LINE, 1),
        )
}

fn with_line(text: &str) -> String {
    let separator = if text.is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{text}{separator}{AGENTS_LINE}\n")
}

enum ProjectStep {
    Bound(Value),
    Create {
        key: String,
        checked_input: Value,
        from: String,
    },
}

/// A skill file where Quadratic, before the rename, wrote it.
enum Retired {
    Remove(File),
    Keep(&'static str, &'static str),
}

impl Retired {
    fn plan(
        root: &Path,
        shown: &'static str,
        digests: &[&str],
    ) -> std::io::Result<Option<Retired>> {
        let classify = |text: String| {
            if shipped::is_listed(&text, digests) {
                Content::Earlier(text)
            } else {
                Content::Different(text)
            }
        };
        Ok(match inspect(root, shown, &classify)? {
            Destination::Here(Content::Absent)
            | Destination::LinkInside {
                content: Content::Absent,
                ..
            } => None,
            Destination::Here(content @ Content::Earlier(_)) => Some(Retired::Remove(File {
                shown,
                target: root.join(shown),
                linked: false,
                content,
            })),
            Destination::Here(_) => Some(Retired::Keep(shown, KEPT)),
            Destination::LinkInside { .. } | Destination::LinkOutside(_) => {
                Some(Retired::Keep(shown, "a link: kept"))
            }
        })
    }
}

const KEPT: &str = "different: yours is kept";

struct Plan {
    root: PathBuf,
    client: HttpClient,
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
                .filter_map(|&(path, digests)| {
                    Retired::plan(&root, path, digests)
                        .map_err(|error| format!("{path}: {error}"))
                        .transpose()
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
    let files: Vec<&File> = skills
        .iter()
        .map(|(file, _)| file)
        .chain(retired.iter().filter_map(|retired| match retired {
            Retired::Remove(file) => Some(file),
            Retired::Keep(..) => None,
        }))
        .chain([&agents])
        .collect();
    for (index, file) in files.iter().enumerate() {
        if let Some(other) = files[..index]
            .iter()
            .find(|other| other.target == file.target)
        {
            return Err(report(
                io,
                Kind::InvalidInput,
                format!(
                    "{} and {} both lead to {}: quadratique init writes them as separate files. Nothing was changed.",
                    other.shown,
                    file.shown,
                    file.target.display()
                ),
            ));
        }
    }
    let client = crate::remote(crate::route::McpTool::Absent).await?;
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
        client,
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
    let warnings = warnings(&plan);
    let mut answer = if warnings.is_empty() {
        json!({ "files": files, "project": project })
    } else {
        json!({ "warnings": warnings, "files": files, "project": project })
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

/// The word for a change: what was done, or with --dry-run what would be done.
fn said(dry_run: bool, done: &'static str, would: &'static str) -> &'static str {
    if dry_run { would } else { done }
}

fn write_skills(
    plan: &Plan,
    progress: &mut Progress,
) -> Result<Vec<(&'static str, &'static str)>, StepFailure> {
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
) -> Result<&'static str, StepFailure> {
    let shown = skill.shown;
    let root = &plan.root;
    let (done, would) = match &skill.content {
        Content::Same => {
            if skill.uncommitted(root) {
                progress.committed.push(skill.committed(root));
            }
            return Ok("unchanged");
        }
        Content::Different(_) => return Ok(KEPT),
        Content::Absent => ("created", "would create"),
        Content::Earlier(_) => (
            "updated: it held an earlier Quadratique text",
            "would update: it holds an earlier Quadratique text",
        ),
    };
    if !plan.dry_run {
        skill
            .write(text)
            .map_err(|error| (format!("Writing {shown}"), error.to_string()))?;
        progress.done.push(format!("wrote {shown}"));
        progress.committed.push(skill.committed(root));
    }
    Ok(said(plan.dry_run, done, would))
}

fn remove_retired(
    plan: &Plan,
    progress: &mut Progress,
) -> Result<Vec<(&'static str, &'static str)>, StepFailure> {
    plan.retired
        .iter()
        .map(|retired| match retired {
            Retired::Keep(shown, word) => Ok((*shown, *word)),
            Retired::Remove(file) => {
                let shown = file.shown;
                if !plan.dry_run {
                    file.remove(&plan.root)
                        .map_err(|error| (format!("Removing {shown}"), error.to_string()))?;
                    progress.done.push(format!("removed {shown}"));
                    if file.uncommitted(&plan.root) {
                        progress.committed.push(file.committed(&plan.root));
                    }
                }
                Ok((
                    shown,
                    said(
                        plan.dry_run,
                        "removed: Quadratic's text, now in .agents/skills/quadratique/",
                        "would remove: Quadratic's text, now in .agents/skills/quadratique/",
                    ),
                ))
            }
        })
        .collect()
}

async fn create_project(plan: &Plan, progress: &mut Progress) -> Result<Value, StepFailure> {
    let failed = |message: String| ("Creating the project".to_owned(), message);
    match &plan.project {
        ProjectStep::Bound(project) => Ok(json!({
            "key": project["key"],
            "created": false,
            "defaultBranch": project["defaultBranch"],
        })),
        ProjectStep::Create {
            key,
            checked_input,
            from,
        } if plan.dry_run => Ok(json!({
            "key": key,
            "created": false,
            "wouldCreate": true,
            "defaultBranch": checked_input["defaultBranch"],
            "defaultBranchFrom": from,
        })),
        ProjectStep::Create {
            key,
            checked_input,
            from,
        } => match plan.client.invoke("project.create", checked_input).await {
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

fn add_agents_line(plan: &Plan, progress: &mut Progress) -> Result<&'static str, StepFailure> {
    let agents = &plan.agents;
    let (text, done, would) = match &agents.content {
        Content::Same => {
            if agents.uncommitted(&plan.root) {
                progress.committed.push(agents.committed(&plan.root));
            }
            return Ok("unchanged");
        }
        Content::Earlier(text) => (
            with_line_updated(text),
            "updated: it held Quadratic's line",
            "would update: it holds Quadratic's line",
        ),
        Content::Different(text) => (with_line(text), "changed", "would change"),
        Content::Absent => (with_line(""), "created", "would create"),
    };
    if !plan.dry_run {
        agents
            .write(&text)
            .map_err(|error| (format!("Writing {AGENTS_PATH}"), error.to_string()))?;
        progress.committed.push(agents.committed(&plan.root));
    }
    Ok(said(plan.dry_run, done, would))
}

#[cfg(test)]
mod tests {
    use super::key_of;

    #[test]
    fn a_folder_name_gives_its_letters_and_digits_in_upper_case_from_the_first_letter() {
        assert_eq!(key_of("tally").as_deref(), Some("TALLY"));
        assert_eq!(key_of("2-my_repo.rs").as_deref(), Some("MYREPORS"));
        assert_eq!(key_of("quadratique-tracker").as_deref(), Some("QUADRATIQU"));
        assert_eq!(key_of("x"), None);
        assert_eq!(key_of("--"), None);
    }
}
