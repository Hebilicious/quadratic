use serde_json::{Value, json};

use crate::support::{Home, Lines, Process, Repository, git, outside, send_line};

struct Client {
    process: Process,
    lines: Lines,
    next_id: u64,
}

impl Client {
    fn start(mut process: Process) -> Client {
        let lines = Lines::of(process.stdout());
        let mut client = Client {
            process,
            lines,
            next_id: 0,
        };
        let initialized = client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" },
            }),
        );
        assert_eq!(initialized["result"]["serverInfo"]["name"], "quadratique");
        send_line(
            &mut client.process,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        );
        client
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        send_line(
            &mut self.process,
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        );
        loop {
            let message = self.lines.next_json();
            assert_eq!(message["jsonrpc"], "2.0", "not an MCP message: {message}");
            if message["id"] == id {
                return message;
            }
        }
    }

    fn refusal(&mut self, name: &str, arguments: Value) -> String {
        let answer = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        if answer.get("error").is_some() {
            return answer["error"].to_string();
        }
        let result = &answer["result"];
        assert_eq!(result["isError"], true, "{name} succeeded: {answer}");
        result["content"][0]["text"]
            .as_str()
            .expect("a text answer")
            .to_owned()
    }

    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let answer = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        let result = &answer["result"];
        assert_ne!(result["isError"], true, "{name} failed: {answer}");
        assert!(
            result.get("structuredContent").is_none(),
            "{name} answered structured content as well as its JSON text: {answer}"
        );
        let text = result["content"][0]["text"]
            .as_str()
            .expect("a text answer");
        serde_json::from_str(text).expect("the answer is JSON")
    }
}

#[test]
fn an_agent_over_mcp_reads_its_instructions_then_finds_claims_and_finishes_seeded_work() {
    let home = Home::new();
    let repository = Repository::new();
    let worktree = repository.worktree("tooling");
    home.quadratique(&repository.root)
        .args(["project", "create", "--key", "MCP", "--name", "Tools"])
        .run()
        .ok();
    home.quadratique(&repository.root)
        .args([
            "create",
            "--title",
            "Unrelated",
            "--acceptance",
            "It is done.",
            "--priority",
            "4",
        ])
        .run()
        .ok();
    let seeded = home
        .quadratique(&repository.root)
        .args([
            "create",
            "--title",
            "Through MCP",
            "--acceptance",
            "It is done.",
            "--priority",
            "1",
        ])
        .run()
        .ok()["issue"]["id"]
        .as_str()
        .expect("an issue id")
        .to_owned();

    let mut agent = Client::start(
        home.quadratique(&worktree)
            .args(["mcp"])
            .spawn_interactive(),
    );

    let listed = agent.request("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().expect("the tools");
    assert!(!tools.is_empty(), "{listed}");
    assert!(
        tools.iter().all(|tool| !tool["name"]
            .as_str()
            .unwrap_or_default()
            .starts_with("token.")),
        "an agent's MCP tools include a token capability: {listed}"
    );
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap_or_default())
        .collect();
    assert!(
        names.contains(&"issue.finish") && !names.contains(&"issue.close"),
        "an agent's MCP tools offer the old name, or lack the new one: {names:?}"
    );
    for tool in tools {
        assert!(
            tool.get("outputSchema").is_none(),
            "{} lists an output schema: {tool}",
            tool["name"]
        );
    }

    let instructions = agent.tool("workspace.instructions", json!({}));
    assert_eq!(instructions["project"], "MCP");
    let text = instructions["text"].as_str().expect("a text");
    for command in ["ready", "show", "claim", "finish", "release"] {
        assert!(
            text.contains(&format!("`quadratique {command}")),
            "the instructions do not name `quadratique {command}`: {text}"
        );
    }

    let ready = agent.tool("issue.ready", json!({ "limit": 1 }));
    assert_eq!(ready["items"][0]["id"], seeded, "{ready}");

    let claimed = agent.tool("issue.claim", json!({ "issue": seeded }));
    assert_eq!(
        claimed["issue"]["claim"]["holder"]["kind"], "agent",
        "{claimed}"
    );
    assert_eq!(claimed["issue"]["claim"]["holder"]["name"], "agent@tooling");

    agent.tool(
        "issue.finish",
        json!({ "issue": seeded, "evidence": "cargo test: 3 passed" }),
    );

    let shown = agent.tool("issue.get", json!({ "issue": seeded }));
    assert_eq!(shown["issue"]["evidence"], "cargo test: 3 passed");
    let history = agent.tool("history.query", json!({ "issue": seeded }));
    assert!(
        history["items"]
            .as_array()
            .expect("the changes")
            .iter()
            .any(|change| change["operation"] == "issue.claim"
                && change["author"]["name"] == "agent@tooling"),
        "{history}"
    );

    agent.process.close_stdin();
    let ended = agent.process.finish();
    assert_eq!(ended.code, 0, "{ended:?}");
}

