use crate::support::{Home, Repository};

const INVALID_INPUT: i32 = 3;

#[test]
fn a_create_without_acceptance_criteria_exits_as_invalid_input_and_creates_nothing() {
    let home = Home::new();
    let repository = Repository::new();
    home.quadratic(&repository.root)
        .args(["project", "create", "--key", "DEM", "--name", "Demo"])
        .run()
        .ok();

    let refused: Vec<_> = [
        vec!["create", "--title", "No criteria"],
        vec!["create", "--title", "Empty criteria", "--acceptance", ""],
        vec![
            "create",
            "--title",
            "Blank criteria",
            "--acceptance",
            "  \t ",
        ],
    ]
    .into_iter()
    .map(|args| home.quadratic(&repository.root).args(args).run())
    .collect();
    let listed = home
        .quadratic(&repository.root)
        .args(["issue", "list"])
        .run()
        .ok();
    let created = home
        .quadratic(&repository.root)
        .args([
            "create",
            "--title",
            "With criteria",
            "--acceptance",
            "x passes",
        ])
        .run()
        .ok();

    for run in &refused {
        assert_eq!(run.code, INVALID_INPUT, "{run:?}");
        assert!(run.stderr.contains("acceptance"), "{run:?}");
    }
    assert_eq!(listed["items"], serde_json::json!([]), "{listed}");
    assert_eq!(created["issue"]["id"], "DEM-1");
}
