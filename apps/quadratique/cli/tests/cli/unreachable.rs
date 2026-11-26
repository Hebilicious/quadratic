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
     `issue.finish` once for each issue, with the same other inputs.";

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
    let with_a_tool: [(&[&str], &str); 11] = [
        (&[], "issue.ready"),
        (&["--json"], "issue.ready"),
        (&["claim", "--issue", "DEM-1"], "issue.claim"),
        (
            &["finish", "--issue", "DEM-1", "--evidence", "x"],
            "issue.finish",
        ),
        (
            &["close", "--issue", "DEM-1", "--evidence", "x"],
            "issue.finish",
        ),
        (
            &["issue", "close", "--issue", "DEM-1", "--evidence", "x"],
            "issue.finish",
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
            "finish",
            "--issue",
            "DEM-1",
            "--issue",
            "DEM-2",
            "--evidence",
            "x",
        ][..],
        &[
            "finish",
            "--issue=DEM-1",
            "--issue=DEM-2",
            "--evidence",
            "x",
        ],
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

/// A read-only home held by a server from before the socket that this command line cannot prove: status says what
/// holds it, and a command and a stop refuse as for that server before they reserve anything. Nothing in the home
/// changes.
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
            .contains("from before the socket (release 0.1)")
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

/// A folder under `/tmp` that every account may traverse, so that only the modes of what it holds stop the account
/// nobody.
fn traversable_folder() -> tempfile::TempDir {
    let folder = tempfile::Builder::new()
        .prefix("q-")
        .tempdir_in("/tmp")
        .expect("a folder in /tmp");
    std::fs::set_permissions(folder.path(), std::fs::Permissions::from_mode(0o755))
        .expect("mode 0755");
    folder
}

/// The processes that run `program`.
fn running(program: &Path) -> Vec<u32> {
    std::fs::read_dir("/proc")
        .expect("/proc lists the processes")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            cmdline.split(|byte| *byte == 0).next() == Some(program.as_os_str().as_encoded_bytes())
        })
        .collect()
}

/// Another account names this account's data folder, which lies in a private folder as a home folder does, while this
/// account's server serves it: the command cannot reach that server, says so as for a sandbox, and starts nothing.
#[test]
#[ignore = "needs `sudo -n` as the account nobody, and /tmp, which nobody can traverse; CI runs it with `moon run quadratique-server:test-as-nobody`"]
fn another_account_cannot_reach_the_server_of_this_account_and_starts_nothing() {
    use quadratique_server::server::{self, Paths};

    let folder = traversable_folder();
    let program = folder.path().join("quadratique");
    let copied = std::process::Command::new("cp")
        .arg(crate::support::BINARY)
        .arg(&program)
        .status()
        .expect("cp runs");
    assert!(copied.success(), "the binary is copied");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("mode 0755");
    let private = folder.path().join("private");
    std::fs::create_dir(&private).expect("a private folder");
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).expect("mode 0700");
    let home = private.join("home");
    let runtime = tokio::runtime::Runtime::new().expect("a runtime for the server");
    let serving = runtime
        .block_on(server::start(
            &Paths::in_home(&home).expect("an absolute home"),
            server::loopback(),
        ))
        .expect("this account's server starts");
    let before = entries(&home);

    let refused = std::process::Command::new("sudo")
        .current_dir(folder.path())
        .args(["-n", "-u", "nobody", "env"])
        .arg(format!("QUADRATIQUE_HOME={}", home.display()))
        .arg(&program)
        .args(["issue", "list"])
        .output()
        .expect("sudo runs");
    let started_by_nobody = running(&program);
    let after = entries(&home);
    runtime.block_on(serving.shutdown());

    let refused = Run {
        code: refused.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&refused.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&refused.stderr).into_owned(),
    };
    println!("{refused:?}");
    assert_eq!(refused.code, UNAVAILABLE, "{refused:?}");
    let message = message_of(&refused);
    assert!(
        message.starts_with(&format!(
            "Cannot reach the Quadratique server from here: this process cannot use {}: Permission denied",
            home.display()
        )) && message.ends_with(&guidance("issue.list")),
        "{message}"
    );
    assert_eq!(
        started_by_nobody,
        Vec::<u32>::new(),
        "the command of the account nobody started a server"
    );
    assert_eq!(after, before);
}

