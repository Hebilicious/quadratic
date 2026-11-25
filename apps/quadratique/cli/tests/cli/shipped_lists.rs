//! The digest lists of `src/shipped.rs` against the earlier skill texts kept as fixtures. This
//! lives in the integration tests because the fixtures are not part of the packaged crate.

use std::collections::BTreeSet;
use std::path::Path;

use crate::shipped::{PLANNING_MD, QUADRATIC_PLANNING_MD, QUADRATIC_SKILL_MD, SKILL_MD, digest};

const SKILL: &str = include_str!("../../../skill/quadratique/SKILL.md");
const PLANNING: &str = include_str!("../../../skill/quadratique/planning.md");

/// The digests of `file` in every fixture folder named `<build>-<commit>`.
fn fixtures(build: &str, file: &str) -> BTreeSet<String> {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skill");
    std::fs::read_dir(&folder)
        .expect("the fixtures of earlier skill texts")
        .map(|entry| entry.expect("a fixture").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.rsplit_once('-'))
                .is_some_and(|(name, _)| name == build)
        })
        .map(|path| digest(&std::fs::read(path.join(file)).expect("a fixture file")))
        .collect()
}

fn listed(digests: &[&str]) -> BTreeSet<String> {
    digests.iter().map(|digest| (*digest).to_owned()).collect()
}

#[test]
fn each_list_holds_exactly_the_texts_kept_as_fixtures_and_the_current_text() {
    let current = |text: &str| BTreeSet::from([digest(text.as_bytes())]);
    for (list, digests, expected) in [
        (
            "SKILL_MD",
            SKILL_MD,
            &fixtures("quadratique", "SKILL.md") | &current(SKILL),
        ),
        (
            "PLANNING_MD",
            PLANNING_MD,
            &fixtures("quadratique", "planning.md") | &current(PLANNING),
        ),
        (
            "QUADRATIC_SKILL_MD",
            QUADRATIC_SKILL_MD,
            fixtures("quadratic", "SKILL.md"),
        ),
        (
            "QUADRATIC_PLANNING_MD",
            QUADRATIC_PLANNING_MD,
            fixtures("quadratic", "planning.md"),
        ),
    ] {
        assert!(!expected.is_empty(), "{list}: no fixture found");
        assert_eq!(
            listed(digests),
            expected,
            "{list} must list the digest of each fixture text (and of the current text for this build's paths), and nothing else"
        );
    }
}
