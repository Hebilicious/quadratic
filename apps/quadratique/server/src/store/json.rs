use super::error::StoreError;
use serde_json::Value;

pub(crate) fn text(value: &Value) -> String {
    sorted(value.clone()).to_string()
}

fn sorted(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<(String, Value)> = object.into_iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sorted(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

pub(crate) fn canonical(what: &'static str, text: &str) -> Result<String, StoreError> {
    serde_json::from_str::<Value>(text)
        .map(|value| self::text(&value))
        .map_err(|error| StoreError::InvalidJson {
            what,
            message: error.to_string(),
        })
}

pub(crate) fn canonical_opt(
    what: &'static str,
    text: Option<&str>,
) -> Result<Option<String>, StoreError> {
    text.map(|text| canonical(what, text)).transpose()
}

pub(crate) fn member(text: &str, key: &str) -> Option<String> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get(key)?
        .as_str()
        .map(str::to_owned)
}

pub(crate) fn string(text: &str) -> String {
    Value::from(text).to_string()
}
