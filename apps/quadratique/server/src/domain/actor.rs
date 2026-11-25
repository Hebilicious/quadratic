use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

pub type ActorName = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorKind {
    Human,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Actor {
    pub kind: ActorKind,
    pub name: ActorName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ActorName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    Cli,
    Mcp,
    Web,
    WebMcp,
    NoActorHeader,
    Server,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActorError {
    InvalidRole { role: String },
}

impl fmt::Display for ActorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActorError::InvalidRole { role } => write!(
                formatter,
                "QUADRATIQUE_ROLE is {role:?}, which is not a role: give a slug of lowercase letters, digits and single hyphens, at most {ROLE_MAX} characters, such as `security`."
            ),
        }
    }
}

impl std::error::Error for ActorError {}

const ROLE_VARIABLE: &str = "QUADRATIQUE_ROLE";
const FIELDS_VARIABLE: &str = "QUADRATIQUE_ACTOR_FIELDS";

const AGENT_VARIABLES: [(&str, Option<&str>); 12] = [
    ("AI_AGENT", None),
    ("CLAUDECODE", Some("claude-code")),
    ("CODEX_THREAD_ID", Some("codex")),
    ("CURSOR_AGENT", None),
    ("GEMINI_CLI", None),
    ("OPENCODE", None),
    ("AMP_CURRENT_THREAD_ID", None),
    ("AUGMENT_AGENT", None),
    ("QWEN_CODE_SESSION_ID", None),
    ("PI_CODING_AGENT", None),
    ("CRUSH", None),
    ("COPILOT_AGENT", None),
];
const FALSE_VALUES: [&str; 5] = ["", "0", "false", "no", "off"];
const TRIM_CHARACTERS: [char; 6] = [' ', '\t', '\n', '\u{b}', '\u{c}', '\r'];

const HOST_VARIABLES: [(&str, &str); 1] = [("T3CODE_THREAD_ID", "t3")];
const SESSION_VARIABLES: [(&str, &str); 2] = [
    ("claude-code", "CLAUDE_CODE_SESSION_ID"),
    ("codex", "CODEX_THREAD_ID"),
];

const SERVER_NAME: &str = "quadratique";
const NO_HEADER_NAME: &str = "agent";
const BROWSER_NAME: &str = "browser";
const NO_LOGIN_NAME: &str = "human";

/// The names the server gives actors of its own: none of them can be taken by an agent token.
pub const FIXED_ACTOR_NAMES: [&str; 4] = [SERVER_NAME, NO_HEADER_NAME, BROWSER_NAME, NO_LOGIN_NAME];

const ROLE_MAX: usize = 32;
const HARNESS_MAX: usize = 64;
const HOST_MAX: usize = 64;
const HOST_THREAD_MAX: usize = 200;
const SESSION_MAX: usize = 200;
const MODEL_MAX: usize = 128;
const NAME_MAX: usize = 420;
const WORKTREE_NAME_MAX: usize = 255;

impl Actor {
    /// The harness and the session id of this actor, when it gives both: a session id means
    /// something only within its harness. Like every actor field, both are self-declared.
    pub fn harness_session(&self) -> Option<(&str, &str)> {
        Some((self.harness.as_deref()?, self.session.as_deref()?))
    }

    pub fn server() -> Actor {
        Actor {
            harness: Some(SERVER_NAME.into()),
            ..Actor::bare_agent(SERVER_NAME)
        }
    }

    pub fn without_header() -> Actor {
        Actor::bare_agent(NO_HEADER_NAME)
    }

    pub fn human(login: Option<&str>) -> Actor {
        Actor {
            kind: ActorKind::Human,
            name: login
                .and_then(|login| text_member(login, NAME_MAX))
                .unwrap_or_else(|| NO_LOGIN_NAME.into()),
            ..Actor::bare_agent("")
        }
    }

    fn bare_agent(name: &str) -> Actor {
        Actor {
            kind: ActorKind::Agent,
            name: name.into(),
            role: None,
            harness: None,
            host: None,
            host_thread: None,
            parent: None,
            model: None,
            session: None,
        }
    }

