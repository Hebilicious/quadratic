use std::path::PathBuf;

use acid_cat_core::{CapabilityDefinition, NoInput};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::catalog::{capability, capability_without_input};
use super::{Context, Refused, finish};
use crate::credentials::{CreateError, Credentials, Token};

#[derive(Deserialize)]
pub(super) struct CreateInput {
    label: String,
    file: String,
}

#[derive(Serialize)]
pub(super) struct Created {
    label: String,
    file: String,
}

pub(super) fn create() -> CapabilityDefinition<Context, CreateInput, Created, Value> {
    capability::<CreateInput, Created>("token.create").run(|input, call| async move {
        finish(create_token(call.context().credentials(), input).await)
    })
}

async fn create_token(
    credentials: Result<&Credentials, Refused>,
    input: CreateInput,
) -> Result<Created, Refused> {
    let credentials = credentials?.clone();
    let CreateInput { label, file } = input;
    let created = {
        let (label, file) = (label.clone(), PathBuf::from(&file));
        blocking(move || credentials.create(&label, &file)).await?
    };
    match created {
        Ok(_) => Ok(Created { label, file }),
        Err(CreateError::NotAbsolute) => Err(failure("NotAbsolute", json!({ "file": file }))),
        Err(CreateError::InDataFolder) => Err(failure("InDataFolder", json!({ "file": file }))),
        Err(CreateError::FileExists) => Err(failure("FileExists", json!({ "file": file }))),
        Err(CreateError::LabelTaken) => Err(failure("LabelTaken", json!({ "label": label }))),
        Err(CreateError::Io(error)) => Err(Refused::defect(error)),
    }
}

#[derive(Serialize)]
pub(super) struct TokenList {
    items: Vec<Token>,
}

pub(super) fn list() -> CapabilityDefinition<Context, NoInput, TokenList, Value> {
    capability_without_input::<TokenList>("token.list").run(|(), call| async move {
        finish(call.context().credentials().map(|credentials| TokenList {
            items: credentials.list(),
        }))
    })
}

#[derive(Deserialize)]
pub(super) struct RevokeInput {
    label: String,
}

#[derive(Serialize)]
pub(super) struct Revoked {
    label: String,
    revoked: bool,
}

pub(super) fn revoke() -> CapabilityDefinition<Context, RevokeInput, Revoked, Value> {
    capability::<RevokeInput, Revoked>("token.revoke").run(|input, call| async move {
        finish(revoke_token(call.context().credentials(), input.label).await)
    })
}

async fn revoke_token(
    credentials: Result<&Credentials, Refused>,
    label: String,
) -> Result<Revoked, Refused> {
    let credentials = credentials?.clone();
    let revoked = {
        let label = label.clone();
        blocking(move || credentials.revoke(&label)).await?
    }
    .map_err(Refused::defect)?;
    Ok(Revoked { label, revoked })
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Refused> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(Refused::defect)
}

fn failure(tag: &str, mut fields: Value) -> Refused {
    fields["_tag"] = json!(tag);
    Refused::Failure(fields)
}
