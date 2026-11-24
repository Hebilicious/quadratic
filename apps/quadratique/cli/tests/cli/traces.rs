use crate::support::{Home, Repository, git};

const MADE_UP_TOKEN: &str = "ghp_MadeUpCliToken0123456789";

#[test]
fn a_command_and_its_server_leave_no_trace_of_the_remotes_credential_when_git_configuration_asks_for_traces()
 {
    let home = Home::new();
    let repository = Repository::new();
    git(
        &repository.root,
        &[
            "remote",
            "add",
            "origin",
            &format!("https://{MADE_UP_TOKEN}@github.com/Owner/repo.git"),
        ],
    );
    let traces: Vec<_> = ["event", "normal", "perf"]
        .iter()
        .map(|kind| home.path().join(format!("{kind}.trace")))
        .collect();
    std::fs::write(
        home.path().join(".gitconfig"),
        format!(
            "[trace2]\n\teventTarget = {}\n\tnormalTarget = {}\n\tperfTarget = {}\n\tconfigParams = remote.*\n",
            traces[0].display(),
            traces[1].display(),
            traces[2].display()
        ),
    )
    .expect("a global git configuration in the home");

    home.quadratique(&repository.root)
        .args(["project", "create", "--key", "DEM", "--name", "Demo"])
        .run()
        .ok();
    let created = home
        .quadratique(&repository.root)
        .args(["create", "--title", "Traced", "--acceptance", "It is done."])
        .run()
        .ok();
    let shown = home
        .quadratique(&repository.root)
        .args([
            "show",
            "--issue",
            created["issue"]["id"].as_str().expect("an id"),
        ])
        .run()
        .ok();
    home.quadratique(&repository.root)
        .args(["server", "stop"])
        .run()
        .ok();

    assert!(!shown.to_string().contains(MADE_UP_TOKEN), "{shown}");
    for path in &traces {
        let traced = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            !traced.contains(MADE_UP_TOKEN),
            "{}: {traced}",
            path.display()
        );
    }
    let control = std::process::Command::new("git")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&repository.root)
        .output()
        .expect("git runs");
    assert!(control.status.success());
    assert!(
        traces.iter().all(
            |path| std::fs::read_to_string(path).is_ok_and(|text| text.contains(MADE_UP_TOKEN))
        ),
        "this configuration makes a plain git command trace the remote's URL"
    );
}
