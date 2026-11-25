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

pub const USAGE: &str = "init [--key <KEY>] [--name <name>] [--default-branch <branch>]: set up Quadratique in this git repository. Writes the skill to .agents/skills/quadratique/ (SKILL.md and planning.md), adds the Quadratique line to AGENTS.md, and creates the project bound to the repository (key from --key, else from the folder's name; default branch from --default-branch, else the remote's HEAD, else the branch checked out in the main worktree, else main; a project that has the repository already is left alone). A skill file you changed is left as it is. Its files stay inside the repository; the project is in the server's store. Writes no agent configuration and makes no commit: when it wrote a setup file, or one of its own is not committed yet, it prints the commit to run.";

const SKILL_FILES: [(&str, &str); 2] = [
    (
        ".agents/skills/quadratique/SKILL.md",
        include_str!("../../skill/quadratique/SKILL.md"),
    ),
    (
        ".agents/skills/quadratique/planning.md",
        include_str!("../../skill/quadratique/planning.md"),
    ),
];
const AGENTS_PATH: &str = "AGENTS.md";
pub const AGENTS_LINE: &str = "We use Quadratique for issues: every task is an issue there. Use the `quadratique` skill (`.agents/skills/quadratique/SKILL.md`) for anything about tasks.";
const COMMIT_MESSAGE: &str = "Use Quadratique for issues";
const LINK_DEPTH: usize = 40;

struct Request {
    key: Option<String>,
    name: Option<String>,
    default_branch: Option<String>,
}

fn request(words: &[&str]) -> Option<Request> {
    let mut request = Request {
        key: None,
        name: None,
        default_branch: None,
    };
    let mut rest = words.iter();
    while let Some(word) = rest.next() {
        let slot = match *word {
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

fn inspect(
    root: &Path,
    relative: &str,
    is_same: &dyn Fn(&str) -> bool,
) -> std::io::Result<Destination> {
    let lexical = root.join(relative);
    let target = landing(&lexical, 0)?;
    if !target.starts_with(root) {
        return Ok(Destination::LinkOutside(target));
    }
    let content = match std::fs::read_to_string(&target) {
        Ok(text) if is_same(&text) => Content::Same,
        Ok(text) => Content::Different(text),
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
    fn plan(
        root: &Path,
        shown: &'static str,
        is_same: &dyn Fn(&str) -> bool,
    ) -> Result<File, String> {
        let destination =
            inspect(root, shown, is_same).map_err(|error| format!("{shown}: {error}"))?;
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

fn has_line(text: &str) -> bool {
    text.lines().any(|line| line.trim() == AGENTS_LINE)
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

struct Plan {
    root: PathBuf,
    client: HttpClient,
    skills: Vec<(File, &'static str)>,
    agents: File,
    project: ProjectStep,
}

pub async fn init(words: &[&str], io: &mut CliIo) -> i32 {
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
        .map(|&(path, text)| {
            File::plan(&root, path, &|found| found == text).map(|file| (file, text))
        })
        .collect::<Result<Vec<_>, _>>()
        .and_then(|skills| Ok((skills, File::plan(&root, AGENTS_PATH, &has_line)?)));
    let (skills, agents) = match planned {
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
        agents,
        project,
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
        .map(|(path, word)| (path.to_owned(), json!(word)))
        .collect();
    files.insert(AGENTS_PATH.into(), json!(agents_word));
    let mut answer = json!({ "files": files, "project": project });
    let links: Map<String, Value> = plan
        .skills
        .iter()
        .map(|(file, _)| file)
        .chain([&plan.agents])
        .filter(|file| file.linked)
        .map(|file| (file.shown.to_owned(), json!(file.committed(&plan.root))))
        .collect();
    if !links.is_empty() {
        answer["writtenThroughLinks"] = Value::Object(links);
    }
    if !progress.committed.is_empty() {
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

fn write_skills(
    plan: &Plan,
    progress: &mut Progress,
) -> Result<Vec<(&'static str, &'static str)>, StepFailure> {
    plan.skills
        .iter()
        .map(|(skill, text)| Ok((skill.shown, write_skill(skill, text, &plan.root, progress)?)))
        .collect()
}

fn write_skill(
    skill: &File,
    text: &str,
    root: &Path,
    progress: &mut Progress,
) -> Result<&'static str, StepFailure> {
    let shown = skill.shown;
    Ok(match &skill.content {
        Content::Absent => {
            skill
                .write(text)
                .map_err(|error| (format!("Writing {shown}"), error.to_string()))?;
            progress.done.push(format!("wrote {shown}"));
            progress.committed.push(skill.committed(root));
            "created"
        }
        Content::Same => {
            if skill.uncommitted(root) {
                progress.committed.push(skill.committed(root));
            }
            "unchanged"
        }
        Content::Different(_) => "different: yours is kept",
    })
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
    let (text, word) = match &agents.content {
        Content::Same => {
            if agents.uncommitted(&plan.root) {
                progress.committed.push(agents.committed(&plan.root));
            }
            return Ok("unchanged");
        }
        Content::Different(text) => (with_line(text), "changed"),
        Content::Absent => (with_line(""), "created"),
    };
    agents
        .write(&text)
        .map_err(|error| (format!("Writing {AGENTS_PATH}"), error.to_string()))?;
    progress.committed.push(agents.committed(&plan.root));
    Ok(word)
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
