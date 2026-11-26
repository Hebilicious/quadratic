//! Pairing through the real server: `ui`'s request on the socket, `POST /session` and the bearer on TCP, the token
//! file across restarts, the sign-out and strict mode. docs/decisions/0004-a-socket-for-commands-and-a-token-for-the-browser.md, §4.

use std::path::Path;

use quadratique_server::git::watcher;
use quadratique_server::home::{self, Prepared};
use quadratique_server::pairing::Issued;
use quadratique_server::server::{self, Asked, Paths, Server, Waits};
use quadratique_server::web::WebApp;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::support::{private_home, within};
use crate::web::{Answer, Site, built, request, start};

fn prepared(home: &Path) -> Prepared {
    home::prepare(&Paths::in_home(home).expect("an absolute home")).expect("a private home")
}

async fn pair(home: &Path, open: bool) -> Issued {
    match server::pair_over_socket(&prepared(home), open).await {
        Ok(Asked::Answered(issued)) => issued,
        other => panic!("no link: {other:?}"),
    }
}

fn code_of(issued: &Issued) -> String {
    issued
        .link
        .split_once("#pair=")
        .expect("a code in the link")
        .1
        .to_owned()
}

async fn redeem(site_address: std::net::SocketAddr, code: &str, origin: Option<&str>) -> Answer {
    let host = site_address.to_string();
    let own = format!("http://{host}");
    let mut headers = vec![("Host", host.as_str())];
    if let Some(origin) = origin {
        headers.push(("Origin", origin));
    } else {
        headers.push(("Origin", own.as_str()));
    }
    request(
        site_address,
        "POST",
        "/session",
        &headers,
        Some(json!({ "code": code })),
    )
    .await
}

fn token_of(answer: &Answer) -> String {
    serde_json::from_str::<Value>(&answer.body).expect("a JSON answer")["token"]
        .as_str()
        .expect("a token")
        .to_owned()
}

/// `project.list` on `route` with the bearer `token`, or with none.
async fn with_bearer(site: &Site, route: &str, token: Option<&str>) -> u16 {
    let bearer = token.map(|token| format!("Bearer {token}"));
    let extra: Vec<(&str, &str)> = bearer
        .iter()
        .map(|value| ("Authorization", value.as_str()))
        .collect();
    let id = if route == "/web" {
        "project.list"
    } else {
        "issue.list"
    };
    site.call(route, id, json!({}), &extra).await.status
}

async fn stream_with_bearer(site: &Site, token: &str) -> Answer {
    let host = site.address().to_string();
    let origin = site.origin();
    let bearer = format!("Bearer {token}");
    request(
        site.address(),
        "POST",
        "/web/events/issue.changed",
        &[
            ("Host", host.as_str()),
            ("Origin", origin.as_str()),
            ("Authorization", bearer.as_str()),
        ],
        Some(json!({})),
    )
    .await
}

/// An event stream of `issue.changed` on the web target, read as it comes.
struct EventStream {
    connection: TcpStream,
    received: String,
    read_up_to: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum Next {
    Event,
    End,
}

impl EventStream {
    /// Opens the stream with the bearer `token`, or with none, and reads its head: once the head is in, the server has
    /// registered the subscription. The status of a refused stream is the error.
    async fn open(site: &Site, token: Option<&str>) -> Result<EventStream, u16> {
        within("the event stream's head", async {
            let host = site.address().to_string();
            let mut text = format!(
                "POST /web/events/issue.changed HTTP/1.1\r\nConnection: close\r\nHost: {host}\r\nOrigin: {}\r\n",
                site.origin()
            );
            if let Some(token) = token {
                text.push_str(&format!("Authorization: Bearer {token}\r\n"));
            }
            text.push_str("Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{}");
            let mut connection = TcpStream::connect(site.address())
                .await
                .expect("a connection");
            connection
                .write_all(text.as_bytes())
                .await
                .expect("the request is sent");
            let mut stream = EventStream {
                connection,
                received: String::new(),
                read_up_to: 0,
            };
            while !stream.received.contains("\r\n\r\n") {
                assert!(stream.read().await, "the server closed before the head");
            }
            let status: u16 = stream
                .received
                .split_whitespace()
                .nth(1)
                .and_then(|status| status.parse().ok())
                .expect("a status");
            stream.read_up_to = stream.received.find("\r\n\r\n").expect("the head's end");
            if status == 200 { Ok(stream) } else { Err(status) }
        })
        .await
    }

