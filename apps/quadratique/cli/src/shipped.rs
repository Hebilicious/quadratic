//! The texts that builds of Quadratique, and of Quadratic before the rename, wrote into a
//! repository. `init` replaces a file that holds one of them and keeps any other text, which a
//! person wrote. A text is known by the SHA-256 of its bytes, so the skill an agent reads carries
//! no version marker.
//!
//! When a skill text changes, add the SHA-256 of the new text at the end of its list and keep the
//! previous text as a fixture in `tests/fixtures/skill/quadratique-<commit>/`: the test below and
//! `tests/cli/shipped_lists.rs` (which compares each list with the fixtures, outside the package)
//! fail until both are there. Never remove a digest: repositories keep the text of the build that
//! set them up.

use sha2::{Digest, Sha256};

/// `.agents/skills/quadratique/SKILL.md`, oldest first: 3b8a755, 57e4140, f4e2319, acbb941,
/// f0b3930 (QUA-30, pull request 20), 23dcc62 (QUA-65's first text), and the text of QUA-65.
pub const SKILL_MD: &[&str] = &[
    "fab0cd822aae542fe583f3ded982e56a7f00a651b662539cff800df8416ae0aa",
    "436608de203dc8c8fb0704c8ac70ca3fad2fc7bf04a7b5055072bb9c70322f2b",
    "a1a05f7f68d80c7adf4cfaa5d37877d42a60715f61212c5050935cfb76bc7a21",
    "3c07040a22b65572f0bb7e63494b6ac6b3d79a09785b8794935596136f432616",
    "1ef8092a26716c8a466e7a1bf332e0424a3f29f7f9540c699b530d2272a39942",
    "ee796f7c3e2d38455d3cab76768482edf178e44fc279d0bac7e745f639fa96e7",
    "5abfa582f307435cf43efa5f44419bdb858e68b69997697f232c4c84172b7311",
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
    use super::{PLANNING_MD, SKILL_MD, digest};

    const SKILL: &str = include_str!("skill/SKILL.md");
    const PLANNING: &str = include_str!("skill/planning.md");
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
}
