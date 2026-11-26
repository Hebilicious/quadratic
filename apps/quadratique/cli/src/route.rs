use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

const GENERIC_COMMANDS: [&str; 3] = ["call", "list", "watch"];
const HELP_FLAGS: [&str; 2] = ["--help", "-h"];
const VERSION_FLAGS: [&str; 2] = ["--version", "-v"];
const OUTPUT_FLAG: &str = "--json";

/// The capabilities that 0.2 renamed, by their old name. The old names still answer, until the
/// release after 0.2.
const RENAMED: [(&str, &str); 2] = [
    ("issue.close", "issue.finish"),
    ("issues.close", "issues.finish"),
];

/// The capabilities over several issues, which take no human's words: those settle one issue.
const SEVERAL: [&str; 2] = ["issues.finish", "issues.close"];

/// The inputs of the human's words, which only the one-issue capabilities take.
const HUMAN_WORD_FLAGS: [&str; 2] = ["humanWords", "confidence"];

/// What a command called by an old name adds on standard error, after its answer.
pub const RENAMED_POINTER: &str =
    "quadratique: `close` is now `finish`; `close` goes away in the release after 0.2.";

pub struct Words {
    server_words: BTreeSet<String>,
    aliases: BTreeMap<String, String>,
    default: Option<String>,
    commands: BTreeSet<String>,
    mcp_tools: BTreeSet<String>,
}

pub enum McpTool {
    Offered(String),
    /// A command-line call over several issues, whose MCP tool takes one issue.
    OnePerIssue(String),
    Absent,
    NotACommand,
}

impl Words {
    pub fn of(catalog: &Value, target: &str) -> Words {
        let cli = catalog.pointer(&format!("/targets/{target}/options/cli"));
        let aliases: BTreeMap<String, String> = cli
            .and_then(|cli| cli["aliases"].as_object())
            .into_iter()
            .flatten()
            .filter_map(|(word, alias)| {
                Some((word.clone(), alias["capability"].as_str()?.to_owned()))
            })
            .collect();
        let default = cli.and_then(|cli| cli["default"].as_str()).map(|word| {
            aliases
                .get(word)
                .cloned()
                .unwrap_or_else(|| word.to_owned())
        });
        let capabilities = || {
            catalog
                .get("capabilities")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
        };
        let commands: BTreeSet<String> = capabilities()
            .filter(|(_, spec)| in_audience(spec, target))
            .map(|(id, _)| id.clone())
            .collect();
        let first_words: Vec<String> = commands
            .iter()
            .filter_map(|id| id.split('.').next().map(str::to_owned))
            .collect();
        let mcp_tools = capabilities()
            .filter(|(_, spec)| in_audience(spec, "mcp"))
            .map(|(id, _)| id.clone())
            .collect();
        Words {
            server_words: GENERIC_COMMANDS
                .iter()
                .map(|word| (*word).to_owned())
                .chain(aliases.keys().cloned())
                .chain(first_words)
                .collect(),
            aliases,
            default,
            commands,
            mcp_tools,
        }
    }

    pub fn call_the_server(&self, args: &[String]) -> bool {
        let words = command_words(args);
        if words.iter().any(|word| HELP_FLAGS.contains(word)) {
            return false;
        }
        match words.first() {
            None => true,
            Some(first) if VERSION_FLAGS.contains(first) => false,
            Some(first) if first.starts_with("--") => true,
            Some(first) => self.server_words.contains(*first),
        }
    }

    /// Whether the command calls a capability by the name it had before 0.2.
    pub fn renamed(&self, args: &[String]) -> bool {
        self.capability(args)
            .ok()
            .flatten()
            .is_some_and(|id| RENAMED.iter().any(|(old, _)| *old == id))
    }

    /// The usage error of a command over several issues that is given the human's words: the
    /// generated command line would only call the flag unknown, so this one also names the
    /// command that takes it. One line, as the command line writes a usage error.
    pub fn human_words_on_several(&self, args: &[String]) -> Option<String> {
        let words = command_words(args);
        if words.iter().any(|word| HELP_FLAGS.contains(word)) {
            return None;
        }
        let called = self.capability(args).ok().flatten()?;
        if !SEVERAL.contains(&called.as_str()) {
            return None;
        }
        let flag = words.iter().find_map(|word| {
            let name = word.strip_prefix("--")?;
            let name = name.split_once('=').map_or(name, |(name, _)| name);
            HUMAN_WORD_FLAGS.contains(&name).then_some(name)
        })?;
        Some(format!(
            "Unknown flag --{flag}. The human's words settle one issue: give them to `quadratique issue finish`."
        ))
    }

    /// The capability that the command calls; `Err` for a command that calls none.
    fn capability(&self, args: &[String]) -> Result<Option<String>, McpTool> {
        let words = command_words(args);
        Ok(match words[..] {
            [] => self.default.clone(),
            ["list" | "watch", ..] => return Err(McpTool::Absent),
            ["call", id, ..] => Some(id.to_owned()),
            [first, ..] if self.aliases.contains_key(first) => self.aliases.get(first).cloned(),
            [noun, verb, ..] => Some(format!("{noun}.{verb}")),
            _ => None,
        })
    }

    pub fn mcp_tool(&self, args: &[String]) -> McpTool {
        let capability = match self.capability(args) {
            Ok(capability) => capability.map(|id| new_name(&id)),
            Err(answer) => return answer,
        };
        match capability {
            Some(id) if self.mcp_tools.contains(&id) => McpTool::Offered(id),
            Some(id) if self.mcp_tools.contains(&one_issue(&id)) => {
                if issue_flags(args) > 1 {
                    McpTool::OnePerIssue(one_issue(&id))
                } else {
                    McpTool::Offered(one_issue(&id))
                }
            }
            Some(id) if self.commands.contains(&id) => McpTool::Absent,
            _ => McpTool::NotACommand,
        }
    }
}

fn new_name(id: &str) -> String {
    RENAMED
        .iter()
        .find(|(old, _)| *old == id)
        .map_or(id, |(_, new)| new)
        .to_owned()
}

/// A command-line capability over several issues, such as `issues.finish`, has an MCP tool for
/// one issue in its place, such as `issue.finish`.
fn one_issue(id: &str) -> String {
    match id.strip_prefix("issues.") {
        Some(verb) => format!("issue.{verb}"),
        None => id.to_owned(),
    }
}

fn issue_flags(args: &[String]) -> usize {
    args.iter()
        .filter(|arg| *arg == "--issue" || arg.starts_with("--issue="))
        .count()
}

fn command_words(args: &[String]) -> Vec<&str> {
    let args = args.strip_prefix(&["--".to_owned()]).unwrap_or(args);
    args.iter()
        .map(String::as_str)
        .take_while(|arg| *arg != "--")
        .filter(|arg| *arg != OUTPUT_FLAG)
        .collect()
}

fn in_audience(spec: &Value, target: &str) -> bool {
    spec["audience"]
        .as_array()
        .is_some_and(|audience| audience.iter().any(|each| each == target))
}