fn text(path: &std::path::Path) -> String {
    path.display().to_string()
}

fn seeded(home: &Home, repository: &Repository, titles: &[&str]) -> Vec<String> {
    home.quadratique(&repository.root)
        .args(["project", "create", "--key", "DEM", "--name", "Demo"])
        .run()
        .ok();
    titles
        .iter()
        .map(|title| {
            home.quadratique(&repository.root)
                .args(["create", "--title", title, "--acceptance", "It is done."])
                .run()
                .ok()["issue"]["id"]
                .as_str()
                .expect("an issue id")
                .to_owned()
        })
        .collect()
}

#[test]
fn a_session_started_in_the_main_checkout_claims_for_a_worktree_it_names_and_then_works_from_it() {
    let home = Home::new();
    let repository = Repository::new();
    let ids = seeded(&home, &repository, &["Sandboxed"]);
    let [issue] = ids.as_slice() else {
        unreachable!()
    };
    let worktree = repository.worktree("dem-12");
    let mut agent = Client::start(
        home.quadratique(&repository.root)
            .args(["mcp"])
            .spawn_interactive(),
    );

    let claimed = agent.tool(
        "issue.claim",
        json!({ "issue": issue, "worktree": format!("{}/", text(&worktree)) }),
    );
    let claim = &claimed["issue"]["claim"];
    assert_eq!(claim["worktree"], text(&worktree), "{claimed}");
    assert_eq!(claim["branch"], "dem-12", "{claimed}");
    assert_eq!(claim["holder"]["name"], "agent@dem-12", "{claimed}");

    let instructions = agent.tool("workspace.instructions", json!({}));
    assert_eq!(instructions["worktree"], text(&worktree), "{instructions}");
    assert_eq!(instructions["branch"], "dem-12", "{instructions}");

    let revision = claimed["issue"]["revision"].clone();
    agent.tool(
        "issue.update",
        json!({ "issue": issue, "revision": revision, "notes": "from the worktree" }),
    );
    agent.tool(
        "issue.release",
        json!({ "issue": issue, "reason": "a break" }),
    );
    let again = agent.tool("issue.claim", json!({ "issue": issue }));
    assert_eq!(
        again["issue"]["claim"]["worktree"],
        text(&worktree),
        "{again}"
    );
    let closed = agent.tool(
        "issue.finish",
        json!({ "issue": issue, "evidence": "cargo test: 3 passed" }),
    );
    assert_eq!(
        closed["issue"]["evidence"], "cargo test: 3 passed",
        "{closed}"
    );
    let note = &closed["issue"]["notes"][0];
    assert_eq!(note["text"], "from the worktree", "{closed}");
    assert_eq!(note["author"], "agent@dem-12", "{closed}");
    assert_eq!(
        closed["issue"]["claim"]["holder"]["name"], "agent@dem-12",
        "{closed}"
    );

    agent.process.close_stdin();
    assert_eq!(agent.process.finish().code, 0);
}

