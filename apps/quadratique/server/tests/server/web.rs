use std::net::SocketAddr;
use std::path::Path;

use std::sync::Mutex;

use quadratique_server::app::CATALOG_JSON;
use quadratique_server::git::watcher;
use quadratique_server::home::{self, Prepared};
use quadratique_server::pairing::Issued;
use quadratique_server::server::{self, Asked, Paths, Server, Waits};
use quadratique_server::web::{EmbeddedFile, WEB_TARGET, WEBMCP_TARGET, WebApp};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::support::within;

pub struct Site {
    _server: Server,
    pub home: TempDir,
    address: SocketAddr,
    bearer: Mutex<String>,
    _app: TempDir,
}

impl Site {
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn origin(&self) -> String {
        format!("http://{}", self.address())
    }

    /// The bearer the site's page holds.
    pub fn bearer(&self) -> String {
        self.bearer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Lets the page in again, as a new link of `quadratique ui` does, and gives the bearer it now holds.
    pub async fn pair(&self) -> String {
        let (_, bearer) = paired(self.home.path()).await;
        *self
            .bearer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = bearer.clone();
        bearer
    }
}

/// A server of the app `build` writes, and a page let in as `quadratique ui` lets one in.
pub async fn start(build: impl FnOnce(&Path)) -> Site {
    let app = tempfile::tempdir().expect("a folder for the built app");
    build(app.path());
    let web_app = WebApp::Directory(app.path().to_path_buf());
    start_app(web_app, app).await
}

async fn start_app(web_app: WebApp, app: TempDir) -> Site {
    let home = crate::support::private_home();
    let server = within(
        "the server's start",
        server::start_with(
            &Paths::in_home(home.path()).expect("an absolute home"),
            server::loopback(),
            web_app,
            watcher::EVERY,
            Waits::default(),
        ),
    )
    .await
    .expect("the server starts");
    let (address, bearer) = paired(home.path()).await;
    Site {
        _server: server,
        home,
        address,
        bearer: Mutex::new(bearer),
        _app: app,
    }
}

pub fn prepared(home: &Path) -> Prepared {
    home::prepare(&Paths::in_home(home).expect("an absolute home")).expect("a private home")
}

/// What `quadratique ui` gets from the server of `home`: the server opens the web app's listener first.
pub async fn pair(home: &Path, open: bool) -> Issued {
    match server::pair_over_socket(&prepared(home), open).await {
        Ok(Asked::Answered(issued)) => issued,
        other => panic!("no link: {other:?}"),
    }
}

pub fn code_of(issued: &Issued) -> String {
    issued
        .link
        .split_once("#pair=")
        .expect("a code in the link")
        .1
        .to_owned()
}

/// `POST /session` as the page sends it, from its own origin unless `origin` names another.
pub async fn redeem(site_address: SocketAddr, code: &str, origin: Option<&str>) -> Answer {
    let host = site_address.to_string();
    let own = format!("http://{host}");
    let headers = [("Host", host.as_str()), ("Origin", origin.unwrap_or(&own))];
    request(
        site_address,
        "POST",
        "/session",
        &headers,
        Some(json!({ "code": code })),
    )
    .await
}

pub fn token_of(answer: &Answer) -> String {
    serde_json::from_str::<Value>(&answer.body).expect("a JSON answer")["token"]
        .as_str()
        .expect("a token")
        .to_owned()
}

/// A link of `quadratique ui`, redeemed as the page does: the web app's address and the bearer.
pub async fn paired(home: &Path) -> (SocketAddr, String) {
    let issued = pair(home, false).await;
    let address: SocketAddr = issued
        .url
        .strip_prefix("http://")
        .and_then(|url| url.strip_suffix('/'))
        .and_then(|authority| authority.parse().ok())
        .expect("the web app's address");
    let redeemed = redeem(address, &code_of(&issued), None).await;
    assert_eq!(redeemed.status, 200, "{}", redeemed.body);
    (address, token_of(&redeemed))
}

pub fn built(directory: &Path) {
    std::fs::write(
        directory.join("index.html"),
        "<!doctype html><title>Quadratique</title>",
    )
    .expect("an index page");
    std::fs::create_dir(directory.join("assets")).expect("an assets folder");
    std::fs::write(directory.join("assets/app-1a2b.js"), "export {};").expect("a script");
}

pub struct Answer {
    pub status: u16,
    pub head: String,
    pub body: String,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }
}

