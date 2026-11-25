//! Where the server cannot tell which account connects to its local listener, it refuses to start instead of serving
//! every account of the machine: Windows until QUA-34. On Linux and macOS, tests/server/process.rs shows the check
//! itself, and tests/callers_macos.rs the lookup on macOS.
#![cfg(not(any(target_os = "linux", target_os = "macos")))]

use quadratique_server::server::{self, Paths, ServerError};

#[tokio::test]
async fn the_server_refuses_to_start_where_it_cannot_tell_who_connects() {
    let home = tempfile::tempdir().expect("a temporary home");
    let paths = Paths::in_home(home.path()).expect("an absolute home");

    let refused = server::start(&paths, server::loopback()).await.err();

    let Some(ServerError::CallersUnknown(message)) = refused else {
        panic!("the server did not refuse to start: {refused:?}");
    };
    assert!(
        message.contains("cannot yet tell which account connects"),
        "{message}"
    );
    assert!(
        !paths.runtime.exists(),
        "a refused server published its runtime file"
    );
}
