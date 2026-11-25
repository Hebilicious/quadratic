//! A socket that crosses from one account to another, on Linux and macOS. The server's caller check names the account
//! recorded on the caller's socket when it was created (`idiag_uid` on Linux, `so_uid` on macOS), not the account
//! of a process that holds it later. One process creates a TCP socket and passes it, unconnected, to a process of the
//! other account over a Unix socket (`SCM_RIGHTS`); that process connects it to the server and asks for
//! `/server`. These tests act as the account `nobody` through `sudo -n` and are ignored by `test`;
//! `moon run quadratique-server:test-other-accounts` runs them, and so does CI on Linux and macOS.

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use quadratique_server::server::{self, Paths};

/// `use`: waits on a Unix socket for a TCP socket, connects it to the server, asks for /server and prints the status
/// line and the body. `give`: creates a TCP socket and passes it; with `keep` it holds its own copy until the other process is
/// done, with `drop` it closes it at once. The timeouts only stop a hang.
const HELPER: &str = r#"
import socket, sys, os
role, unix_path, host, port, copy = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), sys.argv[5]
if role == "use":
    listener = socket.socket(socket.AF_UNIX)
    listener.settimeout(60)
    listener.bind(unix_path)
    os.chmod(unix_path, 0o777)
    listener.listen(1)
    print("ready", flush=True)
    giver, _ = listener.accept()
    giver.settimeout(60)
    _, descriptors, _, _ = socket.recv_fds(giver, 16, 1)
    tcp = socket.socket(fileno=descriptors[0])
    tcp.settimeout(60)
    tcp.connect((host, port))
    tcp.sendall(b"GET /server HTTP/1.1\r\nHost: %s:%d\r\nConnection: close\r\n\r\n" % (host.encode(), port))
    answer = b""
    while True:
        chunk = tcp.recv(4096)
        if not chunk:
            break
        answer += chunk
    head, _, body = answer.partition(b"\r\n\r\n")
    print(head.split(b"\r\n")[0].decode() + " | " + body.decode(errors="replace"), flush=True)
    giver.sendall(b"done")
else:
    tcp = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    user = socket.socket(socket.AF_UNIX)
    user.settimeout(60)
    user.connect(unix_path)
    socket.send_fds(user, [b"x"], [tcp.fileno()])
    if copy == "drop":
        tcp.close()
    user.recv(16)
"#;

enum Account {
    Owner,
    Nobody,
}

fn python(account: &Account) -> Command {
    match account {
        Account::Owner => Command::new("python3"),
        Account::Nobody => {
            let mut command = Command::new("sudo");
            command.args(["-n", "-u", "nobody", "python3"]);
            command
        }
    }
}

/// The status line and the body that the server gives to a socket created by `creator`, connected and used by `user`.
fn answer_to_a_passed_socket(
    address: std::net::SocketAddr,
    creator: Account,
    copy: &str,
    user: Account,
) -> String {
    let shared = tempfile::Builder::new()
        .prefix("quadratique-passed-socket-")
        .tempdir_in("/tmp")
        .expect("a folder both accounts reach");
    std::fs::set_permissions(shared.path(), std::fs::Permissions::from_mode(0o777))
        .expect("a folder both accounts may write in");
    let script = shared.path().join("helper.py");
    std::fs::write(&script, HELPER).expect("the helper");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644))
        .expect("a helper both accounts read");
    let unix_path = shared.path().join("pass.sock");
    let arguments = |role: &str| -> Vec<String> {
        vec![
            path(&script),
            role.into(),
            path(&unix_path),
            address.ip().to_string(),
            address.port().to_string(),
            copy.into(),
        ]
    };

    let mut using = python(&user)
        .args(arguments("use"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the using process starts");
    let mut lines = BufReader::new(using.stdout.take().expect("its output")).lines();
    let ready = lines.next().and_then(Result::ok);
    assert_eq!(
        ready.as_deref(),
        Some("ready"),
        "{:?}",
        using.wait_with_output()
    );
    let given = python(&creator)
        .args(arguments("give"))
        .output()
        .expect("the giving process runs");
    let status = lines.next().and_then(Result::ok).unwrap_or_default();
    let used = using.wait_with_output().expect("the using process ends");
    assert!(given.status.success(), "{given:?}");
    assert!(used.status.success(), "{used:?}");
    status
}

fn path(path: &Path) -> String {
    path.to_str().expect("a UTF-8 path").to_owned()
}

async fn passed_socket(creator: Account, copy: &'static str, user: Account) -> String {
    let home = tempfile::tempdir().expect("a temporary home");
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let running = server::start(&paths, server::loopback())
        .await
        .expect("the server starts");
    let address = running.address();
    let status = tokio::task::spawn_blocking(move || {
        answer_to_a_passed_socket(address, creator, copy, user)
    })
    .await
    .expect("the helper processes");
    running.shutdown().await;
    status
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sudo to act as the account nobody"]
async fn a_socket_created_by_another_account_and_used_by_the_owner_is_refused() {
    let status = passed_socket(Account::Nobody, "keep", Account::Owner).await;

    println!("created by nobody, kept by nobody, connected and used by the owner: {status}");
    assert_eq!(
        status,
        format!(
            "HTTP/1.1 403 Forbidden | {}",
            quadratique_server::serve::OWNER_ONLY
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs sudo to act as the account nobody"]
async fn a_socket_created_by_the_owner_and_used_by_another_account_is_admitted() {
    let status = passed_socket(Account::Owner, "drop", Account::Nobody).await;

    println!("created by the owner, closed by the owner, connected and used by nobody: {status}");
    assert!(status.starts_with("HTTP/1.1 200 OK | {"), "{status}");
}
