use serde_json::{Value, json};

use crate::support::{Home, Process, Repository, await_exit, kill, outside};

#[test]
fn a_command_in_a_fresh_home_starts_the_server_the_next_reuses_it_and_stop_ends_it() {
    let home = Home::new();
    let elsewhere = outside();
    assert!(home.runtime().is_none());

    let projects = home
        .quadratique(elsewhere.path())
        .args(["project", "list"])
        .run()
        .ok();
    assert_eq!(projects, json!({ "items": [] }));
    let first = home
        .runtime()
        .expect("the server published its runtime file");
    let pid = first["pid"].as_u64().expect("a pid") as u32;
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
    assert_eq!(home.runtime().expect("the runtime file")["pid"], pid);
    assert_eq!(home.servers(), vec![pid]);

    let stopped = home
        .quadratique(elsewhere.path())
        .args(["server", "stop"])
        .run()
        .ok();
    assert_eq!(stopped["stopped"], true);
    assert_eq!(stopped["pid"], pid);
    assert!(home.runtime().is_none());
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
    let dead = home.runtime().expect("the runtime file")["pid"]
        .as_u64()
        .expect("a pid") as u32;

    kill(dead);
    await_exit(dead);
    assert_eq!(
        home.runtime().expect("the stale runtime file stays")["pid"],
        dead
    );

    let shown = run(&["show", "--issue", "KEEP-1", "--json"]).ok();
    assert_eq!(shown["issue"]["title"], "Survives");
    let replaced = home.runtime().expect("a new runtime file")["pid"]
        .as_u64()
        .expect("a pid") as u32;
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

    let pid = home.runtime().expect("the runtime file")["pid"]
        .as_u64()
        .expect("a pid") as u32;
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
    let mut stale = theirs.runtime().expect("their runtime file");
    stale["database"] = json!(ours.path().canonicalize().unwrap().join("workspace.db"));
    std::fs::write(ours.runtime_path(), stale.to_string()).expect("the stale runtime file");

    let projects = ours
        .quadratique(elsewhere.path())
        .args(["project", "list"])
        .run()
        .ok();

    assert_eq!(projects, json!({ "items": [] }));
    let started = ours.runtime().expect("our own runtime file")["pid"].clone();
    assert_ne!(started, stale["pid"]);
}

#[test]
fn serve_with_a_port_listens_on_that_loopback_port() {
    let home = Home::new();
    let elsewhere = outside();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free loopback port")
        .port();

    let _server = home
        .quadratique(elsewhere.path())
        .args(["serve", "--port", &port.to_string()])
        .spawn();
    let deadline = std::time::Instant::now() + crate::support::HANG_GUARD;
    let runtime = loop {
        if let Some(runtime) = home.runtime() {
            break runtime;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the server published no runtime file within the hang guard"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(runtime["address"], format!("127.0.0.1:{port}"));
    let status = home
        .quadratique(elsewhere.path())
        .args(["server", "status"])
        .run()
        .ok();
    assert_eq!(status["url"], format!("http://127.0.0.1:{port}/cli"));
}

#[test]
fn serve_refuses_a_port_that_is_not_a_number() {
    let home = Home::new();
    let elsewhere = outside();
    let run = home
        .quadratique(elsewhere.path())
        .args(["serve", "--port", "web"])
        .run();
    assert_ne!(run.code, 0);
    assert!(home.runtime().is_none());
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
