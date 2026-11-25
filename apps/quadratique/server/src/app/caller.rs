use std::sync::LazyLock;

use acid_cat_core::remote::Refusal;
use http::HeaderMap;
use serde_json::{Value, json};

use crate::domain::{Actor, Caller, WorkingCopy};

pub const ACTOR_HEADER: &str = "quadratique-actor";
pub const REPOSITORY_HEADER: &str = "quadratique-repository";
pub const WORKTREE_HEADER: &str = "quadratique-worktree";
pub const BRANCH_HEADER: &str = "quadratique-branch";

static ACTOR_SCHEMA: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema = json!({ "$ref": "#/$defs/Actor", "$defs": super::catalog::types() });
    jsonschema::validator_for(&schema).expect("the catalog's Actor is a JSON Schema")
});

pub fn caller_of(headers: &HeaderMap) -> Result<Caller, Refusal> {
    let actor = match header(headers, ACTOR_HEADER)? {
        None => Actor::without_header(),
        Some(record) => actor_of(&record)?,
    };
    Ok(Caller {
        actor,
        working_copy: WorkingCopy {
            repository: header(headers, REPOSITORY_HEADER)?,
            worktree: header(headers, WORKTREE_HEADER)?,
            branch: header(headers, BRANCH_HEADER)?,
            contains: None,
        },
    })
}

pub fn encode_header(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_graphic() && byte != b'%' {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub fn decode_header(encoded: &[u8]) -> Option<String> {
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut rest = encoded;
    while let Some((&byte, after)) = rest.split_first() {
        if byte == b'%' {
            let hex = std::str::from_utf8(after.get(..2)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            rest = &after[2..];
        } else {
            bytes.push(byte);
            rest = after;
        }
    }
    String::from_utf8(bytes).ok()
}

fn header(headers: &HeaderMap, name: &str) -> Result<Option<String>, Refusal> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    let text = decode_header(value.as_bytes()).ok_or_else(|| {
        Refusal::new(
            400,
            format!("The header {name} is not percent-encoded UTF-8."),
        )
    })?;
    Ok(Some(text).filter(|text| !text.is_empty()))
}

fn actor_of(record: &str) -> Result<Actor, Refusal> {
    let invalid = |problem: &str| {
        Refusal::new(
            400,
            format!("The header {ACTOR_HEADER} is not an actor record: {problem}."),
        )
    };
    let value: Value = serde_json::from_str(record).map_err(|_| invalid("not JSON"))?;
    if !ACTOR_SCHEMA.is_valid(&value) {
        return Err(invalid("it does not agree with the catalog's Actor"));
    }
    serde_json::from_value(value)
        .map(Actor::without_blank_ids)
        .map_err(|error| invalid(&error.to_string()))
}
