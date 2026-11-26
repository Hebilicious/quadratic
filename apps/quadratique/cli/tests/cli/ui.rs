use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use acid_cat_core::Outcome;
use acid_cat_core::http::HttpClient;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::support::{HANG_GUARD, Home, await_until, outside};

fn within<F: Future>(what: &str, future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(async { tokio::time::timeout(HANG_GUARD, future).await })
        .unwrap_or_else(|_| panic!("{what} did not end within the hang guard of {HANG_GUARD:?}"))
}

fn origin(url: &str) -> &str {
    url.strip_suffix('/').expect("a URL that ends with /")
}

fn status_of(url: &str) -> u16 {
    status_at(url, "/")
}

fn status_at(url: &str, path: &str) -> u16 {
    let path = path.to_owned();
    let address = origin(url)
        .strip_prefix("http://")
        .expect("an http URL")
        .to_owned();
    let head = within("a request for the page", async move {
        let mut stream = TcpStream::connect(&address).await.expect("a connection");
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
        stream
            .write_all(request.as_bytes())
            .await
            .expect("the request is sent");
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).await.expect("an answer");
        String::from_utf8_lossy(&answer).into_owned()
    });
    head.split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or_else(|| panic!("an HTTP status line, not {head:?}"))
}

/// Sends `request`, with `{host}` replaced by the server's address, and returns the status and the body.
fn exchange(url: &str, request: &str) -> (u16, String) {
    let address = origin(url)
        .strip_prefix("http://")
        .expect("an http URL")
        .to_owned();
    let request = request.replace("{host}", &address);
    let answer = within("a request to the server", async move {
        let mut stream = TcpStream::connect(&address).await.expect("a connection");
        stream
            .write_all(request.as_bytes())
            .await
            .expect("the request is sent");
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).await.expect("an answer");
        String::from_utf8_lossy(&answer).into_owned()
    });
    let (head, body) = answer.split_once("\r\n\r\n").expect("a head and a body");
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .expect("a status");
    (status, body.to_owned())
}

fn link_of(printed: &Value) -> String {
    printed["link"].as_str().expect("a link").to_owned()
}

/// The link a bootstrap file sends the browser to.
fn opened_link(bootstrap: &str) -> String {
    let page = std::fs::read_to_string(bootstrap).expect("the bootstrap file");
    let start = page.find("url=").expect("a refresh") + "url=".len();
    let end = page[start..].find('"').expect("the end of the refresh") + start;
    page[start..end].to_owned()
}