pub async fn request(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Answer {
    within("a request to the server", async {
        let mut stream = TcpStream::connect(address).await.expect("a connection");
        let body = body.map(|body| body.to_string()).unwrap_or_default();
        let mut text = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n");
        for (name, value) in headers {
            text.push_str(&format!("{name}: {value}\r\n"));
        }
        if !body.is_empty() {
            text.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            ));
        }
        text.push_str("\r\n");
        text.push_str(&body);
        stream
            .write_all(text.as_bytes())
            .await
            .expect("the request is sent");
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).await.expect("an answer");
        let answer = String::from_utf8(answer).expect("a UTF-8 answer");
        let (head, body) = answer.split_once("\r\n\r\n").expect("a head and a body");
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|status| status.parse().ok())
            .expect("a status");
        Answer {
            status,
            head: head.to_owned(),
            body: unchunked(head, body),
        }
    })
    .await
}

fn unchunked(head: &str, body: &str) -> String {
    let chunked = head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    if !chunked {
        return body.to_owned();
    }
    let mut rest = body;
    let mut text = String::new();
    while let Some((size, after)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        text.push_str(&after[..size]);
        rest = &after[size + 2..];
    }
    text
}

impl Site {
    /// A call of the site's page, with its bearer unless `extra` names an `Authorization`.
    pub async fn call(
        &self,
        route: &str,
        id: &str,
        input: Value,
        extra: &[(&str, &str)],
    ) -> Answer {
        let bearer = format!("Bearer {}", self.bearer());
        let mut headers = extra.to_vec();
        if !extra.iter().any(|(name, _)| *name == "Authorization") {
            headers.push(("Authorization", bearer.as_str()));
        }
        self.call_bare(route, id, input, &headers).await
    }

    /// A call of the site's page with only the headers `extra` adds to its `Host` and `Origin`.
    pub async fn call_bare(
        &self,
        route: &str,
        id: &str,
        input: Value,
        extra: &[(&str, &str)],
    ) -> Answer {
        let host = self.address().to_string();
        let origin = self.origin();
        let mut headers = vec![("Host", host.as_str())];
        if !extra.iter().any(|(name, _)| *name == "Origin") {
            headers.push(("Origin", origin.as_str()));
        }
        headers.extend_from_slice(extra);
        request(
            self.address(),
            "POST",
            &format!("{route}/capabilities/{id}"),
            &headers,
            Some(input),
        )
        .await
    }