/// The home's folder, closed to every account but root until the guard is dropped: a sandbox that hides the data
/// folder, or another account's home folder.
struct Closed(PathBuf);

impl Closed {
    fn make(home: &Path) -> Closed {
        std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o000)).expect("mode 0000");
        Closed(home.to_path_buf())
    }
}

impl Drop for Closed {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
    }
}

/// One rule for a data folder that this process cannot enter, whatever the command: `server status` answers
/// `reachable: false` with the reason, as for a socket it cannot connect to, and every other command that would reach
/// the server is `unavailable` (exit 4) with "Cannot reach the Quadratique server from here". `serve` is the server
/// itself, not a caller: its home is unusable to it, an internal failure.
#[test]
fn a_data_folder_this_process_cannot_enter_is_unreachable_for_every_command_and_status_says_so() {
    let home = Home::new();
    let elsewhere = outside();
    let cannot_use = format!("this process cannot use {}", home.path().display());
    let _closed = Closed::make(home.path());
    let run = |args: &[&str]| home.quadratique(elsewhere.path()).args(args).run();

    let status = run(&["server", "status"]);
    let caller_commands = [
        (&["issue", "list"][..], "Nothing was started."),
        (&["ui"], "Nothing was started."),
        (&["server", "stop"], "Nothing was stopped."),
        (&["server", "sign-out"], "Nothing was started."),
        (&["server", "strict", "on"], "Nothing was started."),
    ];
    let served = run(&["serve"]);

    assert_eq!(status.code, 0, "{status:?}");
    let status = status.json();
    assert_eq!(status["reachable"], false, "{status}");
    assert!(
        status["reason"]
            .as_str()
            .is_some_and(|reason| reason.starts_with(&cannot_use)),
        "{status}"
    );
    for (args, outcome) in caller_commands {
        let refused = run(args);
        assert_eq!(refused.code, UNAVAILABLE, "{args:?}: {refused:?}");
        let message = message_of(&refused);
        assert!(
            message.starts_with(&format!(
                "Cannot reach the Quadratique server from here: {cannot_use}"
            )) && message.contains(outcome),
            "{args:?}: {message}"
        );
    }
    assert_eq!(served.code, 5, "{served:?}");
    assert!(
        served.stderr.contains("\"kind\":\"internal\""),
        "{served:?}"
    );
}

/// What `unshare` takes to run a command as Codex's sandbox does: in a user namespace that maps this account to
/// itself and nothing else, where every folder of root (`/`, `/home`, `/tmp`) shows the kernel's overflow uid. `None`,
/// saying why, where this machine makes no such namespace; with `QUADRATIQUE_TEST_USER_NAMESPACES=required`, as CI
/// sets it, that is a failure instead.
fn mapping_only_this_account() -> Option<[String; 2]> {
    let prefix = [
        "--user".to_owned(),
        format!("--map-user={}", rustix::process::getuid().as_raw()),
    ];
    let probe = std::process::Command::new("unshare")
        .args(&prefix)
        .arg("true")
        .output();
    let missing = match probe {
        Ok(output) if output.status.success() => return Some(prefix),
        Ok(output) => String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        Err(error) => format!("unshare does not run: {error}"),
    };
    let required = std::env::var("QUADRATIQUE_TEST_USER_NAMESPACES").as_deref() == Ok("required");
    assert!(
        !required,
        "QUADRATIQUE_TEST_USER_NAMESPACES=required, and this machine makes no user namespace: {missing}"
    );
    println!("skipped: this machine makes no user namespace that maps this account ({missing})");
    None
}

