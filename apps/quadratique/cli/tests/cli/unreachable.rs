use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use quadratique_server::serve::OWNER_ONLY;
use serde_json::json;

use crate::support::{Home, Repository, Run, outside};

const UNAVAILABLE: i32 = 4;

fn of_another_account(home: &Home) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let address = listener.local_addr().expect("an address");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().expect("a stream"));
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                line.clear();
            }
            let _ = write!(
                stream,
                "HTTP/1.1 403 Forbidden\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{OWNER_ONLY}",
                OWNER_ONLY.len()
            );
        }
    });
    let database = home
        .path()
        .canonicalize()
        .expect("a real path")
        .join("workspace.db");
    let runtime = json!({
        "pid": std::process::id(),
        "address": address.to_string(),
        "url": format!("http://{address}/cli"),
        "database": database,
    });
    std::fs::write(home.runtime_path(), runtime.to_string()).expect("the runtime file");
}

fn hold_the_home(home: &Home) -> File {
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.path().join("server.lock"))
        .expect("the home's lock file");
    lock.lock().expect("a server of the home holds its lock");
    lock
}

/// A server from before the socket whose TCP listener answers only its own account: the command line asks TCP
/// nothing, and refuses as for any server of release 0.1 that it cannot prove (record 0004, §3).
#[test]
fn a_command_that_meets_a_server_from_before_the_socket_says_so_and_starts_nothing() {
    let home = Home::new();
    let elsewhere = outside();
    of_another_account(&home);
    let _server_of_the_home = hold_the_home(&home);

    let refused = home
        .quadratique(elsewhere.path())
        .args(["issue", "list"])
        .run();

    assert_eq!(refused.code, UNAVAILABLE, "{refused:?}");
    let message = message_of(&refused);
    assert!(
        message.contains("from before the socket (release 0.1)")
            && message.contains("Nothing was started.")
            && message.contains("systemctl --user stop quadratique.service"),
        "{message}"
    );
    assert!(
        !home.path().join("start.lock").exists(),
        "the command took the start lock"
    );
    assert!(
        !home.path().join("server.log").exists(),
        "the command spawned a server"
    );
    assert!(home.servers().is_empty());

    let status = home
        .quadratique(elsewhere.path())
        .args(["server", "status"])
        .run()
        .ok();
    assert_eq!(status["legacy"], true, "{status}");
    assert_eq!(status["provenStop"], false, "{status}");
    assert!(status["reason"].is_string(), "{status}");

    let stop = home
        .quadratique(elsewhere.path())
        .args(["server", "stop"])
        .run();
    assert_eq!(stop.code, UNAVAILABLE, "{stop:?}");
    assert!(stop.stderr.contains("Nothing was stopped."), "{stop:?}");
    assert!(!stop.stderr.contains("MCP"), "{stop:?}");
}

fn guidance(tool: &str) -> String {
    format!(
        "Nothing was started. Call the Quadratique MCP tool `{tool}` with the same inputs instead. \
         The MCP tools act from the folder this session started in; for work in another worktree, \
         pass its absolute path as `worktree` to `issue.claim`; once that claim succeeds, later calls act from it. \
         If this session has no Quadratique MCP tools, tell the human that its harness must start \
         `quadratique mcp` as an MCP server."
    )
}

fn message_of(refused: &Run) -> String {
    let failure: serde_json::Value =
        serde_json::from_str(refused.stderr.lines().last().unwrap_or_default())
            .unwrap_or_else(|error| panic!("standard error is not JSON ({error}): {refused:?}"));
    failure["message"]
        .as_str()
        .unwrap_or_else(|| panic!("no message: {refused:?}"))
        .to_owned()
}

const SEVERAL_ISSUES: &str = "Nothing was started. To act on these issues together, all or none, run this command \
     outside the sandbox, or ask the human to. If separate writes are acceptable, call the Quadratique MCP tool \
     `issue.close` once for each issue, with the same other inputs.";

const COMMAND_LINE_ONLY: &str = "Nothing was started. This command works only on the command line: \
     run it outside the sandbox, or ask the human to.";

const NOT_A_COMMAND: &str = "Nothing was started. `quadratique --help` lists the commands.";

fn plain(message: &str) -> bool {
    message.starts_with("Cannot reach the Quadratique server from here: ")
        && message.ends_with(". Nothing was started.")
        && !message.contains("MCP")
}

