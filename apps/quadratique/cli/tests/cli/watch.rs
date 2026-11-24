use crate::support::{Home, Lines, Repository};

#[test]
fn watch_prints_one_line_of_json_for_a_change_that_another_command_makes() {
    let home = Home::new();
    let repository = Repository::new();
    home.quadratique(&repository.root)
        .args(["project", "create", "--key", "LIVE", "--name", "Live"])
        .run()
        .ok();
    let history = home
        .quadratique(&repository.root)
        .args(["history", "query"])
        .run()
        .ok();
    let latest = history["items"][0]["commit"]
        .as_str()
        .expect("the commit of the project")
        .to_owned();

    let mut watcher = home
        .quadratique(&repository.root)
        .args(["watch", "issue.changed", "--cursor", &latest])
        .spawn();
    let lines = Lines::of(watcher.stdout());
    let created = home
        .quadratique(&repository.root)
        .args([
            "create",
            "--title",
            "Watched",
            "--acceptance",
            "It is done.",
        ])
        .run()
        .ok();

    let occurrence = lines.next_json();
    assert_eq!(occurrence["name"], "issue.changed");
    assert_eq!(occurrence["data"]["operation"], "issue.create");
    assert_eq!(
        occurrence["data"]["issues"][0]["issue"]["id"],
        created["issue"]["id"]
    );
}
