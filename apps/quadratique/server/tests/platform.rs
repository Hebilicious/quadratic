//! Commands and agents reach the server only through its Unix socket, which Windows lacks: there the server refuses to
//! start, and says that QUA-61 brings it.
#![cfg(windows)]

use quadratique_server::server::{self, Paths, ServerError};

#[tokio::test]
async fn the_server_refuses_to_start_on_windows_and_names_the_issue_that_brings_it() {
    let parent = tempfile::tempdir().expect("a temporary folder");
    let home = parent.path().join("home");
    let paths = Paths::in_home(&home).expect("an absolute home");

    let refused = server::start(&paths, server::loopback()).await.err();

    let Some(error @ ServerError::Unsupported) = refused else {
        panic!("the server did not refuse to start: {refused:?}");
    };
    assert!(error.to_string().contains("QUA-61"), "{error}");
    assert!(!home.exists(), "a refused server made its data folder");
}
