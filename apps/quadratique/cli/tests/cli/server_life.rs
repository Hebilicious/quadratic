use serde_json::{Value, json};

use crate::support::{Home, Process, Repository, await_exit, kill, outside};

#[test]
fn a_command_in_a_fresh_home_starts_the_server_the_next_reuses_it_and_stop_ends_it() {
    let home = Home::new();
    let elsewhere = outside();
    assert!(home.served().is_none());

    let projects = home
        .quadratique(elsewhere.path())
        .args(["project", "list"])
        .run()
        .ok();
    assert_eq!(projects, json!({ "items": [] }));
    let pid = home.served_pid().expect("the server answers on its socket");
    assert_eq!(home.servers(), vec![pid]);

    let status = home
        .quadratique(elsewhere.path())
        .args(["server", "status"])
        .run()
        .ok();
    assert_eq!(status["running"], true);
    assert_eq!(status["pid"], pid);

    home.quadratique(elsewhere.path())
        .args(["project", "list", "--json"])
        .run()
        .ok();
    assert_eq!(home.served_pid(), Some(pid));
    assert_eq!(home.servers(), vec![pid]);

    let stopped = home
        .quadratique(elsewhere.path())
        .args(["server", "stop"])
        .run()
        .ok();
    assert_eq!(stopped["stopped"], true);
    assert_eq!(stopped["pid"], pid);
    assert!(home.served().is_none());
    assert!(!home.runtime_path().exists(), "a server wrote server.json");
    await_exit(pid);
    assert!(home.servers().is_empty());

    let status = home
        .quadratique(elsewhere.path())
        .args(["server", "status"])
        .run()
        .ok();
    assert_eq!(status["running"], false);
}

#[test]
fn a_server_that_died_is_replaced_by_the_next_command_which_opens_the_same_data() {
    let home = Home::new();
    let elsewhere = outside();
    let run = |args: &[&str]| home.quadratique(elsewhere.path()).args(args).run();
    run(&["project", "create", "--key", "KEEP", "--name", "Kept"]).ok();
    run(&[
        "create",
        "--project",
        "KEEP",
        "--title",
        "Survives",
        "--acceptance",
        "It is done.",
    ])
    .ok();
    let dead = home.served_pid().expect("the server answers");

    kill(dead);
    await_exit(dead);
    assert!(
        home.path().join("server.sock").exists(),
        "the stale socket stays"
    );
    assert!(home.served().is_none());

    let shown = run(&["show", "--issue", "KEEP-1", "--json"]).ok();
    assert_eq!(shown["issue"]["title"], "Survives");
    let replaced = home.served_pid().expect("a new server answers");
    assert_ne!(replaced, dead);
    assert_eq!(home.servers(), vec![replaced]);
    let history = run(&["history", "query", "--issue", "KEEP-1"]).ok();
    let operations: Vec<&Value> = history["items"]
        .as_array()
        .expect("the changes")
        .iter()
        .map(|change| &change["operation"])
        .collect();
    assert_eq!(operations, [&json!("issue.create")]);
}

#[test]
fn commands_that_start_together_in_a_fresh_home_share_one_server() {
    const STARTERS: usize = 8;
    let home = Home::new();
    let elsewhere = outside();

    let children: Vec<_> = (0..STARTERS)
        .map(|_| {
            home.quadratique(elsewhere.path())
                .args(["instructions"])
                .spawn()
        })
        .collect();
    for run in children.into_iter().map(Process::finish) {
        assert_eq!(run.code, 0, "a starter fails: {run:?}");
        assert!(run.json()["text"].is_string(), "{run:?}");
    }

    let pid = home.served_pid().expect("the server answers");
    assert_eq!(home.servers(), vec![pid]);
}

#[test]
fn a_home_inside_a_git_working_tree_is_refused_before_any_file_is_written_there() {
    let home = Home::new();
    let repository = Repository::new();
    let inside = repository.root.join("quadratique-home");

    let refused = home
        .quadratique(&repository.root)
        .env("QUADRATIQUE_HOME", &inside.display().to_string())
        .args(["project", "list"])
        .run();

    assert_ne!(refused.code, 0, "{refused:?}");
    assert!(refused.stderr.contains("git working tree"), "{refused:?}");
    assert!(!inside.exists(), "files were written into the repository");
}