#[test]
fn in_a_sandbox_a_command_names_its_mcp_tool_or_says_it_works_only_on_the_command_line() {
    let home = Home::new();
    let repository = Repository::new();
    let _sandboxed = ReadOnly::make(home.path());
    let with_a_tool: [(&[&str], &str); 9] = [
        (&[], "issue.ready"),
        (&["--json"], "issue.ready"),
        (&["claim", "--issue", "DEM-1"], "issue.claim"),
        (
            &["close", "--issue", "DEM-1", "--evidence", "x"],
            "issue.close",
        ),
        (&["show", "--issue", "DEM-1"], "issue.get"),
        (&["instructions"], "workspace.instructions"),
        (&["issue", "list"], "issue.list"),
        (&["history", "query"], "history.query"),
        (&["call", "issue.ready", "{}"], "issue.ready"),
    ];
    let without_a_tool: [&[&str]; 9] = [
        &["init"],
        &["issue", "advance", "--issue", "DEM-1"],
        &["token", "list"],
        &["project", "list"],
        &["project", "create", "--key", "NEW", "--name", "New"],
        &[
            "findings",
            "import",
            "--project",
            "NEW",
            "--tool",
            "linter",
            "--findings",
            "[]",
        ],
        &["call", "project.list", "{}"],
        &["list"],
        &["watch", "issue.changed"],
    ];
    let not_commands: [&[&str]; 8] = [
        &["--issue", "DEM-1", "show"],
        &["--project", "DEM", "claim", "--issue", "DEM-1"],
        &["issue", "gett", "--issue", "DEM-1"],
        &["issue", "frob"],
        &["issue"],
        &["token"],
        &["call"],
        &["call", "nope.x", "{}"],
    ];

    for (args, tool) in with_a_tool {
        let refused = home.quadratique(&repository.root).args(args).run();
        assert_eq!(refused.code, UNAVAILABLE, "{args:?}: {refused:?}");
        let message = message_of(&refused);
        assert!(
            message.starts_with("Cannot reach the Quadratique server from here: ")
                && message.ends_with(&guidance(tool)),
            "{args:?}: {message}"
        );
    }
    for args in [
        &[
            "close",
            "--issue",
            "DEM-1",
            "--issue",
            "DEM-2",
            "--evidence",
            "x",
        ][..],
        &["close", "--issue=DEM-1", "--issue=DEM-2", "--evidence", "x"],
    ] {
        let refused = home.quadratique(&repository.root).args(args).run();
        assert_eq!(refused.code, UNAVAILABLE, "{args:?}: {refused:?}");
        let message = message_of(&refused);
        assert!(
            message.starts_with("Cannot reach the Quadratique server from here: ")
                && message.ends_with(SEVERAL_ISSUES),
            "{args:?}: {message}"
        );
    }
    for args in without_a_tool {
        let refused = home.quadratique(&repository.root).args(args).run();
        assert_eq!(refused.code, UNAVAILABLE, "{args:?}: {refused:?}");
        let message = message_of(&refused);
        assert!(
            message.starts_with("Cannot reach the Quadratique server from here: ")
                && message.ends_with(COMMAND_LINE_ONLY),
            "{args:?}: {message}"
        );
    }
    for args in not_commands {
        let refused = home.quadratique(&repository.root).args(args).run();
        assert_eq!(refused.code, UNAVAILABLE, "{args:?}: {refused:?}");
        let message = message_of(&refused);
        assert!(
            message.starts_with("Cannot reach the Quadratique server from here: ")
                && message.ends_with(NOT_A_COMMAND),
            "{args:?}: {message}"
        );
    }
    let ui = message_of(&home.quadratique(&repository.root).args(["ui"]).run());
    assert!(plain(&ui), "a person runs `ui`: {ui}");
}

#[test]
fn a_stale_runtime_file_that_names_a_server_of_another_account_does_not_stop_a_start() {
    let home = Home::new();
    let elsewhere = outside();
    of_another_account(&home);

    let projects = home
        .quadratique(elsewhere.path())
        .args(["project", "list"])
        .run()
        .ok();

    assert_eq!(projects, json!({ "items": [] }));
    let runtime = home.runtime().expect("the new server's runtime file");
    assert_eq!(
        home.servers(),
        vec![runtime["pid"].as_u64().expect("a pid") as u32]
    );
}

