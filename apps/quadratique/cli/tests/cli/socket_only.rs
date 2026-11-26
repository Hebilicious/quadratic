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
    let mut watcher = home
        .quadratique(&repository.root)
        .args(["watch", "issue.changed"])
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
