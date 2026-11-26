use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};

use acid_cat_core::http::HttpClient;
use quadratique_server::app::{
    ACTOR_HEADER, BRANCH_HEADER, REPOSITORY_HEADER, WORKTREE_HEADER, encode_header,
};
use quadratique_server::domain::{Actor, ActorError, Binding, Caller, WorkingCopy};
use quadratique_server::git::environment;

use crate::background::Endpoint;

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

/// The one client of every command, `quadratique mcp`, `init` and `upgrade --skills`: it carries the caller in its
/// headers and reaches the server where `endpoint` says, on Unix only through the home's socket.
pub fn client(caller: &Caller, endpoint: &Endpoint) -> Result<HttpClient, String> {
    let actor = serde_json::to_string(&caller.actor).map_err(|error| error.to_string())?;
    let copy = &caller.working_copy;
    let headers = [
        (ACTOR_HEADER, Some(actor)),
        (REPOSITORY_HEADER, copy.repository.clone()),
        (WORKTREE_HEADER, copy.worktree.clone()),
        (BRANCH_HEADER, copy.branch.clone()),
    ];
    let client = HttpClient::new(&endpoint.base)?;
    let client = match &endpoint.socket {
        Some(socket) => client.unix_socket(socket),
        None => client,
    };
    headers
        .into_iter()
        .filter_map(|(name, value)| Some((name, value?)))
        .try_fold(client, |client, (name, value)| {
            client.header(name, &encode_header(&value))
        })
}

fn working_copy(directory: &Path) -> WorkingCopy {
    WorkingCopy {
        repository: git_in(
            directory,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        ),
        worktree: git_in(
            directory,
            &["rev-parse", "--path-format=absolute", "--show-toplevel"],
        ),
        branch: branch_in(directory),
        contains: None,
    }
}

/// The branch checked out in `directory` now; `None` on a detached `HEAD` or outside git.
pub fn branch_in(directory: &Path) -> Option<String> {
    match git_in(directory, &["rev-parse", "--abbrev-ref", "HEAD"]).as_deref() {
        Some("HEAD") => None,
        Some(branch) => Some(branch.to_owned()),
        // `rev-parse --abbrev-ref HEAD` fails on a branch with no commit yet; `symbolic-ref` still names it.
        None => git_in(directory, &["symbolic-ref", "--short", "--quiet", "HEAD"]),
    }
}

fn git_in(directory: &Path, args: &[&str]) -> Option<String> {
    let output = environment::without_trace(&mut Command::new("git"))
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim_end_matches(['\n', '\r']);
    (output.status.success() && !text.is_empty()).then(|| text.to_owned())
}