    async fn page(&self, path: &str) -> Answer {
        let host = self.address().to_string();
        request(
            self.address(),
            "GET",
            path,
            &[("Host", host.as_str())],
            None,
        )
        .await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_page_of_another_origin_or_another_host_name_is_refused() {
    let site = start(built).await;
    let own_host = site.address().to_string();
    let foreign_host = format!("rebound.example:{}", site.address().port());
    let evil = "http://evil.example";

    for route in ["/web", "/webmcp"] {
        let foreign_origin = site
            .call(route, "issue.list", json!({}), &[("Origin", evil)])
            .await;
        assert_eq!(foreign_origin.status, 403, "{route} with Origin {evil}");

        let preflight = request(
            site.address(),
            "OPTIONS",
            &format!("{route}/capabilities/issue.create"),
            &[
                ("Host", own_host.as_str()),
                ("Origin", evil),
                ("Access-Control-Request-Method", "POST"),
                ("Access-Control-Request-Headers", "content-type"),
            ],
            None,
        )
        .await;
        assert_eq!(preflight.status, 403, "{route} preflight");
        assert_eq!(preflight.header("Access-Control-Allow-Origin"), None);
    }

    let rebound = request(
        site.address(),
        "GET",
        "/",
        &[("Host", foreign_host.as_str())],
        None,
    )
    .await;
    assert_eq!(rebound.status, 403, "a page under another host name");

    let same_origin = site.call("/web", "project.list", json!({}), &[]).await;
    assert_eq!(same_origin.status, 200, "the server's own page");

    let localhost = format!("localhost:{}", site.address().port());
    let localhost_origin = format!("http://{localhost}");
    let crossed = site
        .call(
            "/web",
            "project.list",
            json!({}),
            &[("Origin", localhost_origin.as_str())],
        )
        .await;
    assert_eq!(
        crossed.status, 403,
        "a page of {localhost_origin} calling Host {own_host}"
    );
    let named = request(
        site.address(),
        "POST",
        "/web/capabilities/project.list",
        &[
            ("Host", localhost.as_str()),
            ("Origin", localhost_origin.as_str()),
        ],
        Some(json!({})),
    )
    .await;
    assert_eq!(
        named.status, 403,
        "the server's own page under {localhost}, which may resolve to [::1]"
    );
}

/// The command line's routes and the control requests answer only on the socket: on TCP every method gets `404`, never
/// the app's shell, from a page of the server's own origin or from any other client.
#[tokio::test(flavor = "multi_thread")]
async fn the_command_line_and_control_routes_are_not_found_on_tcp_not_even_as_the_shell() {
    let site = start(built).await;
    let host = site.address().to_string();
    let origin = site.origin();
    let bearer = format!("Bearer {}", site.bearer());

    for path in [
        "/cli",
        "/cli/x",
        "/cli/capabilities/project.list",
        "/server",
        "/server/x",
        "/server/stop",
        "/server/pair",
    ] {
        for method in ["GET", "HEAD", "POST"] {
            for headers in [
                vec![("Host", host.as_str())],
                vec![
                    ("Host", host.as_str()),
                    ("Origin", origin.as_str()),
                    ("Authorization", bearer.as_str()),
                ],
            ] {
                let body = (method == "POST").then(|| json!({}));
                let answer = request(site.address(), method, path, &headers, body).await;
                assert_eq!(answer.status, 404, "{method} {path} {headers:?}");
                assert!(
                    !answer.body.contains("<title>"),
                    "{method} {path}: the shell"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_built_app_is_served_and_a_client_side_route_falls_back_to_its_index() {
    let site = start(built).await;

    let index = site.page("/").await;
    assert_eq!(index.status, 200);
    assert!(index.body.contains("<title>Quadratique</title>"));
    assert_eq!(
        index.header("Content-Type"),
        Some("text/html; charset=utf-8")
    );

    let deep = site.page("/projects/WEB").await;
    assert_eq!(
        (deep.status, deep.body.as_str()),
        (200, index.body.as_str())
    );

    let script = site.page("/assets/app-1a2b.js").await;
    assert_eq!((script.status, script.body.as_str()), (200, "export {};"));
    assert_eq!(
        script.header("Content-Type"),
        Some("text/javascript; charset=utf-8")
    );

    assert_eq!(site.page("/assets/missing.js").await.status, 404);
    for elsewhere in ["/elsewhere", "/projects/WEB/other", "/webx", "/now/x"] {
        assert_eq!(site.page(elsewhere).await.status, 404, "{elsewhere}");
    }
    assert_eq!(site.page("/../Cargo.toml").await.status, 404);
}

static EMBEDDED_APP: &[EmbeddedFile] = &[
    EmbeddedFile {
        relative_path: "index.html",
        media_type: "text/html; charset=utf-8",
        bytes: b"<!doctype html><title>Quadratique</title>",
    },
    EmbeddedFile {
        relative_path: "assets/app-1a2b.js",
        media_type: "text/javascript; charset=utf-8",
        bytes: b"export {};",
    },
];

#[tokio::test(flavor = "multi_thread")]
async fn an_asset_built_into_the_binary_is_served_under_a_path_with_a_doubled_separator() {
    let empty = tempfile::tempdir().expect("an unused folder");
    let site = start_app(WebApp::Embedded(EMBEDDED_APP), empty).await;

    for path in ["/assets/app-1a2b.js", "/assets//app-1a2b.js"] {
        let script = site.page(path).await;
        assert_eq!(
            (script.status, script.body.as_str()),
            (200, "export {};"),
            "{path}"
        );
        assert_eq!(
            script.header("Content-Type"),
            Some("text/javascript; charset=utf-8"),
            "{path}"
        );
    }
}

static EMBEDDED_ALIAS: &[EmbeddedFile] = &[
    EmbeddedFile {
        relative_path: "index.html",
        media_type: "text/html; charset=utf-8",
        bytes: b"<!doctype html><title>Quadratique</title>",
    },
    EmbeddedFile {
        relative_path: "alias.js",
        media_type: "text/html; charset=utf-8",
        bytes: b"<!doctype html><title>Quadratique</title>",
    },
];

#[tokio::test(flavor = "multi_thread")]
async fn a_link_built_into_the_binary_is_served_with_the_type_of_the_file_it_names() {
    let empty = tempfile::tempdir().expect("an unused folder");
    let site = start_app(WebApp::Embedded(EMBEDDED_ALIAS), empty).await;

    let page = site.page("/alias.js").await;
    assert_eq!(page.status, 200);
    assert_eq!(
        page.header("Content-Type"),
        Some("text/html; charset=utf-8")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_inside_the_built_app_is_served_with_the_type_of_the_file_it_names() {
    let site = start(|directory| {
        built(directory);
        std::os::unix::fs::symlink(directory.join("index.html"), directory.join("alias.js"))
            .expect("a link to the index page");
    })
    .await;

    let page = site.page("/alias.js").await;
    assert_eq!(page.status, 200);
    assert_eq!(
        page.header("Content-Type"),
        Some("text/html; charset=utf-8")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_built_app_the_page_says_how_to_build_it() {
    let site = start(|_| {}).await;
    let page = site.page("/").await;
    assert_eq!(page.status, 503);
    assert!(
        page.body.contains("moon run quadratique-web:build"),
        "{}",
        page.body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_in_the_built_app_to_a_file_outside_it_is_not_served() {
    let outside = tempfile::tempdir().expect("a folder outside the app");
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "a secret outside the app").expect("a file outside the app");
    let site = start(|directory| {
        std::fs::create_dir(directory.join("assets")).expect("an assets folder");
        std::os::unix::fs::symlink(&secret, directory.join("assets/secret.js"))
            .expect("a link to an asset outside the app");
        std::os::unix::fs::symlink(&secret, directory.join("index.html"))
            .expect("a link to an index outside the app");
    })
    .await;

    for path in ["/assets/secret.js", "/"] {
        let page = site.page(path).await;
        assert_ne!(page.status, 200, "{path}");
        assert!(!page.body.contains("a secret"), "{path}: {}", page.body);
    }
}

#[test]
fn every_target_the_server_serves_over_http_declares_the_http_surface() {
    let catalog: Value = serde_json::from_str(CATALOG_JSON).expect("the catalog is JSON");
    for target in [server::TARGET, WEB_TARGET, WEBMCP_TARGET] {
        let surfaces = &catalog["targets"][target]["surfaces"];
        assert!(
            surfaces
                .as_array()
                .is_some_and(|surfaces| surfaces.contains(&json!("http"))),
            "{target}: {surfaces}"
        );
    }
}

/// One row of the route table: the method, the path, the headers beside `Host`, and the status under the own address.
type Row<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], u16);

/// The route table of record 0004, §4, row by row: every route refuses a `Host` that is not the server's numeric
/// address, `localhost` included, which may resolve to `[::1]`; under its own address each row answers as the table says.
#[tokio::test(flavor = "multi_thread")]
async fn every_route_answers_as_the_route_table_says_and_only_under_the_servers_own_address() {
    let site = start(built).await;
    let port = site.address().port();
    let own = site.address().to_string();
    let origin = site.origin();
    let bearer = format!("Bearer {}", site.bearer());
    let foreign_host = format!("rebound.example:{port}");
    let localhost = format!("localhost:{port}");
    let page = [
        ("Origin", origin.as_str()),
        ("Authorization", bearer.as_str()),
    ];
    let routes: [Row; 11] = [
        ("GET", "/", &[], 200),
        ("HEAD", "/issues/DEM-1", &[], 200),
        ("GET", "/assets/app-1a2b.js", &[], 200),
        ("POST", "/now", &[], 405),
        ("GET", "/elsewhere", &[], 404),
        ("POST", "/elsewhere", &[], 404),
        ("POST", "/cli/capabilities/project.list", &page, 404),
        ("GET", "/server", &[], 404),
        ("POST", "/web/capabilities/project.list", &page, 200),
        ("POST", "/webmcp/capabilities/issue.list", &page, 200),
        (
            "POST",
            "/web/capabilities/project.list",
            &[("Origin", origin.as_str())],
            401,
        ),
    ];

    for (method, path, extra, status) in routes {
        let body = (method == "POST").then(|| json!({}));
        for host in [foreign_host.as_str(), localhost.as_str()] {
            let mut headers = vec![("Host", host)];
            headers.extend_from_slice(extra);
            let refused = request(site.address(), method, path, &headers, body.clone()).await;
            assert_eq!(refused.status, 403, "{method} {path} under {host}");
        }
        let mut headers = vec![("Host", own.as_str())];
        headers.extend_from_slice(extra);
        let answer = request(site.address(), method, path, &headers, body).await;
        assert_eq!(
            answer.status, status,
            "{method} {path} under {own}: {}",
            answer.body
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn no_page_reaches_the_token_capabilities() {
    let site = start(built).await;

    for route in ["/web", "/webmcp"] {
        for id in ["token.create", "token.list", "token.revoke"] {
            let answer = site.call(route, id, json!({}), &[]).await;
            assert_eq!(answer.status, 404, "{route} {id}: {}", answer.body);
        }
    }
}

/// The bearer guards `/web`, `/webmcp` and their event streams, now that no command-line route answers on TCP: a
/// request without it gets `401`, and the same request with it `200`.
#[tokio::test(flavor = "multi_thread")]
async fn the_bearer_is_required_only_once_no_command_line_route_answers_on_tcp() {
    let site = start(built).await;
    let host = site.address().to_string();
    let command_line = request(
        site.address(),
        "POST",
        "/cli/capabilities/project.list",
        &[("Host", host.as_str())],
        Some(json!({})),
    )
    .await;
    let identity = request(
        site.address(),
        "GET",
        "/server",
        &[("Host", host.as_str())],
        None,
    )
    .await;
    let mut answers = Vec::new();
    for (route, id) in [("/web", "project.list"), ("/webmcp", "issue.list")] {
        let without = site.call_bare(route, id, json!({}), &[]).await;
        let with = site.call(route, id, json!({}), &[]).await;
        answers.push((route, without.status, with.status));
    }
    let stream_without = crate::pairing::EventStream::open(&site, None).await.err();
    let stream_with = crate::pairing::EventStream::open(&site, Some(&site.bearer()))
        .await
        .err();

    assert_eq!((command_line.status, identity.status), (404, 404));
    assert_eq!(
        answers,
        vec![("/web", 401, 200), ("/webmcp", 401, 200)],
        "without the bearer, then with it"
    );
    assert_eq!(
        (stream_without, stream_with),
        (Some(401), None),
        "the event stream without the bearer, then with it"
    );
}
