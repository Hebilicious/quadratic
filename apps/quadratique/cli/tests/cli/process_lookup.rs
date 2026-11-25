//! The macOS helpers that find a home's processes by their environment, whatever spaces its path holds.

use std::process::{Command, Stdio};

use crate::support::macos;

#[test]
fn a_process_is_found_by_its_environment_even_when_the_path_has_spaces() {
    let folder = tempfile::Builder::new()
        .prefix("quadratique home with spaces ")
        .tempdir()
        .expect("a folder whose path has spaces");
    let marker = format!("HOME={}", folder.path().display());
    let mut child = Command::new("/bin/sleep")
        .arg("600")
        .env_clear()
        .env("HOME", folder.path())
        .stdin(Stdio::null())
        .spawn()
        .expect("sleep starts");
    let pid = child.id();

    let candidates = macos::pids_mentioning(&marker);
    let arguments = macos::arguments(pid);
    let environment: Vec<String> = arguments
        .iter()
        .flat_map(|arguments| arguments.environment.iter())
        .map(|variable| String::from_utf8_lossy(variable).into_owned())
        .collect();
    let argv: Vec<String> = arguments
        .iter()
        .flat_map(|arguments| arguments.argv.iter())
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
        .collect();
    let ended_while_running = macos::has_ended(pid);
    child.kill().expect("sleep is killed");
    child.wait().expect("sleep ends");

    assert!(
        candidates.contains(&pid),
        "ps -E does not list {pid}: {candidates:?}"
    );
    assert_eq!(argv, ["/bin/sleep", "600"], "{environment:?}");
    assert_eq!(environment, [marker]);
    assert!(!ended_while_running);
    assert!(macos::has_ended(pid));
}
