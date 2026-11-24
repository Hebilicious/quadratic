use std::path::{Path, PathBuf};
use std::time::Duration;

use acid_cat_core::Outcome;
use acid_cat_core::http::HttpClient;
use quadratic_server::git::watcher;
use quadratic_server::server::{self, Paths, Server, Waits};
use quadratic_server::web::WebApp;
use serde_json::{Value, json};
use tempfile::TempDir;

pub const HANG_GUARD: Duration = Duration::from_secs(60);

pub async fn within<F: Future>(what: &str, future: F) -> F::Output {
    tokio::time::timeout(HANG_GUARD, future)
        .await
        .unwrap_or_else(|_| panic!("{what} did not end within the hang guard of {HANG_GUARD:?}"))
}

pub struct Running {
    pub server: Server,
    code: PathBuf,
    home: TempDir,
}

pub async fn start() -> Running {
    start_looking_every(watcher::EVERY).await
}

pub async fn start_looking_every(every: Duration) -> Running {
    let home = tempfile::tempdir().expect("a temporary home");
    let server = within(
        "the server's start",
        server::start_with(
            &Paths::in_home(home.path()).expect("an absolute home"),
            server::loopback(),
            WebApp::from_env(),
            every,
            Waits::default(),
        ),
    )
    .await
    .expect("the server starts");
    let code = home.path().join("code");
    Running::of(server, code, home)
}

#[derive(Clone, Debug)]
pub struct Who {
    pub actor: Option<Value>,
    pub repository: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
}

impl Who {
    pub fn name(&self) -> String {
        self.actor
            .as_ref()
            .and_then(|actor| actor["name"].as_str())
            .unwrap_or("agent")
            .to_owned()
    }

    pub fn worktree(&self) -> String {
        self.worktree.clone().unwrap_or_default()
    }
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

impl Running {
    pub fn of(server: Server, code: PathBuf, home: TempDir) -> Running {
        Running { server, code, home }
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    pub fn into_parts(self) -> (Server, PathBuf, TempDir) {
        (self.server, self.code, self.home)
    }

    pub fn repository(&self) -> String {
        text(&self.code.join("demo/.git"))
    }

    pub fn other_repository(&self) -> String {
        text(&self.code.join("site/.git"))
    }

    pub fn human(&self) -> Who {
        Who {
            actor: Some(json!({ "kind": "human", "name": "sam" })),
            repository: Some(self.repository()),
            worktree: Some(text(&self.code.join("demo"))),
            branch: Some("main".into()),
        }
    }

    pub fn agent(&self, worktree: &str) -> Who {
        let name = format!("claude-code(t3)@{worktree}");
        Who {
            actor: Some(
                json!({ "kind": "agent", "name": name, "harness": "claude-code", "host": "t3" }),
            ),
            repository: Some(self.repository()),
            worktree: Some(text(&self.code.join("demo/.worktrees").join(worktree))),
            branch: Some(worktree.into()),
        }
    }

    pub fn client(&self, who: &Who) -> HttpClient {
        let mut client = HttpClient::new(self.server.url()).expect("the server's URL");
        let headers = [
            ("Quadratic-Actor", who.actor.as_ref().map(Value::to_string)),
            ("Quadratic-Repository", who.repository.clone()),
            ("Quadratic-Worktree", who.worktree.clone()),
            ("Quadratic-Branch", who.branch.clone()),
        ];
        for (name, value) in headers {
            if let Some(value) = value {
                client = client.header(name, &value).expect("a valid header");
            }
        }
        client
    }

    pub async fn call(&self, who: &Who, id: &str, input: Value) -> Result<Value, Value> {
        call(&self.client(who), id, input).await
    }

    pub async fn ok(&self, who: &Who, id: &str, input: Value) -> Value {
        match self.call(who, id, input.clone()).await {
            Ok(value) => value,
            Err(failure) => panic!("{id} {input} failed: {failure}"),
        }
    }

    pub async fn failure(&self, who: &Who, id: &str, input: Value) -> Value {
        match self.call(who, id, input.clone()).await {
            Ok(value) => panic!("{id} {input} succeeded: {value}"),
            Err(failure) => failure,
        }
    }

    pub async fn project(&self, key: &str) -> Value {
        self.ok(
            &self.human(),
            "project.create",
            json!({ "key": key, "name": format!("Project {key}") }),
        )
        .await
    }

    pub async fn issue(&self, title: &str) -> String {
        self.issue_with(json!({ "acceptance": "It is done.", "title": title }))
            .await
    }

    pub async fn issue_with(&self, input: Value) -> String {
        let created = self.ok(&self.human(), "issue.create", input).await;
        created["issue"]["id"]
            .as_str()
            .expect("an issue id")
            .to_owned()
    }

    pub async fn get(&self, id: &str) -> Value {
        self.ok(&self.human(), "issue.get", json!({ "issue": id }))
            .await["issue"]
            .clone()
    }

    pub async fn ready_ids(&self, who: &Who, limit: usize) -> Vec<String> {
        let ready = self.ok(who, "issue.ready", json!({ "limit": limit })).await;
        ids(&ready["items"])
    }
}

pub async fn call(client: &HttpClient, id: &str, input: Value) -> Result<Value, Value> {
    match within(id, client.invoke(id, &input)).await {
        Outcome::Value(value) => Ok(value),
        Outcome::Failure(failure) => Err(failure),
        Outcome::CallFailure(failure) => {
            panic!("{id} {input} ended with a call failure: {failure}")
        }
    }
}

pub fn ids(items: &Value) -> Vec<String> {
    items
        .as_array()
        .expect("a list")
        .iter()
        .map(|item| item["id"].as_str().expect("an id").to_owned())
        .collect()
}

pub fn reasons(not_ready: &Value) -> std::collections::BTreeSet<&str> {
    not_ready
        .as_array()
        .unwrap_or_else(|| panic!("a list of reasons, not {not_ready}"))
        .iter()
        .map(|reason| reason.as_str().expect("a reason"))
        .collect()
}
