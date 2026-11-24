use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};

use acid_cat_core::http::HttpClient;
use quadratic_server::app::{
    ACTOR_HEADER, BRANCH_HEADER, REPOSITORY_HEADER, WORKTREE_HEADER, encode_header,
};
use quadratic_server::domain::{Actor, ActorError, Binding, Caller, WorkingCopy};

pub fn detect(binding: Binding) -> Result<Caller, ActorError> {
    match std::env::current_dir() {
        Ok(directory) => detect_in(binding, &directory),
        Err(_) => caller(binding, WorkingCopy::default()),
    }
}

pub fn detect_in(binding: Binding, directory: &Path) -> Result<Caller, ActorError> {
    caller(binding, working_copy(directory))
}

fn caller(binding: Binding, working_copy: WorkingCopy) -> Result<Caller, ActorError> {
    let env: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let login = ["USER", "LOGNAME"]
        .iter()
        .find_map(|name| env.get(*name).filter(|value| !value.is_empty()))
        .cloned();
    let actor = Actor::detect(
        &env,
        binding,
        working_copy.worktree.as_deref(),
        login.as_deref(),
    )?;
    Ok(Caller {
        actor,
        working_copy,
    })
}

/// A claim that names a worktree acts from it: sent as the caller found there, naming the worktree as git
/// prints it, or as given when git finds none, which the server then refuses.
pub fn claim_from(binding: Binding, named: &str) -> Result<(Caller, String), ActorError> {
    let caller = detect_in(binding, Path::new(named))?;
    let worktree = caller
        .working_copy
        .worktree
        .clone()
        .unwrap_or_else(|| named.to_owned());
    Ok((caller, worktree))
}

pub fn client(caller: &Caller, url: &str) -> Result<HttpClient, String> {
    let actor = serde_json::to_string(&caller.actor).map_err(|error| error.to_string())?;
    let copy = &caller.working_copy;
    let headers = [
        (ACTOR_HEADER, Some(actor)),
        (REPOSITORY_HEADER, copy.repository.clone()),
        (WORKTREE_HEADER, copy.worktree.clone()),
        (BRANCH_HEADER, copy.branch.clone()),
    ];
    headers
        .into_iter()
        .filter_map(|(name, value)| Some((name, value?)))
        .try_fold(HttpClient::new(url)?, |client, (name, value)| {
            client.header(name, &encode_header(&value))
        })
}

fn working_copy(directory: &Path) -> WorkingCopy {
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(directory)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        let text = text.trim_end_matches(['\n', '\r']);
        (output.status.success() && !text.is_empty()).then(|| text.to_owned())
    };
    let branch = match git(&["rev-parse", "--abbrev-ref", "HEAD"]).as_deref() {
        Some("HEAD") => None,
        Some(branch) => Some(branch.to_owned()),
        // `rev-parse --abbrev-ref HEAD` fails on a branch with no commit yet; `symbolic-ref` still names it.
        None => git(&["symbolic-ref", "--short", "--quiet", "HEAD"]),
    };
    WorkingCopy {
        repository: git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]),
        worktree: git(&["rev-parse", "--path-format=absolute", "--show-toplevel"]),
        branch,
    }
}
