use super::error::StoreError;
use super::types::Call;
use serde_json::{Value, json};

const COMMITTER_EMAIL: &str = "actor@quadratic.local";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) operation: String,
    pub(crate) actor: String,
    pub(crate) reason: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) worktree: Option<String>,
    pub(crate) at: String,
}

impl Message {
    pub(crate) fn of(call: &Call, at: &str) -> Message {
        Message {
            operation: call.operation.clone(),
            actor: call.author.record().to_owned(),
            reason: call.reason.clone(),
            branch: call.branch.clone(),
            worktree: call.worktree.clone(),
            at: at.to_owned(),
        }
    }

    pub(crate) fn encode(&self) -> String {
        let actor: Value = serde_json::from_str(&self.actor).unwrap_or(Value::Null);
        let body = json!({
            "operation": self.operation,
            "actor": actor,
            "reason": self.reason,
            "branch": self.branch,
            "worktree": self.worktree,
            "at": self.at,
        });
        format!("{}\n\n{}", self.operation, body)
    }

    pub(crate) fn decode(text: &str) -> Result<Message, StoreError> {
        let corrupt = || StoreError::Corrupt(text.chars().take(200).collect());
        let (_, body) = text.split_once("\n\n").ok_or_else(corrupt)?;
        let body: Value = serde_json::from_str(body).map_err(|_| corrupt())?;
        let member = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_owned);
        let actor = body.get("actor").filter(|actor| actor.is_object());
        Ok(Message {
            operation: member("operation").ok_or_else(corrupt)?,
            actor: actor.ok_or_else(corrupt)?.to_string(),
            reason: member("reason"),
            branch: member("branch"),
            worktree: member("worktree"),
            at: member("at").ok_or_else(corrupt)?,
        })
    }

    pub(crate) fn is_ours(text: &str) -> bool {
        Message::decode(text).is_ok()
    }
}

// doltlite 0.51.13 parses `--author` in segments split at ')', so a name such as
// `claude-code(t3)@x` would be stored as `@x`, and `<` or `>` would end the name early.
pub(crate) fn committer(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| match c {
            '(' => '（',
            ')' => '）',
            '<' => '＜',
            '>' => '＞',
            control if control.is_control() => ' ',
            other => other,
        })
        .collect();
    format!("{name} <{COMMITTER_EMAIL}>")
}