    /// The next event, or the end of the stream, whichever comes first.
    async fn next(&mut self) -> Next {
        within("the next event or the end of the stream", async {
            loop {
                if let Some(found) = self.received[self.read_up_to..].find("event: next") {
                    self.read_up_to += found + "event: next".len();
                    return Next::Event;
                }
                if !self.read().await {
                    return Next::End;
                }
            }
        })
        .await
    }

    /// Whether more bytes came before the server closed the connection.
    async fn read(&mut self) -> bool {
        let mut buffer = [0u8; 4096];
        let count = self
            .connection
            .read(&mut buffer)
            .await
            .expect("the stream is read");
        self.received
            .push_str(&String::from_utf8_lossy(&buffer[..count]));
        count > 0
    }
}

async fn create_issue(site: &Site, title: &str) {
    let created = site
        .call(
            "/web",
            "issue.create",
            json!({ "project": "DEM", "title": title, "acceptance": "It is done." }),
            &[],
        )
        .await;
    assert_eq!(created.status, 200, "{}", created.body);
    assert!(created.body.contains(r#""ok":true"#), "{}", created.body);
}

async fn create_project(site: &Site) {
    let created = site
        .call(
            "/web",
            "project.create",
            json!({ "key": "DEM", "name": "Demo" }),
            &[],
        )
        .await;
    assert_eq!(created.status, 200, "{}", created.body);
    assert!(created.body.contains(r#""ok":true"#), "{}", created.body);
}

async fn sign_out(site: &Site) {
    let signed_out = server::sign_out_over_socket(&prepared(site.home.path())).await;
    assert!(
        matches!(signed_out, Ok(Asked::Answered(ref answer)) if answer.signed_out),
        "{signed_out:?}"
    );
}

async fn bearer_of(site: &Site) -> String {
    let issued = pair(site.home.path(), false).await;
    token_of(&redeem(site.address(), &code_of(&issued), None).await)
}

async fn start_in(home: &Path, app: &Path) -> Server {
    within(
        "the server's start",
        server::start_with(
            &Paths::in_home(home).expect("an absolute home"),
            server::loopback(),
            WebApp::Directory(app.to_path_buf()),
            watcher::EVERY,
            Waits::default(),
        ),
    )
    .await
    .expect("the server starts")
}

struct Restartable {
    home: TempDir,
    app: TempDir,
}

impl Restartable {
    fn new() -> Restartable {
        let app = tempfile::tempdir().expect("a folder for the built app");
        built(app.path());
        Restartable {
            home: private_home(),
            app,
        }
    }

    async fn start(&self) -> Server {
        start_in(self.home.path(), self.app.path()).await
    }

    /// The bearer that a code of the running server redeems for.
    async fn bearer(&self, server: &Server) -> String {
        let issued = pair(self.home.path(), false).await;
        token_of(&redeem(server.address(), &code_of(&issued), None).await)
    }
}

async fn admitted(server: &Server, token: &str) -> u16 {
    let host = server.address().to_string();
    let origin = format!("http://{host}");
    let bearer = format!("Bearer {token}");
    request(
        server.address(),
        "POST",
        "/web/capabilities/project.list",
        &[
            ("Host", host.as_str()),
            ("Origin", origin.as_str()),
            ("Authorization", bearer.as_str()),
        ],
        Some(json!({})),
    )
    .await
    .status
}

#[tokio::test(flavor = "multi_thread")]
async fn ui_gets_the_servers_own_address_with_a_code_that_redeems_once_for_the_bearer() {
    let site = start(built).await;

    let issued = pair(site.home.path(), false).await;
    let first = redeem(site.address(), &code_of(&issued), None).await;
    let again = redeem(site.address(), &code_of(&issued), None).await;

    assert_eq!(issued.url, format!("http://{}/", site.address()));
    assert_eq!(issued.file, None);
    assert_eq!(first.status, 200, "{}", first.body);
    assert_eq!(first.header("Cache-Control"), Some("no-store"));
    assert!(
        !first.head.to_ascii_lowercase().contains("access-control-"),
        "{}",
        first.head
    );
    assert_eq!(again.status, 403, "{}", again.body);
    let token = token_of(&first);
    for route in ["/web", "/webmcp"] {
        assert_eq!(
            with_bearer(&site, route, Some(&token)).await,
            200,
            "{route}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bearer_that_is_not_the_current_one_gets_401_on_both_targets_and_the_event_stream_and_none_is_still_served()
 {
    let site = start(built).await;
    let wrong = "A".repeat(43);

    for route in ["/web", "/webmcp"] {
        assert_eq!(
            with_bearer(&site, route, Some(&wrong)).await,
            401,
            "{route}"
        );
        assert_eq!(
            with_bearer(&site, route, Some("short")).await,
            401,
            "{route}"
        );
        assert_eq!(with_bearer(&site, route, None).await, 200, "{route}");
    }
    let stream = stream_with_bearer(&site, &wrong).await;
    assert_eq!(stream.status, 401, "{}", stream.body);
    assert_eq!(stream.header("WWW-Authenticate"), Some("Bearer"));
    assert!(
        stream.body.contains("quadratique ui --open"),
        "{}",
        stream.body
    );
    assert!(!stream.body.contains(&wrong), "{}", stream.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn post_session_requires_the_exact_origin_and_host_and_a_small_body() {
    let site = start(built).await;
    let issued = pair(site.home.path(), false).await;
    let code = code_of(&issued);
    let host = site.address().to_string();
    let localhost = format!("localhost:{}", site.address().port());
    let localhost_origin = format!("http://{localhost}");

    let foreign = redeem(site.address(), &code, Some("http://evil.example")).await;
    let named = request(
        site.address(),
        "POST",
        "/session",
        &[
            ("Host", localhost.as_str()),
            ("Origin", localhost_origin.as_str()),
        ],
        Some(json!({ "code": code })),
    )
    .await;
    let without_origin = request(
        site.address(),
        "POST",
        "/session",
        &[("Host", host.as_str())],
        Some(json!({ "code": code })),
    )
    .await;
    let origin = site.origin();
    let large = request(
        site.address(),
        "POST",
        "/session",
        &[("Host", host.as_str()), ("Origin", origin.as_str())],
        Some(json!({ "code": code, "padding": "x".repeat(2048) })),
    )
    .await;
    let read = request(
        site.address(),
        "GET",
        "/session",
        &[("Host", host.as_str()), ("Origin", origin.as_str())],
        None,
    )
    .await;

    assert_eq!(foreign.status, 403, "{}", foreign.body);
    assert!(
        !foreign
            .head
            .to_ascii_lowercase()
            .contains("access-control-"),
        "{}",
        foreign.head
    );
    assert_eq!(named.status, 403, "{}", named.body);
    assert_eq!(without_origin.status, 403, "{}", without_origin.body);
    assert_eq!(large.status, 400, "{}", large.body);
    assert_eq!(read.status, 405, "{}", read.body);
    let redeemed = redeem(site.address(), &code, None).await;
    assert_eq!(
        redeemed.status, 200,
        "the refused requests used the code: {}",
        redeemed.body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bootstrap_file_is_private_names_the_link_and_is_gone_after_use_and_after_a_restart() {
    use std::os::unix::fs::PermissionsExt;

    let site = Restartable::new();
    let server = site.start().await;
    let issued = pair(site.home.path(), true).await;
    let file = issued.file.clone().expect("a bootstrap file");

    let mode = std::fs::metadata(&file)
        .expect("the file")
        .permissions()
        .mode();
    let page = std::fs::read_to_string(&file).expect("the page");
    assert_eq!(file.parent(), Some(prepared(site.home.path()).folder()));
    assert_eq!(mode & 0o777, 0o600);
    assert!(page.contains(&issued.link), "{page}");
    assert_eq!(
        redeem(server.address(), &code_of(&issued), None)
            .await
            .status,
        200
    );
    assert!(!file.exists(), "the used code's bootstrap file stays");

    let left = pair(site.home.path(), true)
        .await
        .file
        .expect("another bootstrap file");
    let crashed = left.with_file_name("open-left-by-a-crash.html");
    std::fs::copy(&left, &crashed).expect("a file as a crash leaves it");
    server.shutdown().await;
    assert!(!left.exists(), "a stopped server left its bootstrap file");
    let restarted = site.start().await;
    assert!(!crashed.exists(), "a restart left a bootstrap file");
    restarted.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bearer_survives_a_restart_and_a_sign_out_replaces_it() {
    let site = Restartable::new();
    let first = site.start().await;
    let bearer = site.bearer(&first).await;
    first.shutdown().await;
    let token_file = prepared(site.home.path()).folder().join("web-token");
    assert!(token_file.is_file());
    let on_disk = std::fs::read_to_string(&token_file).expect("the token file");

    let restarted = site.start().await;
    let after_restart = admitted(&restarted, &bearer).await;
    let signed_out = server::sign_out_over_socket(&prepared(site.home.path())).await;
    let after_sign_out = admitted(&restarted, &bearer).await;
    let renewed = site.bearer(&restarted).await;
    let renewed_admitted = admitted(&restarted, &renewed).await;
    restarted.shutdown().await;

    assert_eq!(on_disk.trim(), bearer);
    assert_eq!(after_restart, 200);
    assert!(
        matches!(signed_out, Ok(Asked::Answered(ref answer)) if answer.signed_out && answer.lasting),
        "{signed_out:?}"
    );
    assert_eq!(after_sign_out, 401);
    assert_ne!(renewed, bearer);
    assert_eq!(renewed_admitted, 200);
    let again = site.start().await;
    let kept = admitted(&again, &renewed).await;
    again.shutdown().await;
    assert_eq!(kept, 200, "the new bearer did not survive a restart");
}

#[tokio::test(flavor = "multi_thread")]
async fn in_strict_mode_the_bearer_changes_at_each_start_and_no_token_file_is_kept() {
    let site = Restartable::new();
    server::set_strict(&prepared(site.home.path()), true).expect("strict on");
    let first = site.start().await;
    let bearer = site.bearer(&first).await;
    first.shutdown().await;

    let restarted = site.start().await;
    let after_restart = admitted(&restarted, &bearer).await;
    let renewed = site.bearer(&restarted).await;
    let renewed_admitted = admitted(&restarted, &renewed).await;
    restarted.shutdown().await;

    assert_eq!(after_restart, 401);
    assert_eq!(renewed_admitted, 200);
    assert!(
        !prepared(site.home.path())
            .folder()
            .join("web-token")
            .exists()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_refuses_a_web_app_inside_its_data_folder() {
    let home = private_home();
    let app = home.path().join("dist");
    std::fs::create_dir(&app).expect("a folder for the app");
    built(&app);

    let refused = server::start_with(
        &Paths::in_home(home.path()).expect("an absolute home"),
        server::loopback(),
        WebApp::Directory(app),
        watcher::EVERY,
        Waits::default(),
    )
    .await;

    assert!(
        matches!(refused, Err(server::ServerError::WebApp(_))),
        "{:?}",
        refused.err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_exact_path_redeems_a_code_and_only_with_one_origin_header() {
    let site = start(built).await;
    let code = code_of(&pair(site.home.path(), false).await);
    let host = site.address().to_string();
    let origin = site.origin();
    let body = Some(json!({ "code": code }));

    let below = request(
        site.address(),
        "POST",
        "/session/x",
        &[("Host", host.as_str()), ("Origin", origin.as_str())],
        body.clone(),
    )
    .await;
    let twice = request(
        site.address(),
        "POST",
        "/session",
        &[
            ("Host", host.as_str()),
            ("Origin", origin.as_str()),
            ("Origin", origin.as_str()),
        ],
        body,
    )
    .await;

    assert_eq!(below.status, 404, "POST /session/x");
    assert_eq!(twice.status, 403, "{}", twice.body);
    assert_eq!(redeem(site.address(), &code, None).await.status, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_out_ends_both_event_streams_of_the_earlier_token_and_neither_delivers_an_event_after_it()
 {
    let site = start(built).await;
    create_project(&site).await;
    let earlier = bearer_of(&site).await;
    let mut first = EventStream::open(&site, Some(&earlier))
        .await
        .expect("a stream of the token");
    let mut second = EventStream::open(&site, Some(&earlier))
        .await
        .expect("another stream of the token");
    create_issue(&site, "Before the sign-out").await;
    assert_eq!(
        first.next().await,
        Next::Event,
        "the first stream is not live"
    );
    assert_eq!(
        second.next().await,
        Next::Event,
        "the second stream is not live"
    );

    sign_out(&site).await;
    create_issue(&site, "After the sign-out").await;

    assert_eq!(
        first.next().await,
        Next::End,
        "the first stream delivered an event after the sign-out"
    );
    assert_eq!(
        second.next().await,
        Next::End,
        "the second stream delivered an event after the sign-out"
    );
    assert_eq!(
        EventStream::open(&site, Some(&earlier)).await.err(),
        Some(401),
        "the stream reopened with the earlier token was served"
    );
}

/// Until the cutover (QUA-58) a stream without a token is served, and no token is replaced for it: a sign-out leaves
/// it running. After the cutover no such stream is admitted.
#[tokio::test(flavor = "multi_thread")]
async fn after_a_sign_out_a_stream_of_the_new_token_and_a_stream_without_a_token_keep_delivering() {
    let site = start(built).await;
    create_project(&site).await;
    let mut without_token = EventStream::open(&site, None)
        .await
        .expect("a stream without a token");

    sign_out(&site).await;
    let renewed = bearer_of(&site).await;
    let mut of_the_new_token = EventStream::open(&site, Some(&renewed))
        .await
        .expect("a stream of the new token");
    create_issue(&site, "After the sign-out").await;

    assert_eq!(of_the_new_token.next().await, Next::Event);
    assert_eq!(without_token.next().await, Next::Event);
}
