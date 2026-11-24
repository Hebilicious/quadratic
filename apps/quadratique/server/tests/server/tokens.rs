use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use acid_cat_core::{Kind, Outcome};
use quadratique_server::git::watcher;
use quadratique_server::server::{self, Paths, Waits};
use quadratique_server::web::WebApp;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::support::{Running, start, within};

struct Elsewhere(TempDir);

impl Elsewhere {
    fn new() -> Elsewhere {
        Elsewhere(tempfile::tempdir().expect("a folder for the token files"))
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .expect("the file is there")
        .permissions()
        .mode()
        & 0o777
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn hex_sha256(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl Running {
    async fn create_token(&self, label: &str, file: &Path) -> Value {
        self.ok(
            &self.human(),
            "token.create",
            json!({ "label": label, "file": text(file) }),
        )
        .await
    }

    async fn create_token_fails(&self, label: &str, file: &str) -> Value {
        self.failure(
            &self.human(),
            "token.create",
            json!({ "label": label, "file": file }),
        )
        .await
    }

    async fn revoke_token(&self, label: &str) -> Value {
        self.ok(&self.human(), "token.revoke", json!({ "label": label }))
            .await
    }

    fn credentials_file(&self) -> PathBuf {
        Paths::in_home(self.home())
            .expect("an absolute home")
            .credentials()
    }
}

async fn start_in(home: TempDir) -> Running {
    let server = within(
        "the server's start",
        server::start_with(
            &Paths::in_home(home.path()).expect("an absolute home"),
            server::loopback(),
            WebApp::from_env(),
            watcher::EVERY,
            Waits::default(),
        ),
    )
    .await
    .expect("the server starts");
    let code = home.path().join("code");
    Running::of(server, code, home)
}

fn files_in(directory: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .expect("the home lists")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.is_file())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_s_value_is_only_in_the_new_private_file_it_names() {
    let running = start().await;
    let elsewhere = Elsewhere::new();
    let file = elsewhere.file("deploy.token");

    let created = running.create_token("deploy", &file).await;
    let listed = running.ok(&running.human(), "token.list", json!({})).await;
    running.project("TOK").await;

    assert_eq!(created, json!({ "label": "deploy", "file": text(&file) }));
    assert_eq!(mode(&file), 0o600);
    let value = std::fs::read_to_string(&file).expect("the token's file");
    assert!(value.starts_with("qdt_"), "the file does not hold a token");
    assert_eq!(value.len(), "qdt_".len() + 43, "not 32 bytes in base64url");
    for answer in [&created, &listed] {
        assert!(
            !contains(answer.to_string().as_bytes(), &value),
            "an answer holds the value"
        );
    }
    let home = running.home().to_path_buf();
    let files = files_in(&home);
    for name in ["credentials.json", "workspace.db"] {
        assert!(files.contains(&home.join(name)), "the home has no {name}");
    }
    for path in files {
        let bytes = std::fs::read(&path).expect("a file of the home");
        assert!(
            !contains(&bytes, &value),
            "{} holds the value",
            path.display()
        );
    }
    assert_eq!(mode(&running.credentials_file()), 0o600);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_existing_file_gives_file_exists_and_stays_untouched() {
    let running = start().await;
    let elsewhere = Elsewhere::new();
    let file = elsewhere.file("taken");
    std::fs::write(&file, "notes that were here first").expect("written");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let dangling = elsewhere.file("dangling");
    std::os::unix::fs::symlink(elsewhere.file("nowhere"), &dangling).expect("a symbolic link");

    for path in [&file, &dangling] {
        let refused = running.create_token_fails("deploy", &text(path)).await;
        assert_eq!(refused, json!({ "_tag": "FileExists", "file": text(path) }));
    }

    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        "notes that were here first"
    );
    assert_eq!(mode(&file), 0o644);
    assert!(
        !elsewhere.file("nowhere").exists(),
        "a write followed the link"
    );
    let listed = running.ok(&running.human(), "token.list", json!({})).await;
    assert_eq!(listed, json!({ "items": [] }));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relative_path_gives_not_absolute() {
    let running = start().await;

    let refused = running.create_token_fails("deploy", "deploy.token").await;

    assert_eq!(
        refused,
        json!({ "_tag": "NotAbsolute", "file": "deploy.token" })
    );
    let listed = running.ok(&running.human(), "token.list", json!({})).await;
    assert_eq!(listed, json!({ "items": [] }));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_in_the_data_folder_gives_in_data_folder() {
    let running = start().await;
    let elsewhere = Elsewhere::new();
    let linked = elsewhere.file("linked-home");
    std::os::unix::fs::symlink(running.home(), &linked).expect("a symbolic link");
    let candidates = [
        running.home().join("credentials.json"),
        running.home().join("credentials.json.partial"),
        running.home().join("deploy.token"),
        linked.join("deploy.token"),
    ];

    for file in &candidates {
        let refused = running.create_token_fails("deploy", &text(file)).await;
        assert_eq!(
            refused,
            json!({ "_tag": "InDataFolder", "file": text(file) })
        );
    }

    for file in &candidates {
        assert!(!file.exists(), "{} was written", file.display());
    }
    let listed = running.ok(&running.human(), "token.list", json!({})).await;
    assert_eq!(listed, json!({ "items": [] }));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_label_that_is_not_a_slug_is_invalid_input() {
    let running = start().await;
    let elsewhere = Elsewhere::new();
    let client = running.client(&running.human());
    let too_long = "a".repeat(33);

    for label in [
        "Deploy",
        "deploy@build-host",
        "two--hyphens",
        "-deploy",
        "with space",
        "tab\there",
        too_long.as_str(),
        "",
    ] {
        let file = elsewhere.file("token");
        let input = json!({ "label": label, "file": text(&file) });
        match within("token.create", client.invoke("token.create", &input)).await {
            Outcome::CallFailure(failure) => assert_eq!(
                failure.kind,
                Kind::InvalidInput,
                "{label:?}: {}",
                failure.message
            ),
            other => panic!("{label:?} was not refused as invalid input: {other:?}"),
        }
        assert!(!file.exists(), "{label:?} wrote a file");
    }
    running
        .create_token(&"a".repeat(32), &elsewhere.file("longest"))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_names_of_the_server_s_own_actors_are_taken() {
    let running = start().await;
    let elsewhere = Elsewhere::new();

    for label in ["quadratique", "agent", "browser", "human"] {
        let file = elsewhere.file(label);
        let refused = running.create_token_fails(label, &text(&file)).await;
        assert_eq!(refused, json!({ "_tag": "LabelTaken", "label": label }));
        assert!(!file.exists(), "{label} wrote a file");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_taken_label_gives_label_taken_and_creates_no_file() {
    let running = start().await;
    let elsewhere = Elsewhere::new();
    running
        .create_token("deploy", &elsewhere.file("first"))
        .await;
    let second = elsewhere.file("second");

    let refused = running.create_token_fails("deploy", &text(&second)).await;

    assert_eq!(refused, json!({ "_tag": "LabelTaken", "label": "deploy" }));
    assert!(!second.exists(), "a refused create wrote a file");
}

#[tokio::test(flavor = "multi_thread")]
async fn token_list_gives_labels_and_dates_and_no_verifier() {
    let running = start().await;
    let elsewhere = Elsewhere::new();
    running
        .create_token("deploy", &elsewhere.file("deploy"))
        .await;
    running
        .create_token("runner", &elsewhere.file("runner"))
        .await;
    let value = std::fs::read_to_string(elsewhere.file("deploy")).expect("the token's file");

    let listed = running.ok(&running.human(), "token.list", json!({})).await;

    let items = listed["items"].as_array().expect("the tokens");
    let labels: Vec<&str> = items
        .iter()
        .map(|item| item["label"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(labels, ["deploy", "runner"]);
    for item in items {
        let keys: std::collections::BTreeSet<&String> =
            item.as_object().expect("a token").keys().collect();
        assert_eq!(keys.into_iter().collect::<Vec<_>>(), ["created", "label"]);
        let created = item["created"].as_str().expect("a date");
        assert!(
            created.parse::<jiff::Timestamp>().is_ok() && created.ends_with('Z'),
            "{created}"
        );
    }
    let verifier = hex_sha256(&value);
    let stored = std::fs::read(running.credentials_file()).expect("credentials.json");
    assert!(
        contains(&stored, &verifier),
        "credentials.json holds no verifier"
    );
    assert!(
        !contains(listed.to_string().as_bytes(), &verifier),
        "the list holds a verifier"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_token_answers_no_label_also_after_a_restart() {
    let running = start().await;
    let elsewhere = Elsewhere::new();
    running
        .create_token("deploy", &elsewhere.file("deploy"))
        .await;
    running
        .create_token("runner", &elsewhere.file("runner"))
        .await;
    let revoked = std::fs::read_to_string(elsewhere.file("deploy")).expect("the token's file");
    let kept = std::fs::read_to_string(elsewhere.file("runner")).expect("the token's file");
    let credentials = running.server.credentials();
    assert_eq!(credentials.label_of(&revoked).as_deref(), Some("deploy"));

    let answer = running.revoke_token("deploy").await;

    assert_eq!(answer, json!({ "label": "deploy", "revoked": true }));
    let credentials = running.server.credentials();
    assert_eq!(credentials.label_of(&revoked), None);
    assert_eq!(credentials.label_of(&kept).as_deref(), Some("runner"));
    let (server, _code, home) = running.into_parts();
    within("the shutdown", server.shutdown()).await;
    let restarted = start_in(home).await;
    let credentials = restarted.server.credentials();
    assert_eq!(credentials.label_of(&revoked), None);
    assert_eq!(credentials.label_of(&kept).as_deref(), Some("runner"));
    assert!(
        elsewhere.file("deploy").exists(),
        "revoke removed the token's file"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_revoke_changes_nothing() {
    let running = start().await;
    let elsewhere = Elsewhere::new();
    running
        .create_token("deploy", &elsewhere.file("deploy"))
        .await;
    running
        .create_token("runner", &elsewhere.file("runner"))
        .await;
    running.revoke_token("deploy").await;
    let before = std::fs::read(running.credentials_file()).expect("credentials.json");
    let listed = running.ok(&running.human(), "token.list", json!({})).await;

    let again = running.revoke_token("deploy").await;
    let never = running.revoke_token("never").await;

    assert_eq!(again, json!({ "label": "deploy", "revoked": false }));
    assert_eq!(never, json!({ "label": "never", "revoked": false }));
    assert_eq!(
        std::fs::read(running.credentials_file()).expect("credentials.json"),
        before
    );
    assert_eq!(
        running.ok(&running.human(), "token.list", json!({})).await,
        listed
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_damaged_credentials_file_stops_the_start_and_names_itself() {
    let home = tempfile::tempdir().expect("a temporary home");
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    std::fs::write(paths.credentials(), "{ not json").expect("written");

    let refused = within(
        "the server's start",
        server::start_with(
            &paths,
            server::loopback(),
            WebApp::from_env(),
            watcher::EVERY,
            Waits::default(),
        ),
    )
    .await
    .err()
    .expect("the start is refused");

    assert!(
        matches!(refused, server::ServerError::Credentials(_)),
        "{refused}"
    );
    assert!(
        refused.to_string().contains(&text(&paths.credentials())),
        "{refused}"
    );
    assert!(!paths.runtime.exists(), "a refused server published itself");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_home_stays_owned_while_a_copy_of_the_credentials_lives() {
    let running = start().await;
    let left_running = running.server.credentials().clone();
    let (server, _code, home) = running.into_parts();
    within("the shutdown", server.shutdown()).await;
    let paths = Paths::in_home(home.path()).expect("an absolute home");
    let short = Waits {
        start: std::time::Duration::from_millis(500),
        ..Waits::default()
    };

    let refused = within(
        "the refused start",
        server::start_with(
            &paths,
            server::loopback(),
            WebApp::from_env(),
            watcher::EVERY,
            short,
        ),
    )
    .await
    .err()
    .expect("a second server starts while the credentials live");
    drop(left_running);
    let started = within(
        "the start",
        server::start_with(
            &paths,
            server::loopback(),
            WebApp::from_env(),
            watcher::EVERY,
            Waits::default(),
        ),
    )
    .await;

    assert!(
        matches!(refused, server::ServerError::Owned(_)),
        "{refused}"
    );
    assert!(started.is_ok(), "{:?}", started.err());
}
