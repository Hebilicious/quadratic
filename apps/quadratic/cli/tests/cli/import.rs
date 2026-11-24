use crate::support::{Home, Repository};

const REVIEW_RUN: &str = include_str!("../../../server/tests/fixtures/findings/review.json");

#[test]
fn findings_import_reads_a_run_from_the_command_line_and_records_it_once() {
    let home = Home::new();
    let repository = Repository::new();
    home.quadratic(&repository.root)
        .args(["project", "create", "--key", "FND", "--name", "Findings"])
        .run()
        .ok();

    let imported = home
        .quadratic(&repository.root)
        .args([
            "findings",
            "import",
            "--tool",
            "linter",
            "--findings",
            REVIEW_RUN,
        ])
        .run()
        .ok();

    assert_eq!(imported["created"]["count"], 5, "{imported}");
    let listed = home
        .quadratic(&repository.root)
        .args(["issue", "list", "--tool", "linter", "--json"])
        .run()
        .ok();
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(5),
        "{listed}"
    );
}
