//! A command reaches the server only through the socket of its home (record 0004, §2): the server's TCP listener is
//! not needed by any command, `quadratique mcp`, `init` or `upgrade --skills`.

use serde_json::json;

use quadratique_server::server::{self, Paths, Server};

use crate::mcp::Client;
use crate::support::{Home, Lines, Repository, outside};

/// The home's server, run by this test, with its TCP listener out of the listening state: every TCP connection to it
/// is refused, and its socket serves.
struct ServerWithoutTcp {
    runtime: tokio::runtime::Runtime,
    server: Option<Server>,
}

impl ServerWithoutTcp {
    fn start(home: &Home) -> ServerWithoutTcp {
        let runtime = tokio::runtime::Runtime::new().expect("a runtime for the server");
        let paths = Paths::in_home(home.path()).expect("an absolute home");
        let server = runtime
            .block_on(server::start(&paths, server::loopback()))
            .expect("the server starts");
        server.stop_listening_on_tcp();
        let refused = std::net::TcpStream::connect(server.address());
        assert_eq!(
            refused.as_ref().map_err(std::io::Error::kind).err(),
            Some(std::io::ErrorKind::ConnectionRefused),
            "{refused:?}"
        );
        ServerWithoutTcp {
            runtime,
            server: Some(server),
        }
    }
}

impl Drop for ServerWithoutTcp {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            self.runtime.block_on(server.shutdown());
        }
    }
}

#[test]
fn a_command_watch_an_mcp_tool_call_init_and_upgrade_skills_work_while_tcp_refuses_every_connection()
 {
    let home = Home::new();
    let repository = Repository::new();
    let elsewhere = outside();
    let _server = ServerWithoutTcp::start(&home);

    let initialized = home
        .quadratique(&repository.root)
        .args(["init", "--key", "TAL", "--name", "tally"])
        .run()
        .ok();
    let history = home
        .quadratique(&repository.root)
        .args(["history", "query"])
        .run()
        .ok();
    let latest = history["items"][0]["commit"]
        .as_str()
        .expect("the commit of init's project");
    let mut watcher = home
        .quadratique(&repository.root)
        .args(["watch", "issue.changed", "--cursor", latest])
        .spawn();
    let watched = Lines::of(watcher.stdout());
    let created = home
        .quadratique(&repository.root)
        .args(["create", "--title", "Parse", "--acceptance", "It parses."])
        .run()
        .ok();
    let issue = created["issue"]["id"].as_str().expect("an issue id");
    let occurrence = watched.next_json();
    let mut mcp = Client::start(
        home.quadratique(&repository.root)
            .args(["mcp"])
            .spawn_interactive(),
    );
    let read = mcp.tool("issue.get", json!({ "issue": issue }));
    let skills = home
        .quadratique(elsewhere.path())
        .args(["upgrade", "--skills"])
        .run()
        .ok();

    assert_eq!(initialized["project"]["created"], true, "{initialized}");
    assert_eq!(
        occurrence["data"]["issues"][0]["issue"]["id"], issue,
        "{occurrence}"
    );
    assert_eq!(read["issue"]["title"], "Parse", "{read}");
    assert_eq!(
        skills["skills"]["current"],
        json!([repository.root]),
        "{skills}"
    );
    assert!(
        home.servers().is_empty(),
        "a command started a server of its own"
    );
}

/// A server that answers its identity on the home's socket, as a server of this build and home does, then closes every
/// call unanswered: a server that stops between the moment a command finds it and its call.
struct FoundThenGone {
    socket: std::path::PathBuf,
}

impl FoundThenGone {
    fn serve(home: &Home) -> FoundThenGone {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::fs::PermissionsExt;

        use quadratique_server::server::{CONTROL, Identity, Started};

        let identity = Identity {
            pid: std::process::id(),
            database: home
                .path()
                .canonicalize()
                .expect("a real path")
                .join("workspace.db"),
            interface: Some(quadratique_server::app::interface()),
            schema: Some(quadratique_server::store::SCHEMA_VERSION),
            generation: Some("0123456789abcdef0123456789abcdef".to_owned()),
            control: Some(CONTROL),
            controls: Some(vec![CONTROL]),
            executable: None,
            started: Some(Started::Serve),
            url: Some("http://127.0.0.1:9/cli".to_owned()),
            address_kept: None,
        };
        let body = serde_json::to_string(&identity).expect("the identity as JSON");
        let socket = home.path().join("server.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("the socket");
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
            .expect("mode 0600");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().expect("a stream"));
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() || !first.starts_with("GET /server ") {
                    continue;
                }
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                    line.clear();
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        FoundThenGone { socket }
    }

    fn lost(&self, again: &str) -> String {
        format!(
            "The Quadratique server stopped after it was found on {}, before it answered. {again}",
            self.socket.display()
        )
    }
}

#[test]
fn a_server_that_stops_after_it_was_found_is_named_by_its_socket_with_what_to_do() {
    let home = Home::new();
    let repository = Repository::new();
    let elsewhere = outside();
    let gone = FoundThenGone::serve(&home);

    let init = home
        .quadratique(&repository.root)
        .args(["init", "--key", "TAL"])
        .run();
    let mut mcp = Client::start(
        home.quadratique(&repository.root)
            .args(["mcp"])
            .spawn_interactive(),
    );
    let tool = mcp.refusal("issue.ready", json!({}));
    let session = mcp.finish();
    let skills = home
        .quadratique(elsewhere.path())
        .args(["upgrade", "--skills"])
        .run()
        .ok();

    assert_eq!(init.code, 4, "{init:?}");
    let init: serde_json::Value = serde_json::from_str(init.stderr.trim()).expect("one JSON line");
    assert_eq!(
        init,
        json!({
            "kind": "unavailable",
            "message": gone.lost("Run the command again: it starts the server."),
        })
    );
    assert!(tool.contains("The capability failed."), "{tool}");
    assert!(
        session
            .stderr
            .contains(&gone.lost("Call the tool again: the next call starts the server."))
            && !session.stderr.contains("the server answered"),
        "{session:?}"
    );
    assert_eq!(
        skills["skills"]["error"],
        format!(
            "the projects could not be listed: {}",
            gone.lost("Run the command again: it starts the server.")
        ),
        "{skills}"
    );
}
