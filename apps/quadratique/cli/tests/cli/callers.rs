use std::process::{Command, Stdio};

use crate::support::{BINARY, Home, await_until};

#[test]
fn a_server_whose_account_is_not_mapped_in_its_user_namespace_refuses_to_start() {
    let home = Home::new();
    let socket = home.path().join("server.sock");
    let mut server = Command::new("unshare")
        .args(["--user", BINARY, "serve"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("QUADRATIQUE_HOME", home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("unshare runs");

    await_until("the server to exit or to bind its socket", || {
        socket.exists() || server.try_wait().is_ok_and(|status| status.is_some())
    });

    let started = socket.exists();
    if started {
        let _ = server.kill();
    }
    let ended = server.wait_with_output().expect("the server's output");
    let stderr = String::from_utf8_lossy(&ended.stderr);
    assert!(!started, "the server started: {stderr}");
    assert!(!ended.status.success(), "{stderr}");
    assert!(stderr.contains("overflow uid"), "{stderr}");
}