#[test]
fn a_home_under_an_empty_git_folder_is_not_taken_for_a_git_working_tree() {
    let home = Home::new();
    std::fs::create_dir(home.path().join(".git")).expect("an empty .git folder");
    let data = home.path().join(".local/share/quadratique");
    let data = data.display().to_string();

    let projects = home
        .quadratique(home.path())
        .env("QUADRATIQUE_HOME", &data)
        .args(["project", "list"])
        .run()
        .ok();
    assert_eq!(projects, json!({ "items": [] }));

    let stopped = home
        .quadratique(home.path())
        .env("QUADRATIQUE_HOME", &data)
        .args(["server", "stop"])
        .run()
        .ok();
    assert_eq!(stopped["stopped"], true);
}

#[test]
fn a_relative_home_names_one_directory_for_the_command_and_its_server() {
    let home = Home::new();
    let relative = "relative-home";

    let projects = home
        .quadratique(home.path())
        .env("QUADRATIQUE_HOME", relative)
        .args(["project", "list"])
        .run()
        .ok();
    assert_eq!(projects, json!({ "items": [] }));
    let directory = home.path().join(relative);
    assert!(directory.join("workspace.db").exists());
    assert!(!directory.join(relative).exists());

    let stopped = home
        .quadratique(home.path())
        .env("QUADRATIQUE_HOME", relative)
        .args(["server", "stop"])
        .run()
        .ok();
    assert_eq!(stopped["stopped"], true);
}

#[test]
fn a_stale_runtime_file_that_points_at_another_homes_server_is_not_used() {
    let ours = Home::new();
    let theirs = Home::new();
    let elsewhere = outside();
    theirs
        .quadratique(elsewhere.path())
        .args(["project", "create", "--key", "OTH", "--name", "Theirs"])
        .run()
        .ok();
    let theirs_pid = theirs.served_pid().expect("their server answers");
    let stale = json!({
        "pid": theirs_pid,
        "address": "127.0.0.1:9",
        "url": "http://127.0.0.1:9/cli",
        "database": ours.path().canonicalize().unwrap().join("workspace.db"),
    });
    std::fs::write(ours.runtime_path(), stale.to_string()).expect("the stale runtime file");

    let projects = ours
        .quadratique(elsewhere.path())
        .args(["project", "list"])
        .run()
        .ok();

    assert_eq!(projects, json!({ "items": [] }));
    let started = ours.served_pid().expect("our own server answers");
    assert_ne!(started, theirs_pid);
}

/// The port given to `serve` is the web app's: it listens there once `quadratique ui` asks.
#[test]
fn serve_with_a_port_gives_that_loopback_port_to_the_web_app() {
    let home = Home::new();
    let elsewhere = outside();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free loopback port")
        .port();
    let mut server = home
        .quadratique(elsewhere.path())
        .args(["serve", "--port", &port.to_string()])
        .spawn();
    home.await_served_by(server.id());
    let before = home
        .quadratique(elsewhere.path())
        .args(["server", "status"])
        .run()
        .ok();
    let paired = home.quadratique(elsewhere.path()).args(["ui"]).run().ok();
    let after = home
        .quadratique(elsewhere.path())
        .args(["server", "status"])
        .run()
        .ok();

    assert_eq!(before.get("web"), None, "{before}");
    assert_eq!(paired["url"], format!("http://127.0.0.1:{port}/"));
    assert_eq!(after["web"], format!("http://127.0.0.1:{port}/"));
}

#[test]
fn commands_with_and_without_a_runtime_directory_find_the_same_server_of_the_default_home() {
    let home = Home::new();
    let elsewhere = outside();
    let data = home.path().join("data");
    let runtime_directory = home.path().join("run");
    std::fs::create_dir(&runtime_directory).expect("a runtime directory");
    let in_default_home = || {
        home.quadratique(elsewhere.path())
            .without_env("QUADRATIQUE_HOME")
            .env("XDG_DATA_HOME", &data.display().to_string())
    };

    let started = in_default_home().args(["server", "status"]).run().ok();
    assert_eq!(started["running"], false);
    let url = in_default_home().args(["ui"]).run().ok()["url"].clone();
    let pid = in_default_home().args(["server", "status"]).run().ok()["pid"].clone();

    let seen = in_default_home()
        .env("XDG_RUNTIME_DIR", &runtime_directory.display().to_string())
        .args(["server", "status"])
        .run()
        .ok();
    assert_eq!(
        (&seen["running"], &seen["pid"]),
        (&json!(true), &pid),
        "{seen}"
    );

    let reused = in_default_home()
        .env("XDG_RUNTIME_DIR", &runtime_directory.display().to_string())
        .args(["ui"])
        .run()
        .ok();
    assert_eq!(reused["url"], url);
    assert_eq!(home.servers().len(), 1);
}

