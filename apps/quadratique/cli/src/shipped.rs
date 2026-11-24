//! The texts that builds of Quadratique, and of Quadratic before the rename, wrote into a
//! repository. `init` replaces a file that holds one of them and keeps any other text, which a
//! person wrote. A text is known by the SHA-256 of its bytes, so the skill an agent reads carries
//! no version marker.
//!
//! When a skill text changes, add the SHA-256 of the new text to its list: the test
//! `every_text_this_build_writes_is_listed` fails until it is there. Never remove a digest:
//! repositories keep the text of the build that set them up.

use sha2::{Digest, Sha256};

/// `SKILL.md`, oldest first: Quadratic 0.1.0 (c2b3f53), e633b9d, then Quadratique 3b8a755,
/// 57e4140, f4e2319, acbb941.
pub const SKILL_MD: &[&str] = &[
    "af86a60776d6cb38f092c3622b1868fcc73f64f88682a0323495d7c84540fdd2",
    "f921a5321438470bd1dd279022c93288388fbad0a70a374464d9cf946d1f9259",
    "fab0cd822aae542fe583f3ded982e56a7f00a651b662539cff800df8416ae0aa",
    "436608de203dc8c8fb0704c8ac70ca3fad2fc7bf04a7b5055072bb9c70322f2b",
    "a1a05f7f68d80c7adf4cfaa5d37877d42a60715f61212c5050935cfb76bc7a21",
    "3c07040a22b65572f0bb7e63494b6ac6b3d79a09785b8794935596136f432616",
];

/// `planning.md`, oldest first: Quadratic 0.1.0 (c2b3f53), then Quadratique 3b8a755.
pub const PLANNING_MD: &[&str] = &[
    "c2d265a6fc3e8a8608e3ac37a503964658291e066a467cdd9e89f1a18d9289f6",
    "14caf7c768e7217cbd0302598f0cb18c943ad42fa06f8b337b53d2cf84c8cd12",
];

/// The `AGENTS.md` lines of earlier builds: Quadratic's, before the rename.
pub const AGENTS_LINES: &[&str] = &[
    "We use Quadratic for issues: every task is an issue there. Use the `quadratic` skill (`.agents/skills/quadratic/SKILL.md`) for anything about tasks.",
];

/// Where Quadratic wrote its skill, before the rename.
pub const QUADRATIC_SKILL_FILES: [(&str, &[&str]); 2] = [
    (".agents/skills/quadratic/SKILL.md", SKILL_MD),
    (".agents/skills/quadratic/planning.md", PLANNING_MD),
];
pub const QUADRATIC_SKILL_FOLDER: &str = ".agents/skills/quadratic";

pub fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn is_listed(text: &str, digests: &[&str]) -> bool {
    let digest = digest(text);
    digests.contains(&digest.as_str())
}

#[cfg(test)]
mod tests {
    use super::{PLANNING_MD, SKILL_MD, digest};

    #[test]
    fn every_text_this_build_writes_is_listed() {
        for (name, text, digests) in [
            (
                "SKILL_MD",
                include_str!("../../skill/quadratique/SKILL.md"),
                SKILL_MD,
            ),
            (
                "PLANNING_MD",
                include_str!("../../skill/quadratique/planning.md"),
                PLANNING_MD,
            ),
        ] {
            let digest = digest(text);
            assert!(
                digests.contains(&digest.as_str()),
                "The skill text changed: add \"{digest}\" at the end of {name} in apps/quadratique/cli/src/shipped.rs, and add the previous text to apps/quadratique/cli/tests/fixtures/skill/."
            );
        }
    }
}
