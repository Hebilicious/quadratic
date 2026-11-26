//! Commands that answer without a server: they must neither start one nor write in the home.

use std::path::Path;
use std::process::{Command, Output};

const BINARY: &str = env!("CARGO_BIN_EXE_quadratique");
const SKILL: &str = include_str!("../../../skill/quadratique/SKILL.md");
const PLANNING: &str = include_str!("../../../skill/quadratique/planning.md");
const LINE: &str = "We use Quadratique for issues: every task is an issue there. Use the `quadratique` skill (`.agents/skills/quadratique/SKILL.md`) for anything about tasks.";

fn quadratique(home: &Path, args: &[&str]) -> Output {
    let elsewhere = tempfile::tempdir().expect("a folder outside any repository");
    Command::new(BINARY)
        .env("QUADRATIQUE_HOME", home)
        .current_dir(elsewhere.path())
        .args(args)
        .output()
        .expect("quadratique runs")
}

fn assert_no_server(home: &Path) {
    for file in ["server.json", "workspace.db", "server.lock"] {
        assert!(!home.join(file).exists(), "{file} exists in the home");
    }
}

#[test]
fn init_print_writes_the_build_text_of_each_file_as_it_is_and_refuses_an_unknown_name() {
    let home = tempfile::tempdir().expect("a temporary home");

    for (name, text) in [
        ("SKILL.md", SKILL.to_owned()),
        ("planning.md", PLANNING.to_owned()),
        ("AGENTS.md", format!("{LINE}\n")),
    ] {
        let printed = quadratique(home.path(), &["init", "--print", name]);
        assert!(printed.status.success(), "{printed:?}");
        assert_eq!(String::from_utf8_lossy(&printed.stdout), text, "{name}");
    }

    let refused = quadratique(home.path(), &["init", "--print", "README.md"]);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(refused.stdout.is_empty(), "{refused:?}");
    assert!(
        stderr.contains("SKILL.md, planning.md, AGENTS.md") && stderr.contains("README.md"),
        "{stderr}"
    );
    assert_no_server(home.path());
}

#[test]
fn serve_refuses_a_port_that_is_not_a_number() {
    let home = tempfile::tempdir().expect("a temporary home");

    let run = quadratique(home.path(), &["serve", "--port", "web"]);

    assert!(!run.status.success(), "{run:?}");
    assert_no_server(home.path());
}

/// On Windows, `serve` refuses before it creates anything: the data folder is not even made.
#[cfg(windows)]
#[test]
fn serve_on_windows_refuses_before_it_creates_the_data_folder() {
    let parent = tempfile::tempdir().expect("a temporary folder");
    let home = parent.path().join("home");

    let run = quadratique(&home, &["serve"]);

    assert!(!run.status.success(), "{run:?}");
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("QUA-61"),
        "{run:?}"
    );
    assert!(
        !home.exists(),
        "serve created the data folder before it refused"
    );
}

/// The release archive that a build for this platform installs, as the release workflow names it.
fn this_platform() -> Option<&'static str> {
    if cfg!(all(
        target_os = "linux",
        target_env = "gnu",
        target_arch = "x86_64"
    )) {
        Some("x86_64-unknown-linux-gnu")
    } else if cfg!(all(
        target_os = "linux",
        target_env = "gnu",
        target_arch = "aarch64"
    )) {
        Some("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else {
        None
    }
}

/// A release source on the loopback interface that describes the latest release as `description`, to every request.
fn release_source(description: serde_json::Value) -> String {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let body = description.to_string();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => request.extend_from_slice(&buffer[..read]),
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    origin
}

/// The latest release, 99.0.0, with an archive for each of `targets`. `--check` downloads none of them.
fn release_with(targets: &[&str]) -> serde_json::Value {
    let assets: Vec<serde_json::Value> = targets
        .iter()
        .map(|target| {
            let name = format!("quadratique-99.0.0-{target}.tar.gz");
            serde_json::json!({
                "name": name,
                "browser_download_url": format!("http://127.0.0.1:9/download/{name}"),
                "digest": format!("sha256:{}", "a".repeat(64)),
            })
        })
        .collect();
    serde_json::json!({ "tag_name": "v99.0.0", "assets": assets })
}

fn check_against(targets: &[&str]) -> serde_json::Value {
    let parent = tempfile::tempdir().expect("a temporary folder");
    let home = parent.path().join("home");
    let origin = release_source(release_with(targets));
    let elsewhere = tempfile::tempdir().expect("a folder outside any repository");
    let run = Command::new(BINARY)
        .env("QUADRATIQUE_HOME", &home)
        .env("QUADRATIQUE_UPGRADE_SOURCE", &origin)
        .current_dir(elsewhere.path())
        .args(["upgrade", "--check"])
        .output()
        .expect("quadratique runs");
    assert!(run.status.success(), "{run:?}");
    assert!(!home.exists(), "upgrade --check created the data folder");
    serde_json::from_slice(&run.stdout).expect("upgrade --check answers JSON")
}

#[test]
fn upgrade_check_names_the_archive_of_this_platform_in_a_release_that_has_one() {
    let answer = check_against(&[
        "aarch64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
    ]);

    assert_eq!(answer["upToDate"], false, "{answer}");
    match this_platform() {
        Some(target) => assert_eq!(
            answer["archive"],
            format!("quadratique-99.0.0-{target}.tar.gz"),
            "{answer}"
        ),
        None => assert!(
            answer["noArchive"]
                .as_str()
                .is_some_and(|why| why.contains("no archive for this platform")),
            "{answer}"
        ),
    }
}

#[test]
fn upgrade_check_says_when_the_release_has_no_archive_for_this_platform() {
    let answer = check_against(&["riscv64gc-unknown-linux-gnu"]);

    assert!(answer.get("archive").is_none(), "{answer}");
    assert!(
        answer["noArchive"]
            .as_str()
            .is_some_and(|why| why.contains("no archive for this platform")
                || why.contains("has no quadratique-99.0.0-")),
        "{answer}"
    );
}
