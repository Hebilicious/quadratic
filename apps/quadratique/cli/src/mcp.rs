use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex};

use acid_cat_core::mcp::{McpStdioOptions, serve_stdio};
use acid_cat_core::{
    App, Application, Call, Capability, CapabilityDefinition, Defect, Outcome, RunError, RunResult,
    Target,
};
use quadratique_server::app::CATALOG_JSON;
use quadratique_server::domain::{Binding, Caller};
use serde_json::{Value, json};

use crate::background::{self, Home};
use crate::caller;

const TARGET: &str = "mcp";

pub struct Upstream {
    home: Home,
    session: Session,
}

/// Where the session acts: the folder it started in, until a claim that names a worktree is taken; then that
/// worktree, until another such claim is taken. Such claims take turns from sending to settling, so they settle in the
/// order they were sent; other calls act as the current caller without waiting for a turn.
struct Session {
    caller: Mutex<Caller>,
    turn: tokio::sync::Mutex<()>,
}

enum Claim {
    Taken,
    Refused,
}

impl Session {
    fn new(caller: Caller) -> Session {
        Session {
            caller: Mutex::new(caller),
            turn: tokio::sync::Mutex::new(()),
        }
    }

    fn caller(&self) -> Caller {
        self.caller
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The caller of a call. For the calls where the branch decides whether an issue can be
    /// started, its branch is the one its worktree has now: the worktree may have switched
    /// branch since the session started.
    fn caller_for(&self, id: &str) -> Caller {
        let mut caller = self.caller();
        if matches!(id, "issue.ready" | "issue.claim")
            && let Some(worktree) = &caller.working_copy.worktree
        {
            caller.working_copy.branch = caller::branch_in(std::path::Path::new(worktree));
        }
        caller
    }

    async fn claim_from<Sent>(
        &self,
        from: Caller,
        send: impl FnOnce(Caller) -> Sent,
    ) -> Sent::Output
    where
        Sent: Future<Output = RunResult<Value, Value>>,
    {
        let _turn = self.turn.lock().await;
        let answer = send(from.clone()).await;
        let claim = if answer.is_ok() {
            Claim::Taken
        } else {
            Claim::Refused
        };
        self.settle(from, claim);
        answer
    }

    fn settle(&self, from: Caller, claim: Claim) {
        match claim {
            Claim::Taken => {
                *self
                    .caller
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = from;
            }
            Claim::Refused => {}
        }
    }
}

impl Upstream {
    pub fn new(home: Home, caller: Caller) -> Upstream {
        Upstream {
            home,
            session: Session::new(caller),
        }
    }

    async fn invoke(&self, id: &str, input: &Value) -> RunResult<Value, Value> {
        match (id, input.get("worktree").and_then(Value::as_str)) {
            ("issue.claim", Some(named)) => {
                let (from, worktree) = caller::claim_from(Binding::Mcp, named)
                    .map_err(|error| Defect::new(error.to_string()))?;
                let mut input = input.clone();
                input["worktree"] = json!(worktree);
                self.session
                    .claim_from(
                        from,
                        |from| async move { self.send(&from, id, &input).await },
                    )
                    .await
            }
            _ => self.send(&self.session.caller_for(id), id, input).await,
        }
    }

    async fn send(&self, caller: &Caller, id: &str, input: &Value) -> RunResult<Value, Value> {
        let endpoint = background::ensure(&self.home)
            .await
            .map_err(|error| Defect::new(error.to_string()))?;
        let client = caller::client(caller, &endpoint).map_err(Defect::new)?;
        match client.invoke(id, input).await {
            Outcome::Value(value) => Ok(value),
            Outcome::Failure(failure) => Err(RunError::Failure(failure)),
            Outcome::CallFailure(failure) => Err(RunError::Defect(Defect::new(format!(
                "the server answered {}: {}",
                failure.kind.as_str(),
                failure.message
            )))),
        }
    }
}

pub async fn serve(upstream: Upstream) -> Result<(), Vec<String>> {
    let app = application()?;
    let options = McpStdioOptions::new("quadratique", env!("CARGO_PKG_VERSION"));
    serve_stdio(app, Arc::new(upstream), options).await;
    Ok(())
}

fn application() -> Result<Arc<Application<Upstream>>, Vec<String>> {
    let catalog: Value =
        serde_json::from_str(CATALOG_JSON).map_err(|error| vec![error.to_string()])?;
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let target = &catalog["targets"][TARGET];
    let mut app = App::new().target_with(
        TARGET,
        Target {
            surfaces: vec!["mcp".into()],
            description: text(target, "description"),
            options: target.get("options").cloned(),
        },
    );
    if let Some(title) = text(&catalog, "title") {
        app = app.title(title);
    }
    if let Some(description) = text(&catalog, "description") {
        app = app.description(description);
    }
    for (name, schema) in members(&catalog, "$defs") {
        app = app.r#type(name, schema);
    }
    for (id, spec) in members(&catalog, "capabilities") {
        let audience = spec["audience"].as_array().cloned().unwrap_or_default();
        if audience.contains(&json!(TARGET)) {
            app = app.capability(forwarded(&id, &spec));
        }
    }
    app.on_defect(report_once()).build()
}

/// AcidCat shows a client only that the capability failed, and by default writes every defect to standard error; the
/// harness's log gets each reason once.
fn report_once() -> impl Fn(&str, &Defect) + Send + Sync + 'static {
    let reported = Mutex::new(HashSet::new());
    move |_capability, defect| {
        let reason = defect.to_string();
        let first = reported
            .lock()
            .map(|mut reported| reported.insert(reason.clone()))
            .unwrap_or(true);
        if first {
            eprintln!("quadratique mcp: {reason}");
        }
    }
}

