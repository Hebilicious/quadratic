//! The texts that builds of Quadratique, and of Quadratic before the rename, wrote into a
//! repository. `init` replaces a file that holds one of them and keeps any other text, which a
//! person wrote. A text is known by the SHA-256 of its bytes, so the skill an agent reads carries
//! no version marker.
//!
//! When a skill text changes, add the SHA-256 of the new text at the end of its list and keep the
//! previous text as a fixture in `tests/fixtures/skill/quadratique-<commit>/`: the tests below
//! fail until both are there. Never remove a digest: repositories keep the text of the build that
//! set them up.

use sha2::{Digest, Sha256};

/// `.agents/skills/quadratique/SKILL.md`, oldest first: 3b8a755, 57e4140, f4e2319, acbb941.
pub const SKILL_MD: &[&str] = &[
    "fab0cd822aae542fe583f3ded982e56a7f00a651b662539cff800df8416ae0aa",
    "436608de203dc8c8fb0704c8ac70ca3fad2fc7bf04a7b5055072bb9c70322f2b",
    "a1a05f7f68d80c7adf4cfaa5d37877d42a60715f61212c5050935cfb76bc7a21",
    "3c07040a22b65572f0bb7e63494b6ac6b3d79a09785b8794935596136f432616",
];

/// `.agents/skills/quadratique/planning.md`, oldest first: 3b8a755.
pub const PLANNING_MD: &[&str] =
    &["14caf7c768e7217cbd0302598f0cb18c943ad42fa06f8b337b53d2cf84c8cd12"];

/// `.agents/skills/quadratic/SKILL.md`, written by Quadratic before the rename: c2b3f53, e633b9d.
/// No build writes there any more, so this list is closed.
pub const QUADRATIC_SKILL_MD: &[&str] = &[
    "af86a60776d6cb38f092c3622b1868fcc73f64f88682a0323495d7c84540fdd2",
    "f921a5321438470bd1dd279022c93288388fbad0a70a374464d9cf946d1f9259",
];

/// `.agents/skills/quadratic/planning.md`, written by Quadratic before the rename: c2b3f53.
pub const QUADRATIC_PLANNING_MD: &[&str] =
    &["c2d265a6fc3e8a8608e3ac37a503964658291e066a467cdd9e89f1a18d9289f6"];

/// The `AGENTS.md` lines of earlier builds: Quadratic's, before the rename.
pub const AGENTS_LINES: &[&str] = &[
    "We use Quadratic for issues: every task is an issue there. Use the `quadratic` skill (`.agents/skills/quadratic/SKILL.md`) for anything about tasks.",
];

/// Where Quadratic wrote its skill, before the rename, with the texts it wrote there.
pub const QUADRATIC_SKILL_FILES: [(&str, &[&str]); 2] = [
    (".agents/skills/quadratic/SKILL.md", QUADRATIC_SKILL_MD),
    (
        ".agents/skills/quadratic/planning.md",
        QUADRATIC_PLANNING_MD,
    ),
];

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn is_listed(bytes: &[u8], digests: &[&str]) -> bool {
    let digest = digest(bytes);
    digests.contains(&digest.as_str())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::{PLANNING_MD, QUADRATIC_PLANNING_MD, QUADRATIC_SKILL_MD, SKILL_MD, digest};

    const SKILL: &str = include_str!("../../skill/quadratique/SKILL.md");
    const PLANNING: &str = include_str!("../../skill/quadratique/planning.md");

    #[test]
    fn the_text_this_build_writes_is_listed() {
        for (name, text, digests) in [
            ("SKILL_MD", SKILL, SKILL_MD),
            ("PLANNING_MD", PLANNING, PLANNING_MD),
        ] {
            let digest = digest(text.as_bytes());
            assert!(
                digests.contains(&digest.as_str()),
                "The skill text changed: add \"{digest}\" at the end of {name} in apps/quadratique/cli/src/shipped.rs, and keep the previous text in apps/quadratique/cli/tests/fixtures/skill/quadratique-<commit>/."
            );
        }
    }

    /// The digests of `file` in every fixture folder whose name starts with `build-`.
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
}