    pub fn detect(
        env: &BTreeMap<String, String>,
        binding: Binding,
        worktree: Option<&str>,
        login: Option<&str>,
    ) -> Result<Actor, ActorError> {
        match binding {
            Binding::Server => return Ok(Actor::server()),
            Binding::NoActorHeader => return Ok(Actor::without_header()),
            Binding::Web => return Ok(Actor::human(login)),
            Binding::Cli | Binding::Mcp | Binding::WebMcp => {}
        }
        let variable = |name: &str| env.get(name).map(String::as_str).filter(|v| !v.is_empty());
        let fields = Overrides::parse(variable(FIELDS_VARIABLE));

        let role = variable(ROLE_VARIABLE)
            .map(|role| {
                if is_slug(role) && role.chars().count() <= ROLE_MAX {
                    Ok(role.to_string())
                } else {
                    Err(ActorError::InvalidRole { role: role.into() })
                }
            })
            .transpose()?;

        let first_truthy_agent_variable = AGENT_VARIABLES
            .iter()
            .find(|(name, _)| variable(name).is_some_and(is_truthy));

        let kind = if matches!(binding, Binding::Mcp | Binding::WebMcp)
            || role.is_some()
            || first_truthy_agent_variable.is_some()
        {
            ActorKind::Agent
        } else {
            ActorKind::Human
        };

        let harness = [
            (binding == Binding::WebMcp).then_some(BROWSER_NAME),
            fields.get("harness"),
            variable("AI_AGENT").and_then(|value| value.split('_').next()),
            first_truthy_agent_variable.and_then(|(_, harness)| *harness),
        ]
        .into_iter()
        .flatten()
        .find_map(|value| slug_member(value, HARNESS_MAX));

        let detected_host = HOST_VARIABLES.iter().find_map(|(name, host)| {
            variable(name).map(|thread| (slug_member(host, HOST_MAX), thread))
        });
        let (host, host_variable_thread) = match fields
            .get("host")
            .and_then(|host| slug_member(host, HOST_MAX))
        {
            Some(host) => (Some(host), None),
            None => match detected_host {
                Some((host, thread)) => (host, Some(thread)),
                None => (None, None),
            },
        };
        let host_thread = fields
            .get("hostThread")
            .or(host_variable_thread)
            .and_then(|thread| text_member(thread, HOST_THREAD_MAX));

        let session = fields
            .get("session")
            .or_else(|| {
                let harness = harness.as_deref()?;
                SESSION_VARIABLES
                    .iter()
                    .find(|(owner, _)| *owner == harness)
                    .and_then(|(_, name)| variable(name))
            })
            .and_then(|session| text_member(session, SESSION_MAX));

        let mut actor = Actor {
            kind,
            name: String::new(),
            role,
            harness,
            host,
            host_thread,
            parent: fields
                .get("parent")
                .and_then(|parent| text_member(parent, NAME_MAX)),
            model: fields
                .get("model")
                .and_then(|model| text_member(model, MODEL_MAX)),
            session,
        };
        actor.name = match kind {
            ActorKind::Human => Actor::human(login).name,
            ActorKind::Agent => agent_name(&actor, worktree),
        };
        Ok(actor)
    }
}

fn agent_name(actor: &Actor, worktree: Option<&str>) -> ActorName {
    let mut name = String::new();
    if let Some(role) = &actor.role {
        name.push_str(&format!("[{role}]"));
    }
    name.push_str(actor.harness.as_deref().unwrap_or(NO_HEADER_NAME));
    if let Some(host) = &actor.host {
        name.push_str(&format!("({host})"));
    }
    if let Some(directory) = worktree
        .and_then(|path| path.trim_end_matches('/').rsplit('/').next())
        .filter(|directory| !directory.is_empty())
    {
        name.push('@');
        name.push_str(&cut(directory, WORKTREE_NAME_MAX));
    }
    name
}

struct Overrides(BTreeMap<String, String>);

impl Overrides {
    fn parse(value: Option<&str>) -> Overrides {
        let object = value
            .and_then(|text| serde_json::from_str::<BTreeMap<String, serde_json::Value>>(text).ok())
            .unwrap_or_default();
        Overrides(
            object
                .into_iter()
                .filter_map(|(key, value)| match value {
                    serde_json::Value::String(text) if !text.is_empty() => Some((key, text)),
                    _ => None,
                })
                .collect(),
        )
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
}

fn is_truthy(value: &str) -> bool {
    let value = value
        .trim_matches(|c| TRIM_CHARACTERS.contains(&c))
        .to_ascii_lowercase();
    !FALSE_VALUES.contains(&value.as_str())
}

fn is_slug(value: &str) -> bool {
    let mut parts = value.split('-');
    let starts_with_letter = value.chars().next().is_some_and(|c| c.is_ascii_lowercase());
    starts_with_letter
        && parts.all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

fn cut(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn slug_member(value: &str, max: usize) -> Option<String> {
    let value = cut(&value.to_lowercase(), max);
    is_slug(&value).then_some(value)
}

fn text_member(value: &str, max: usize) -> Option<String> {
    let value = cut(value, max);
    (!value.is_empty()).then_some(value)
}