fn members(value: &Value, key: &str) -> Vec<(String, Value)> {
    value
        .get(key)
        .and_then(Value::as_object)
        .map(|object| object.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// MCP lists an output schema only for an object schema, so the tool list carries no output schemas,
/// and each answer comes as its JSON text.
fn any_value() -> Value {
    json!({})
}

fn forwarded(id: &str, spec: &Value) -> CapabilityDefinition<Upstream> {
    let description = spec["description"].as_str().unwrap_or_default();
    let mut capability = Capability::new(id, description).audience([TARGET]);
    if spec.get("readOnly") == Some(&Value::Bool(true)) {
        capability = capability.read_only();
    }
    let failures = members(spec, "failures");
    let forward = |id: String| {
        move |input: Value, call: Call<Upstream>| {
            let id = id.clone();
            async move { call.context().invoke(&id, &input).await }
        }
    };
    match spec.get("input") {
        Some(input) => capability
            .input_schema::<Value>(input.clone())
            .output_schema::<Value>(any_value())
            .failure_schemas::<Value, _>(failures)
            .run(forward(id.to_owned()))
            .untyped(),
        None => {
            let forward = forward(id.to_owned());
            capability
                .output_schema::<Value>(any_value())
                .failure_schemas::<Value, _>(failures)
                .run(move |(), call| forward(json!({}), call))
                .untyped()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use quadratique_server::domain::{Actor, ActorKind, Caller, WorkingCopy};
    use serde_json::json;
    use tokio::sync::oneshot;

    use super::Session;

    fn caller_in(worktree: &str) -> Caller {
        Caller {
            actor: Actor {
                kind: ActorKind::Agent,
                name: format!("agent@{worktree}"),
                role: None,
                harness: None,
                host: None,
                host_thread: None,
                parent: None,
                model: None,
                session: None,
            },
            working_copy: WorkingCopy {
                repository: Some("/demo/.git".into()),
                worktree: Some(worktree.into()),
                branch: Some(worktree.into()),
                contains: None,
            },
        }
    }

    fn git(directory: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .current_dir(directory)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(std::process::Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn ready_and_claim_act_from_the_branch_the_worktree_has_at_the_call() {
        let folder = tempfile::tempdir().expect("a temporary folder");
        let worktree = folder.path();
        git(worktree, &["init", "--quiet", "--initial-branch=parse"]);
        git(
            worktree,
            &["commit", "--quiet", "--allow-empty", "--message", "init"],
        );
        let session = Session::new(caller_in(&worktree.display().to_string()));

        git(worktree, &["switch", "--quiet", "--create", "docs"]);
        let branch = |id: &str| session.caller_for(id).working_copy.branch;
        assert_eq!(branch("issue.ready").as_deref(), Some("docs"));
        assert_eq!(branch("issue.claim").as_deref(), Some("docs"));
        git(worktree, &["switch", "--quiet", "--detach"]);
        assert_eq!(branch("issue.ready"), None);
        assert_eq!(
            branch("issue.list").as_deref(),
            Some(worktree.display().to_string().as_str()),
            "other calls keep the branch read at the start"
        );
    }

    async fn let_every_ready_task_run() {
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_claim_sent_while_an_earlier_one_awaits_its_reply_settles_after_it_and_wins() {
        let session = Arc::new(Session::new(caller_in("/main")));
        let (reply_to_a, a_replied) = oneshot::channel::<()>();
        let (a_sent, a_was_sent) = oneshot::channel::<()>();
        let first = tokio::spawn({
            let session = Arc::clone(&session);
            async move {
                session
                    .claim_from(caller_in("/one"), |_| async move {
                        let _ = a_sent.send(());
                        let _ = a_replied.await;
                        Ok(json!({}))
                    })
                    .await
            }
        });
        a_was_sent.await.expect("claim A is sent");
        let second = tokio::spawn({
            let session = Arc::clone(&session);
            async move {
                session
                    .claim_from(caller_in("/two"), |_| async { Ok(json!({})) })
                    .await
            }
        });
        let_every_ready_task_run().await;
        assert_eq!(
            session.caller(),
            caller_in("/main"),
            "a call during claim A acts from where the session was"
        );

        reply_to_a
            .send(())
            .expect("claim A still waits for its reply");
        first
            .await
            .expect("claim A ends")
            .expect("claim A is taken");
        second
            .await
            .expect("claim B ends")
            .expect("claim B is taken");

        assert_eq!(session.caller(), caller_in("/two"));
    }

    #[test]
    fn the_tools_are_the_catalog_of_the_mcp_target_that_the_server_serves_but_for_their_outputs() {
        let forwarded = super::application().expect("the forwarding application builds");
        let served = quadratique_server::app::application().expect("the server's application");
        let (forwarded, served) = (
            forwarded.target_catalog(super::TARGET),
            served.target_catalog(super::TARGET),
        );
        let without_outputs = |view: &serde_json::Value| {
            let mut capabilities = view["capabilities"].clone();
            for capability in capabilities
                .as_object_mut()
                .expect("capabilities")
                .values_mut()
            {
                capability
                    .as_object_mut()
                    .expect("a capability")
                    .remove("output");
            }
            capabilities
        };
        assert!(!without_outputs(&served).as_object().unwrap().is_empty());
        assert_eq!(without_outputs(&forwarded), without_outputs(&served));
        assert_eq!(forwarded["targets"], served["targets"]);
        let types = forwarded["$defs"].as_object().expect("the view has types");
        for (name, schema) in types {
            assert_eq!(schema, &served["$defs"][name], "{name}");
        }
    }
}