/// The home and the files in it stay read-only for this test's process, as on a read-only file system, until the
/// guard is dropped.
#[test]
fn in_a_sandbox_init_prints_the_build_text_of_a_skill_file_without_a_server() {
    let home = Home::new();
    let elsewhere = outside();
    let _sandboxed = ReadOnly::make(home.path());

    let printed = home
        .quadratique(elsewhere.path())
        .args(["init", "--print", "SKILL.md"])
        .run();

    assert_eq!(printed.code, 0, "{printed:?}");
    assert_eq!(
        printed.stdout,
        include_str!("../../../skill/quadratique/SKILL.md")
    );
    assert!(home.servers().is_empty());
    assert_eq!(entries(home.path()), Vec::<String>::new());
}

struct ReadOnly(PathBuf);

impl ReadOnly {
    fn make(home: &Path) -> ReadOnly {
        for entry in std::fs::read_dir(home).expect("the home") {
            let path = entry.expect("an entry").path();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400))
                .expect("mode 0400");
        }
        std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o500)).expect("mode 0500");
        ReadOnly(home.to_path_buf())
    }
}

impl Drop for ReadOnly {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
    }
}

fn entries(home: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(home)
        .expect("the home")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// A read-only home held by a server from before the socket: status says what holds it, a command refuses as for that
/// server, and a stop, which must take `start.lock`, cannot reach it. Nothing in the home changes.
#[test]
fn a_caller_that_cannot_write_a_home_held_by_a_server_from_before_the_socket_changes_nothing() {
    let home = Home::new();
    let elsewhere = outside();
    of_another_account(&home);
    let _server_of_the_home = hold_the_home(&home);
    let before = entries(home.path());
    let _read_only = ReadOnly::make(home.path());
    let run = |args: &[&str]| home.quadratique(elsewhere.path()).args(args).run();

    let status = run(&["server", "status"]).ok();
    let listed = run(&["issue", "list"]);
    let stopped = run(&["server", "stop"]);

    assert_eq!(status["legacy"], true, "{status}");
    assert_eq!(listed.code, UNAVAILABLE, "{listed:?}");
    assert!(
        listed
            .stderr
            .contains("from before the socket (release 0.1)")
            && listed.stderr.contains("Nothing was started"),
        "{listed:?}"
    );
    assert_eq!(stopped.code, UNAVAILABLE, "{stopped:?}");
    assert!(
        stopped
            .stderr
            .contains("Cannot reach the Quadratique server from here")
            && stopped.stderr.contains("Nothing was stopped"),
        "{stopped:?}"
    );
    assert_eq!(entries(home.path()), before);
}

#[test]
fn a_caller_that_cannot_write_a_home_without_a_server_starts_nothing_and_says_why() {
    let home = Home::new();
    let elsewhere = outside();
    let _read_only = ReadOnly::make(home.path());
    let run = |args: &[&str]| home.quadratique(elsewhere.path()).args(args).run();
    let cannot_write = format!("cannot write {}", home.path().display());

    for args in [&["issue", "list"][..], &["serve", "--take-over"][..]] {
        let refused = run(args);
        assert_eq!(refused.code, UNAVAILABLE, "{args:?}: {refused:?}");
        assert!(
            refused
                .stderr
                .contains("Cannot reach the Quadratique server from here")
                && refused.stderr.contains(&cannot_write)
                && refused.stderr.contains("Nothing was started"),
            "{args:?}: {refused:?}"
        );
    }
    let take_over = message_of(&run(&["serve", "--take-over"]));
    assert!(plain(&take_over), "{take_over}");
    let status = run(&["server", "status"]).ok();
    assert_eq!(status["running"], false, "{status}");
    let stopped = run(&["server", "stop"]).ok();
    assert_eq!(stopped["stopped"], false, "{stopped}");
    assert!(
        entries(home.path()).is_empty(),
        "{:?}",
        entries(home.path())
    );
    assert!(home.servers().is_empty());
}

#[test]
fn quadratique_mcp_that_cannot_reach_or_start_the_server_exits_with_the_reason_and_offers_no_tools()
{
    let home = Home::new();
    let elsewhere = outside();
    let _read_only = ReadOnly::make(home.path());
    let mut mcp = home
        .quadratique(elsewhere.path())
        .args(["mcp"])
        .spawn_interactive();
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" },
        },
    });
    let _ = writeln!(mcp.stdin(), "{initialize}");
    mcp.close_stdin();

    let ended = mcp.finish();

    assert_eq!(ended.code, UNAVAILABLE, "{ended:?}");
    assert!(
        ended
            .stderr
            .contains("Cannot reach the Quadratique server from here")
            && ended.stderr.contains("Nothing was started."),
        "{ended:?}"
    );
    assert!(!ended.stderr.contains("MCP tools"), "{ended:?}");
    assert!(ended.stdout.is_empty(), "{ended:?}");
}
