use std::path::{Path, PathBuf};
use std::time::Duration;

use acid_cat_core::Outcome;
use acid_cat_core::http::HttpClient;
use quadratique_server::git::{environment, watcher};
use quadratique_server::server::{self, Paths, Server, Waits};
use quadratique_server::web::WebApp;
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

/// A home as the home policy wants it: mode 0700. `tempfile` makes its folders with the mode the umask leaves,
/// usually 0755, and the policy refuses such a home rather than change its mode.
pub fn private_home() -> TempDir {
    use std::os::unix::fs::PermissionsExt;

    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("a temporary home")
}

/// Takes `listener` out of the listening state for every descriptor of it, then closes it: later connections are
/// refused. A plain drop closes one descriptor, while a child that another test spawns holds a copy of each descriptor
/// of this process between its `fork` and its `exec`, and so keeps the socket listening for that moment.
pub fn stop_listening(listener: std::os::unix::net::UnixListener) {
    #[cfg(target_os = "linux")]
    rustix::net::shutdown(&listener, rustix::net::Shutdown::Both)
        .expect("the socket stops listening");
    drop(listener);
}

/// Opens the web app's listener of `server`, as the first `quadratique ui` does, and gives its address.
pub async fn web_address(server: &Server) -> std::net::SocketAddr {
    within("the web app's listener", server.open_web())
        .await
        .expect("the web app listens")
}

pub async fn start() -> Running {
    start_looking_every(watcher::EVERY).await
}

pub async fn start_looking_every(every: Duration) -> Running {
    start_with(every, Waits::default()).await
}

/// A server that gives a client of its socket or its web app `limit` to send a whole request, for a test that shows what the limit cuts off
/// without waiting for the product's limit.
pub async fn start_with_request_limit(limit: Duration) -> Running {
    let waits = Waits {
        request: limit,
        ..Waits::default()
    };
    start_with(watcher::EVERY, waits).await
}

async fn start_with(every: Duration, waits: Waits) -> Running {
    environment::read_no_user_configuration();
    let home = private_home();
    let server = within(
        "the server's start",
        server::start_with(
            &Paths::in_home(home.path()).expect("an absolute home"),
            server::loopback(),
            WebApp::from_env(),
            every,
            waits,
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
        let mut client = HttpClient::new(&server::base_over_socket())
            .expect("the base URL over the socket")
            .unix_socket(self.server.socket_path());
        let headers = [
            (
                "Quadratique-Actor",
                who.actor.as_ref().map(Value::to_string),
            ),
            ("Quadratique-Repository", who.repository.clone()),
            ("Quadratique-Worktree", who.worktree.clone()),
            ("Quadratique-Branch", who.branch.clone()),
        ];
        for (name, value) in headers {
            if let Some(value) = value {
                client = client.header(name, &value).expect("a valid header");
            }
        }
        client
    }

    /// A client of the web app's target at `base` (`/web` or `/webmcp`), as the server's own page sends its calls once
    /// a link of `quadratique ui` let it in.
    pub async fn page(&self, base: &str) -> HttpClient {
        let (address, bearer) = crate::web::paired(self.home()).await;
        let origin = format!("http://{address}");
        HttpClient::new(&format!("{origin}{base}"))
            .expect("the web target's URL")
            .header("Origin", &origin)
            .expect("a valid header")
            .header("Authorization", &format!("Bearer {bearer}"))
            .expect("a valid header")
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