/// In a user namespace that does not map root, the owner of a folder above the data folder shows as the overflow uid,
/// which may be root or another account: the policy still refuses the folder, and every command that would reach the
/// server answers as for any place it cannot reach the server from, `unavailable` with its MCP tool, and starts
/// nothing. `serve` is the server itself and refuses as before.
#[test]
fn where_a_sandbox_hides_who_owns_the_folders_above_the_home_every_command_is_unavailable_and_starts_nothing()
 {
    let Some(prefix) = mapping_only_this_account() else {
        return;
    };
    let home = Home::new();
    let elsewhere = outside();
    let repository = Repository::new();
    let hidden = "this sandbox hides who owns ";
    let cannot_check =
        ", so this command cannot check that the data folder is safe to use from here";
    let in_the_namespace = |directory: &Path| {
        home.quadratique_at(Path::new("unshare"), directory)
            .args(&prefix)
            .args([crate::support::BINARY])
    };

    let listed = in_the_namespace(elsewhere.path())
        .args(["issue", "list"])
        .run();
    let initialised = in_the_namespace(&repository.root).args(["init"]).run();
    let status = in_the_namespace(elsewhere.path())
        .args(["server", "status"])
        .run();
    let served = in_the_namespace(elsewhere.path()).args(["serve"]).run();
    let mut mcp = in_the_namespace(elsewhere.path())
        .args(["mcp"])
        .spawn_interactive();
    mcp.close_stdin();
    let mcp = mcp.finish();

    for (refused, outcome) in [
        (&listed, guidance("issue.list")),
        (&initialised, COMMAND_LINE_ONLY.to_owned()),
        (&mcp, "Nothing was started.".to_owned()),
    ] {
        assert_eq!(refused.code, UNAVAILABLE, "{refused:?}");
        let message = message_of(refused);
        assert!(
            message.starts_with(&format!(
                "Cannot reach the Quadratique server from here: {hidden}"
            )) && message.contains(cannot_check)
                && message.ends_with(outcome.as_str()),
            "{message}"
        );
    }
    assert!(mcp.stdout.is_empty(), "{mcp:?}");
    assert_eq!(status.code, 0, "{status:?}");
    let status = status.json();
    assert_eq!(status["reachable"], false, "{status}");
    assert!(
        status["reason"]
            .as_str()
            .is_some_and(|reason| reason.starts_with(hidden) && reason.ends_with(cannot_check)),
        "{status}"
    );
    assert_eq!(served.code, 5, "{served:?}");
    assert!(
        served.stderr.contains("\"kind\":\"internal\""),
        "{served:?}"
    );
    assert!(home.servers().is_empty());
    assert_eq!(entries(home.path()), Vec::<String>::new());
}

/// Outside a user namespace, a folder above the data folder that the account nobody really owns is refused as any
/// other account's folder, even though nobody's uid is the kernel's overflow uid.
#[test]
#[ignore = "needs `sudo -n` as root, and /tmp; CI runs it with `moon run quadratique-server:test-as-nobody`"]
fn another_account_that_owns_a_folder_above_the_home_is_refused_even_as_the_overflow_uid() {
    let folder = traversable_folder();
    let owned = folder.path().join("nobody");
    std::fs::create_dir(&owned).expect("a folder");
    let chowned = std::process::Command::new("sudo")
        .args(["-n", "chown", "nobody"])
        .arg(&owned)
        .status()
        .expect("sudo runs");
    assert!(chowned.success(), "root gives the folder to nobody");
    let nobody =
        std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&owned).expect("the folder"));
    let overflow: u32 = std::fs::read_to_string("/proc/sys/kernel/overflowuid")
        .expect("the overflow uid")
        .trim()
        .parse()
        .expect("a uid");
    assert_eq!(
        nobody, overflow,
        "the account nobody has the overflow uid here"
    );
    let elsewhere = outside();

    let refused = std::process::Command::new(crate::support::BINARY)
        .current_dir(elsewhere.path())
        .env("QUADRATIQUE_HOME", owned.join("home"))
        .args(["issue", "list"])
        .output()
        .expect("the command runs");

    let refused = Run {
        code: refused.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&refused.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&refused.stderr).into_owned(),
    };
    println!("{refused:?}");
    assert_eq!(refused.code, 5, "{refused:?}");
    assert_eq!(
        message_of(&refused),
        format!(
            "another account can replace {}: it belongs to another account (uid {nobody}); ask its owner to give it to you or to root, or set QUADRATIQUE_HOME to a folder that only you and root can change, such as one in your home folder",
            owned.display()
        )
    );
}