/// What the page does with the code of a link: `POST /session`, for the token.
fn redeem(link: &str) -> String {
    let (url, code) = link.split_once("#pair=").expect("a code in the link");
    let body = json!({ "code": code }).to_string();
    let (status, answer) = exchange(
        url,
        &format!(
            "POST /session HTTP/1.1\r\nHost: {{host}}\r\nOrigin: http://{{host}}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    assert_eq!(status, 200, "{answer}");
    serde_json::from_str::<Value>(&answer).expect("a JSON answer")["token"]
        .as_str()
        .expect("a token")
        .to_owned()
}

/// The status of `project.list` on the web target with the bearer `token`.
fn listed_with(url: &str, token: &str) -> u16 {
    exchange(
        url,
        &format!(
            "POST /web/capabilities/project.list HTTP/1.1\r\nHost: {{host}}\r\nOrigin: http://{{host}}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    )
    .0
}

/// A client of the web app's `route`, as the page sends its calls with the bearer `token`, claiming to be `claimed`.
fn client(url: &str, token: &str, route: &str, claimed: &Value) -> HttpClient {
    HttpClient::new(&format!("{}{route}", origin(url)))
        .and_then(|client| client.header("Origin", origin(url)))
        .and_then(|client| client.header("Authorization", &format!("Bearer {token}")))
        .and_then(|client| client.header("Quadratique-Actor", &claimed.to_string()))
        .expect("a client of the route")
}

fn ok(client: &HttpClient, id: &str, input: Value) -> Value {
    match within(id, client.invoke(id, &input)) {
        Outcome::Value(value) => value,
        other => panic!("{id} {input} failed: {other:?}"),
    }
}

fn fake_opener(directory: &Path, opened: &Path) {
    let script = directory.join("xdg-open");
    let opened = opened.display();
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s' \"$1\" > '{opened}.part' && mv '{opened}.part' '{opened}'\n"
        ),
    )
    .expect("a fake opener");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("an executable opener");
}

fn await_file(path: &Path) -> String {
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "{} was not written within the hang guard",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn ui_starts_the_server_prints_the_address_of_the_web_app_and_opens_it_when_asked() {
    let home = Home::new();
    let elsewhere = outside();
    let app = elsewhere.path().join("dist");
    std::fs::create_dir(&app).expect("a folder for the built app");
    std::fs::write(app.join("index.html"), "<title>Quadratique</title>").expect("an index page");
    let bin = elsewhere.path().join("bin");
    std::fs::create_dir(&bin).expect("a folder for the fake opener");
    let opened = elsewhere.path().join("opened");
    fake_opener(&bin, &opened);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let printed = home
        .quadratique(elsewhere.path())
        .env("QUADRATIQUE_WEB", &app.display().to_string())
        .args(["ui"])
        .run()
        .ok();
    let url = printed["url"].as_str().expect("a URL").to_owned();
    let link = link_of(&printed);
    assert_eq!(
        printed,
        json!({ "url": url, "link": link, "opened": false, "addressKept": true })
    );
    assert!(link.starts_with(&format!("{url}#pair=")), "{link}");
    assert_eq!(status_of(&url), 200, "the page at {url}");

    let reopened = home
        .quadratique(elsewhere.path())
        .env("PATH", &path)
        .args(["ui", "--open"])
        .run()
        .ok();
    assert_eq!(
        reopened,
        json!({ "url": url, "opened": true, "addressKept": true })
    );
    let bootstrap = await_file(&opened);
    assert!(
        Path::new(&bootstrap).starts_with(home.path()),
        "the opener got {bootstrap}"
    );
    assert!(
        opened_link(&bootstrap).starts_with(&format!("{url}#pair=")),
        "{bootstrap}"
    );
}

#[test]
fn the_web_route_acts_as_the_servers_login_and_webmcp_as_the_agent_browser_whatever_a_page_claims()
{
    let home = Home::new();
    let elsewhere = outside();
    let printed = home
        .quadratique(elsewhere.path())
        .env("USER", "ada")
        .args(["ui"])
        .run()
        .ok();
    let url = printed["url"].as_str().expect("a URL").to_owned();
    let token = redeem(&link_of(&printed));
    let page = client(
        &url,
        &token,
        "/web",
        &json!({ "kind": "agent", "name": "claude-code@elsewhere", "harness": "claude-code" }),
    );
    let agent = client(
        &url,
        &token,
        "/webmcp",
        &json!({ "kind": "human", "name": "sam" }),
    );

    ok(
        &page,
        "project.create",
        json!({ "key": "WEB", "name": "Web" }),
    );
    let by_page = ok(
        &page,
        "issue.create",
        json!({ "project": "WEB", "title": "From the page", "acceptance": "It is done." }),
    );
    let by_agent = ok(
        &agent,
        "issue.create",
        json!({ "project": "WEB", "title": "From an agent in the browser", "acceptance": "It is done." }),
    );

    let creator = |created: &Value| {
        let id = created["issue"]["id"].as_str().expect("an issue id");
        ok(&page, "issue.get", json!({ "issue": id }))["issue"]["createdBy"].clone()
    };
    assert_eq!(creator(&by_page), json!({ "kind": "human", "name": "ada" }));
    assert_eq!(
        creator(&by_agent),
        json!({ "kind": "agent", "name": "browser", "harness": "browser" })
    );
}

#[test]
fn ui_open_reports_an_opener_that_fails_with_its_exit_code() {
    let home = Home::new();
    let elsewhere = outside();
    let bin = elsewhere.path().join("bin");
    std::fs::create_dir(&bin).expect("a folder for the fake opener");
    let script = bin.join("xdg-open");
    std::fs::write(&script, "#!/bin/sh\nexit 3\n").expect("a failing opener");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("an executable opener");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let refused = home
        .quadratique(elsewhere.path())
        .env("PATH", &path)
        .args(["ui", "--open"])
        .run();

    assert_ne!(refused.code, 0, "{refused:?}");
    assert!(refused.stderr.contains("exited with code 3"), "{refused:?}");
}

fn built_app(elsewhere: &Path) -> String {
    let app = elsewhere.join("dist");
    std::fs::create_dir(&app).expect("a folder for the built app");
    std::fs::write(app.join("index.html"), "<title>Quadratique</title>").expect("an index page");
    app.display().to_string()
}

fn ui_url(home: &Home, elsewhere: &Path, app: &str) -> Value {
    home.quadratique(elsewhere)
        .env("QUADRATIQUE_WEB", app)
        .args(["ui"])
        .run()
        .ok()
}

fn url_of(printed: &Value) -> String {
    printed["url"].as_str().expect("a URL").to_owned()
}

fn stop(home: &Home, elsewhere: &Path) {
    home.quadratique(elsewhere)
        .args(["server", "stop"])
        .run()
        .ok();
}

fn strict(home: &Home, elsewhere: &Path, on: &str) -> Value {
    home.quadratique(elsewhere)
        .args(["server", "strict", on])
        .run()
        .ok()
}

#[test]
fn by_default_after_a_restart_the_web_app_has_the_same_address_and_an_issue_page_opened_before_still_loads()
 {
    let home = Home::new();
    let elsewhere = outside();
    let app = built_app(elsewhere.path());
    let before = ui_url(&home, elsewhere.path(), &app);
    let url = url_of(&before);
    assert_eq!(
        status_at(&url, "/issues/ABC-1"),
        200,
        "the issue page at {url}"
    );
    let kept = std::fs::read_to_string(home.path().join("server.port")).expect("a kept port");

    stop(&home, elsewhere.path());
    let after = ui_url(&home, elsewhere.path(), &app);

    assert_eq!(
        (&after["url"], &after["addressKept"]),
        (&json!(url), &json!(true)),
        "{after}"
    );
    assert_eq!(format!("http://127.0.0.1:{}/", kept.trim()), url);
    assert_eq!(
        status_at(&url, "/issues/ABC-1"),
        200,
        "the issue page at {url}"
    );
}

#[test]
fn when_the_kept_port_is_taken_ui_opens_the_new_address_which_is_kept() {
    let home = Home::new();
    let elsewhere = outside();
    let app = built_app(elsewhere.path());
    let bin = elsewhere.path().join("bin");
    std::fs::create_dir(&bin).expect("a folder for the fake opener");
    let opened = elsewhere.path().join("opened");
    fake_opener(&bin, &opened);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let before = url_of(&ui_url(&home, elsewhere.path(), &app));
    stop(&home, elsewhere.path());
    let previous = origin(&before)
        .strip_prefix("http://")
        .expect("an http URL");
    let squatter = std::net::TcpListener::bind(previous).expect("the kept port, now free");

    let reopened = home
        .quadratique(elsewhere.path())
        .env("QUADRATIQUE_WEB", &app)
        .env("PATH", &path)
        .args(["ui", "--open"])
        .run()
        .ok();

    let after = url_of(&reopened);
    assert_ne!(after, before);
    assert_eq!(
        reopened,
        json!({ "url": after, "opened": true, "addressKept": true })
    );
    assert!(
        opened_link(&await_file(&opened)).starts_with(&format!("{after}#pair=")),
        "the opener got another address"
    );
    assert_eq!(
        status_at(&after, "/issues/ABC-1"),
        200,
        "the issue page at {after}"
    );
    stop(&home, elsewhere.path());
    drop(squatter);
    assert_eq!(url_of(&ui_url(&home, elsewhere.path(), &app)), after);
}

#[test]
fn strict_on_takes_a_fresh_address_at_each_start_and_after_strict_off_the_next_address_is_kept() {
    let home = Home::new();
    let elsewhere = outside();
    let app = built_app(elsewhere.path());
    let first = url_of(&ui_url(&home, elsewhere.path(), &app));

    let on = strict(&home, elsewhere.path(), "on");
    assert_eq!(on["strict"], true, "{on}");
    assert_eq!(on["addressKept"], false, "{on}");
    assert!(
        !home.path().join("server.port").exists(),
        "strict on kept the port"
    );
    stop(&home, elsewhere.path());
    let second = ui_url(&home, elsewhere.path(), &app);
    stop(&home, elsewhere.path());
    let third = url_of(&ui_url(&home, elsewhere.path(), &app));

    assert_eq!(second["addressKept"], false, "{second}");
    let second = url_of(&second);
    assert_ne!(second, first);
    assert_ne!(third, first);
    assert_ne!(third, second);
    assert!(
        !home.path().join("server.port").exists(),
        "a strict start kept its port"
    );

    let off = strict(&home, elsewhere.path(), "off");
    assert_eq!(off["addressKept"], true, "{off}");
    stop(&home, elsewhere.path());
    let status = home
        .quadratique(elsewhere.path())
        .args(["server", "status"])
        .run()
        .ok();
    assert_eq!(status["running"], false, "{status}");
    let fourth = url_of(&ui_url(&home, elsewhere.path(), &app));
    let status = home
        .quadratique(elsewhere.path())
        .args(["server", "status"])
        .run()
        .ok();
    assert_eq!(
        (&status["web"], &status["addressKept"]),
        (&json!(fourth), &json!(true)),
        "{status}"
    );
    stop(&home, elsewhere.path());
    assert_eq!(url_of(&ui_url(&home, elsewhere.path(), &app)), fourth);
}

fn server_status(home: &Home, elsewhere: &Path) -> Value {
    home.quadratique(elsewhere)
        .args(["server", "status"])
        .run()
        .ok()
}

#[test]
fn a_server_on_a_given_port_says_its_address_is_not_kept() {
    let home = Home::new();
    let elsewhere = outside();
    let app = built_app(elsewhere.path());
    let kept = url_of(&ui_url(&home, elsewhere.path(), &app));
    stop(&home, elsewhere.path());
    let serving = home
        .quadratique(elsewhere.path())
        .env("QUADRATIQUE_WEB", &app)
        .args(["serve", "--port", "0"])
        .spawn();
    await_until("the server on port 0 answers", || {
        server_status(&home, elsewhere.path())["running"] == true
    });

    let printed = ui_url(&home, elsewhere.path(), &app);
    let status = server_status(&home, elsewhere.path());
    stop(&home, elsewhere.path());
    serving.finish();

    assert_ne!(url_of(&printed), kept);
    assert_eq!(printed["addressKept"], false, "{printed}");
    assert_eq!(status["addressKept"], false, "{status}");
}

#[test]
fn a_server_that_could_not_keep_its_port_says_its_address_is_not_kept() {
    let home = Home::new();
    let elsewhere = outside();
    let app = built_app(elsewhere.path());
    std::fs::create_dir(home.path().join("server.port")).expect("a folder where server.port goes");

    let printed = ui_url(&home, elsewhere.path(), &app);
    let status = server_status(&home, elsewhere.path());
    stop(&home, elsewhere.path());
    let stopped = server_status(&home, elsewhere.path());

    assert_eq!(printed["addressKept"], false, "{printed}");
    assert_eq!(status["addressKept"], false, "{status}");
    assert_eq!(
        stopped["addressKept"], true,
        "with no server, addressKept says what the next start does: {stopped}"
    );
}

#[test]
fn strict_on_and_off_start_no_server_and_apply_from_the_next_start() {
    let home = Home::new();
    let elsewhere = outside();

    let on = strict(&home, elsewhere.path(), "on");
    let after_on = server_status(&home, elsewhere.path());
    let off = strict(&home, elsewhere.path(), "off");
    let after_off = server_status(&home, elsewhere.path());

    assert_eq!(
        after_on["running"], false,
        "strict on started a server: {after_on}"
    );
    assert_eq!(
        after_off["running"], false,
        "strict off started a server: {after_off}"
    );
    assert_eq!(on["from"], "the next start of the server", "{on}");
    assert_eq!(off["from"], "the next start of the server", "{off}");
}

#[test]
fn a_browser_let_in_by_ui_stays_in_across_a_restart_until_server_sign_out() {
    let home = Home::new();
    let elsewhere = outside();
    let app = built_app(elsewhere.path());
    let printed = ui_url(&home, elsewhere.path(), &app);
    let url = url_of(&printed);
    let token = redeem(&link_of(&printed));
    assert_eq!(listed_with(&url, &token), 200);
    let file = std::fs::read_to_string(home.path().join("web-token")).expect("the token file");
    assert_eq!(file.trim(), token);

    stop(&home, elsewhere.path());
    let after = ui_url(&home, elsewhere.path(), &app);
    assert_eq!(url_of(&after), url);
    assert_eq!(
        listed_with(&url, &token),
        200,
        "a restart signed the browser out"
    );

    let signed_out = home
        .quadratique(elsewhere.path())
        .args(["server", "sign-out"])
        .run()
        .ok();
    assert_eq!(
        (&signed_out["signedOut"], &signed_out["lasting"]),
        (&json!(true), &json!(true)),
        "{signed_out}"
    );
    assert!(!signed_out.to_string().contains(&token));
    assert_eq!(listed_with(&url, &token), 401);
    let again = redeem(&link_of(&ui_url(&home, elsewhere.path(), &app)));
    assert_ne!(again, token);
    assert_eq!(listed_with(&url, &again), 200);
}

#[test]
fn in_strict_mode_a_restart_signs_every_browser_out_and_ui_lets_it_in_again() {
    let home = Home::new();
    let elsewhere = outside();
    let app = built_app(elsewhere.path());
    strict(&home, elsewhere.path(), "on");
    stop(&home, elsewhere.path());
    let first = ui_url(&home, elsewhere.path(), &app);
    let token = redeem(&link_of(&first));
    assert!(!home.path().join("web-token").exists());

    stop(&home, elsewhere.path());
    let second = ui_url(&home, elsewhere.path(), &app);
    let url = url_of(&second);

    assert_ne!(url, url_of(&first));
    assert_eq!(listed_with(&url, &token), 401);
    let again = redeem(&link_of(&second));
    assert_eq!(listed_with(&url, &again), 200);
}

#[test]
fn a_running_server_says_its_address_is_kept_only_while_the_next_start_would_ask_for_it() {
    let home = Home::new();
    let elsewhere = outside();
    let app = built_app(elsewhere.path());
    let first = ui_url(&home, elsewhere.path(), &app);
    assert_eq!(first["addressKept"], true, "{first}");

    strict(&home, elsewhere.path(), "on");
    let after_strict = ui_url(&home, elsewhere.path(), &app);
    let status_after_strict = server_status(&home, elsewhere.path());
    strict(&home, elsewhere.path(), "off");
    std::fs::write(home.path().join("server.port"), "9\n").expect("server.port edited by hand");
    let after_edit = server_status(&home, elsewhere.path());
    std::fs::remove_file(home.path().join("server.port")).expect("server.port removed by hand");
    let after_removal = ui_url(&home, elsewhere.path(), &app);
    stop(&home, elsewhere.path());

    assert_eq!(after_strict["addressKept"], false, "{after_strict}");
    assert_eq!(
        status_after_strict["addressKept"], false,
        "{status_after_strict}"
    );
    assert_eq!(after_edit["addressKept"], false, "{after_edit}");
    assert_eq!(after_removal["addressKept"], false, "{after_removal}");
}

#[test]
fn the_first_start_after_strict_off_makes_a_new_token_even_with_no_strict_start_in_between() {
    let home = Home::new();
    let elsewhere = outside();
    let app = built_app(elsewhere.path());
    let token = redeem(&link_of(&ui_url(&home, elsewhere.path(), &app)));

    strict(&home, elsewhere.path(), "on");
    strict(&home, elsewhere.path(), "off");
    stop(&home, elsewhere.path());
    let url = url_of(&ui_url(&home, elsewhere.path(), &app));

    assert_eq!(
        listed_with(&url, &token),
        401,
        "the old token outlived strict on and off"
    );
}