/// No server writes a runtime file: the answer comes over the socket, and names no TCP address of the command line.
#[test]
fn server_status_reaches_the_server_over_its_socket_and_reports_its_generation_and_control_version()
{
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    let home = Home::new();
    let elsewhere = outside();
    home.quadratique(elsewhere.path())
        .args(["project", "list"])
        .run()
        .ok();
    let pid = home.served_pid().expect("the server answers");
    let socket = home.path().join("server.sock");
    let socket_meta = std::fs::symlink_metadata(&socket).expect("the socket");
    assert!(socket_meta.file_type().is_socket());
    assert_eq!(socket_meta.permissions().mode() & 0o777, 0o600);

    let status = home
        .quadratique(elsewhere.path())
        .args(["server", "status"])
        .run()
        .ok();

    assert_eq!(status["running"], true, "{status}");
    assert_eq!(status["sameInterface"], true, "{status}");
    assert_eq!(status["pid"], pid, "{status}");
    assert_eq!(status.get("url"), None, "{status}");
    assert_eq!(status.get("runtime"), None, "{status}");
    assert_eq!(status.get("web"), None, "{status}");
    assert_eq!(status["control"], 1, "{status}");
    assert_eq!(status["started"], "on-demand", "{status}");
    let generation = status["generation"].as_str().expect("a generation");
    assert!(
        generation.len() == 32 && generation.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{status}"
    );

    let stopped = home
        .quadratique(elsewhere.path())
        .args(["server", "stop"])
        .run()
        .ok();
    assert_eq!(stopped["stopped"], true, "{stopped}");
}

#[test]
fn a_home_that_the_policy_refuses_gets_no_lock_file_and_no_server() {
    use std::os::unix::fs::PermissionsExt;

    let home = Home::new();
    let elsewhere = outside();
    std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o750))
        .expect("a home its group may read");

    let refused = home
        .quadratique(elsewhere.path())
        .args(["project", "list"])
        .run();

    assert_ne!(refused.code, 0, "{refused:?}");
    assert!(
        refused.stderr.contains("is not private") && refused.stderr.contains("chmod 700"),
        "{refused:?}"
    );
    let left: Vec<_> = std::fs::read_dir(home.path())
        .expect("the home")
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert!(left.is_empty(), "the refused home holds {left:?}");
    assert!(home.servers().is_empty());
}

#[test]
fn a_missing_default_home_is_made_private_with_the_folders_above_it() {
    use std::os::unix::fs::PermissionsExt;

    let home = Home::new();
    let elsewhere = outside();
    let data = home.path().join("data");

    let status = home
        .quadratique(elsewhere.path())
        .without_env("QUADRATIQUE_HOME")
        .env("XDG_DATA_HOME", &data.display().to_string())
        .args(["server", "status"])
        .run()
        .ok();

    assert_eq!(status["running"], false, "{status}");
    for folder in [data.clone(), data.join("quadratique")] {
        let mode = std::fs::metadata(&folder)
            .expect("the folder")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "{}", folder.display());
    }
}

/// The control protocol adds a way to stop the server; it takes none away. A person stops `quadratique serve` with
/// Ctrl-C in its terminal, and a supervisor with SIGTERM.
#[test]
fn sigterm_and_sigint_still_stop_a_server_started_with_serve() {
    use rustix::process::{Pid, Signal, kill_process};

    for signal in [Signal::TERM, Signal::INT] {
        let home = Home::new();
        let elsewhere = outside();
        let mut server = home.quadratique(elsewhere.path()).args(["serve"]).spawn();
        let pid = server.id();
        home.await_served_by(pid);
        assert_eq!(
            home.quadratique(elsewhere.path())
                .args(["server", "status"])
                .run()
                .ok()["pid"],
            pid
        );

        kill_process(
            Pid::from_raw(i32::try_from(pid).expect("a pid")).expect("a pid"),
            signal,
        )
        .expect("the signal is sent");
        let ended = server.finish();

        assert_eq!(ended.code, 0, "{signal:?}: {ended:?}");
        assert!(!home.path().join("server.sock").exists(), "{signal:?}");
        assert!(!home.runtime_path().exists(), "{signal:?}");
        let status = home
            .quadratique(elsewhere.path())
            .args(["server", "status"])
            .run()
            .ok();
        assert_eq!(status["running"], false, "{signal:?}: {status}");
    }
}
