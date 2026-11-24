use crate::support::{Home, outside};

fn holds(text: &str, value: &str) -> bool {
    text.contains(value)
}

#[test]
fn the_owner_creates_lists_and_revokes_a_token_and_its_value_stays_in_its_file() {
    let home = Home::new();
    let elsewhere = outside();
    let file = elsewhere.path().join("deploy.token");
    let file_text = file.display().to_string();

    let created = home
        .quadratique(elsewhere.path())
        .args(["token", "create", "--label", "deploy", "--file", &file_text])
        .run();
    let listed = home
        .quadratique(elsewhere.path())
        .args(["token", "list"])
        .run();
    let revoked = home
        .quadratique(elsewhere.path())
        .args(["token", "revoke", "--label", "deploy"])
        .run();
    let stopped = home
        .quadratique(elsewhere.path())
        .args(["server", "stop"])
        .run();

    let value = std::fs::read_to_string(&file).expect("the token's file");
    assert!(value.starts_with("qdt_"), "the file does not hold a token");
    for run in [&created, &listed, &revoked, &stopped] {
        assert_eq!(run.code, 0, "a command failed: {run:?}");
        assert!(
            !holds(&run.stdout, &value),
            "a command's output holds the value"
        );
        assert!(
            !holds(&run.stderr, &value),
            "a command's error output holds the value"
        );
    }
    let log = std::fs::read_to_string(home.path().join("server.log")).expect("the server's log");
    assert!(!log.is_empty(), "the server wrote no standard error");
    assert!(
        !holds(&log, &value),
        "the server's standard error holds the value"
    );
    assert_eq!(created.json()["label"], "deploy");
    assert_eq!(listed.json()["items"][0]["label"], "deploy");
    assert_eq!(revoked.json()["revoked"], true);
}