#[test]
fn a_claim_that_fails_leaves_the_session_where_it_started_and_records_nothing() {
    let home = Home::new();
    let repository = Repository::new();
    let ids = seeded(&home, &repository, &["Free", "Held"]);
    let [free, held] = ids.as_slice() else {
        unreachable!()
    };
    let worktree = repository.worktree("dem-12");
    let theirs = repository.worktree("theirs");
    home.quadratique(&theirs)
        .args(["claim", "--issue", held])
        .run()
        .ok();
    let elsewhere = outside();
    git(
        elsewhere.path(),
        &["init", "--quiet", "--initial-branch=main"],
    );
    git(
        elsewhere.path(),
        &["commit", "--quiet", "--allow-empty", "--message", "init"],
    );
    let mut agent = Client::start(
        home.quadratique(&repository.root)
            .args(["mcp"])
            .spawn_interactive(),
    );
    let started = agent.tool("workspace.instructions", json!({}));
    assert_eq!(started["worktree"], text(&repository.root), "{started}");
    let free_before = agent.tool("issue.get", json!({ "issue": free }));

    let attempts = [
        (held.as_str(), text(&worktree), "Held"),
        (
            free.as_str(),
            "/no/such/worktree".to_owned(),
            "NoWorkingCopy",
        ),
        (
            free.as_str(),
            text(&elsewhere.path().canonicalize().expect("a real path")),
            "NoWorkingCopy",
        ),
        (free.as_str(), "worktrees/dem-12".to_owned(), "worktree"),
    ];
    for (issue, named, expected) in attempts {
        let refused = agent.refusal("issue.claim", json!({ "issue": issue, "worktree": named }));
        assert!(refused.contains(expected), "{named}: {refused}");
        let now = agent.tool("workspace.instructions", json!({}));
        assert_eq!(now["worktree"], started["worktree"], "{named}: {now}");
        assert_eq!(now["actor"], started["actor"], "{named}: {now}");
    }
    assert_eq!(
        agent.tool("issue.get", json!({ "issue": free })),
        free_before
    );

    agent.process.close_stdin();
    assert_eq!(agent.process.finish().code, 0);
}

#[test]
fn repeating_a_claim_whose_reply_was_lost_moves_the_session_and_changes_nothing_on_the_server() {
    let home = Home::new();
    let repository = Repository::new();
    let ids = seeded(&home, &repository, &["Lost reply"]);
    let [issue] = ids.as_slice() else {
        unreachable!()
    };
    let worktree = repository.worktree("dem-12");
    let named = json!({ "issue": issue, "worktree": text(&worktree) });
    let mut recorded = Client::start(
        home.quadratique(&repository.root)
            .args(["mcp"])
            .spawn_interactive(),
    );
    let claimed = recorded.tool("issue.claim", named.clone());
    recorded.process.close_stdin();
    assert_eq!(recorded.process.finish().code, 0);

    let mut agent = Client::start(
        home.quadratique(&repository.root)
            .args(["mcp"])
            .spawn_interactive(),
    );
    let before = agent.tool("workspace.instructions", json!({}));
    assert_eq!(before["worktree"], text(&repository.root), "{before}");
    let shown = agent.tool("issue.get", json!({ "issue": issue }));
    assert_eq!(
        shown["issue"]["claim"]["worktree"],
        text(&worktree),
        "{shown}"
    );

    let repeated = agent.tool("issue.claim", named);
    assert_eq!(
        repeated["issue"]["revision"], claimed["issue"]["revision"],
        "{repeated}"
    );
    let after = agent.tool("workspace.instructions", json!({}));
    assert_eq!(after["worktree"], text(&worktree), "{after}");

    agent.process.close_stdin();
    assert_eq!(agent.process.finish().code, 0);
}
