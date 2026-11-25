//! The listener's refusal, with an admission that refuses every connection: no caller check takes part.

use std::time::Duration;

use quadratique_server::server;

const HANG_GUARD: Duration = Duration::from_secs(60);
const LONGER_THAN_THE_TEST: Duration = Duration::from_secs(3600);

#[tokio::test]
async fn a_refused_connection_reads_as_the_owner_only_refusal() {
    use quadratique_server::serve::{self, Connection};
    use quadratique_server::server::Unanswered;

    let refuse_all: serve::Admit = std::sync::Arc::new(|_: Connection| false);
    let served = serve::serve(server::loopback(), Vec::new(), refuse_all)
        .await
        .expect("a listener");

    let probe = server::identity(served.address).await;

    served.close().await;
    assert!(matches!(probe, Err(Unanswered::OwnerOnly)), "{probe:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_beyond_the_draining_bound_closes_the_connection_at_once() {
    use std::io::{Read, Write};

    use quadratique_server::serve::{self, Connection, Refusals};

    let refusals = Refusals {
        draining: 1,
        drain_bytes: u64::MAX,
        drain_for: LONGER_THAN_THE_TEST,
    };
    let refuse_all: serve::Admit = std::sync::Arc::new(|_: Connection| false);
    let served = serve::serve_with(server::loopback(), Vec::new(), refuse_all, refusals)
        .await
        .expect("a listener");
    let address = served.address;
    let refused = |mut client: std::net::TcpStream| {
        client
            .write_all(b"GET /server HTTP/1.1\r\nHost: quadratique\r\n\r\n")
            .expect("the request is sent");
        let mut answer = [0u8; 16];
        let _ = client.read(&mut answer);
        client
    };

    let drained = tokio::task::spawn_blocking(move || {
        let mut drained = refused(std::net::TcpStream::connect(address).expect("a connection"));
        let mut beyond = refused(std::net::TcpStream::connect(address).expect("a connection"));
        let closed_at_once = within_blocking(|| beyond.write_all(b"x").is_err());
        let still_drained = drained.write_all(b"x").is_ok();
        (closed_at_once, still_drained)
    })
    .await
    .expect("the clients");

    served.close().await;
    assert_eq!(drained, (true, true));
}

fn within_blocking(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + HANG_GUARD;
    while std::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
    }
    false
}
