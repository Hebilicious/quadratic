use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

const GENERIC_COMMANDS: [&str; 3] = ["call", "list", "watch"];
const HELP_FLAGS: [&str; 2] = ["--help", "-h"];
const VERSION_FLAGS: [&str; 2] = ["--version", "-v"];
const OUTPUT_FLAG: &str = "--json";

pub struct Words {
    server_words: BTreeSet<String>,
    aliases: BTreeMap<String, String>,
    default: Option<String>,
    commands: BTreeSet<String>,
    mcp_tools: BTreeSet<String>,
}

pub enum McpTool {
    Offered(String),
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

    pub fn mcp_tool(&self, args: &[String]) -> McpTool {
        let words = command_words(args);
        let capability = match words[..] {
            [] => self.default.clone(),
            ["list" | "watch", ..] => return McpTool::Absent,
            ["call", id, ..] => Some(id.to_owned()),
            [first, ..] if self.aliases.contains_key(first) => self.aliases.get(first).cloned(),
            [noun, verb, ..] => Some(format!("{noun}.{verb}")),
            _ => None,
        };
        match capability {
            Some(id) if self.mcp_tools.contains(&id) => McpTool::Offered(id),
            Some(id) if self.commands.contains(&id) => McpTool::Absent,
            _ => McpTool::NotACommand,
        }
    }
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
