use std::sync::LazyLock;

use acid_cat_core::{App, Capability, Event, Target};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::Context;
use crate::domain::Workflow;

pub const CATALOG_JSON: &str = include_str!("quadratique.acidcat.json");

/// The interface of this build: the 64-bit FNV-1a hash of the catalog JSON it serves. Two builds with the same catalog
/// have the same interface. FNV-1a is written out here because its value must never change with the Rust version.
pub fn interface() -> String {
    let hash = CATALOG_JSON
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("fnv1a64:{hash:016x}")
}

static CATALOG: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(CATALOG_JSON).expect("the catalog of Quadratique is JSON")
});

fn member<'a>(value: &'a Value, path: &str) -> &'a Value {
    value
        .pointer(path)
        .unwrap_or_else(|| panic!("the catalog has no {path}"))
}

fn text(value: &Value, path: &str) -> String {
    member(value, path)
        .as_str()
        .unwrap_or_else(|| panic!("{path} of the catalog is not a string"))
        .to_owned()
}

fn names(value: &Value, path: &str) -> Vec<String> {
    member(value, path)
        .as_array()
        .unwrap_or_else(|| panic!("{path} of the catalog is not a list"))
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn entries(value: &Value, path: &str) -> Vec<(String, Value)> {
    value
        .pointer(path)
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

pub fn app() -> App<Context> {
    let mut app = App::new()
        .title(text(&CATALOG, "/title"))
        .description(text(&CATALOG, "/description"));
    for (name, target) in entries(&CATALOG, "/targets") {
        app = app.target_with(
            name,
            Target {
                bindings: names(&target, "/bindings"),
                description: target
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                options: target.get("options").cloned(),
            },
        );
    }
    for (name, schema) in entries(&CATALOG, "/$defs") {
        app = app.r#type(name, schema);
    }
    app
}

pub fn default_workflow() -> Result<Workflow, String> {
    let workflow: Workflow = serde_json::from_value(
        member(&CATALOG, "/$defs/Workflow/default").clone(),
    )
    .map_err(|error| format!("the catalog's default workflow is not a workflow: {error}"))?;
    workflow
        .validate()
        .map_err(|failure| format!("the catalog's default workflow is invalid: {failure:?}"))?;
    Ok(workflow)
}

pub(crate) fn max_items(path: &str) -> usize {
    let limit = member(&CATALOG, &format!("/$defs/{path}/maxItems"));
    limit
        .as_u64()
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or_else(|| panic!("{path} of the catalog has no maxItems that fits a usize"))
}

pub fn input_refusal(capability: &str, input: &Value) -> Option<String> {
    let mut schema = member(&CATALOG, &format!("/capabilities/{capability}/input")).clone();
    schema["$defs"] = types();
    let validator =
        jsonschema::validator_for(&schema).expect("a capability's input is a JSON Schema");
    let issues: Vec<String> = validator
        .iter_errors(input)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect();
    (!issues.is_empty()).then(|| issues.join("; "))
}

pub fn types() -> Value {
    member(&CATALOG, "/$defs").clone()
}

type Declared<I, O> = Capability<Context, I, O, Value, I>;

fn declared(id: &str) -> (Capability<Context>, Value) {
    let spec = member(&CATALOG, &format!("/capabilities/{id}")).clone();
    let mut capability =
        Capability::new(id, text(&spec, "/description")).audience(names(&spec, "/audience"));
    if spec.get("readOnly") == Some(&Value::Bool(true)) {
        capability = capability.read_only();
    }
    (capability, spec)
}

pub fn capability<I: DeserializeOwned, O: Serialize>(id: &str) -> Declared<I, O> {
    let (capability, spec) = declared(id);
    capability
        .input_schema::<I>(member(&spec, "/input").clone())
        .output_schema::<O>(member(&spec, "/output").clone())
        .failure_schemas::<Value, _>(entries(&spec, "/failures"))
}

pub fn capability_without_input<O: Serialize>(id: &str) -> Capability<Context, (), O, Value> {
    let (capability, spec) = declared(id);
    capability
        .output_schema::<O>(member(&spec, "/output").clone())
        .failure_schemas::<Value, _>(entries(&spec, "/failures"))
}

pub fn event<I: DeserializeOwned, P: Serialize>(id: &str) -> Event<Context, I, P, I> {
    let spec = member(&CATALOG, &format!("/events/{id}")).clone();
    Event::new(id, text(&spec, "/description"))
        .audience(names(&spec, "/audience"))
        .input_schema::<I>(member(&spec, "/input").clone())
        .payload_schema::<P>(member(&spec, "/payload").clone())
}
