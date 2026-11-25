//! The platform of this build, as the release archives name it, and the names of its files in a
//! release. A new platform is a new arm of `THIS`, and a new archive format a change here only.

/// The target triple of this build when the releases carry an archive for it.
pub const THIS: Option<&str> = if cfg!(all(
    target_arch = "x86_64",
    target_os = "linux",
    target_env = "gnu"
)) {
    Some("x86_64-unknown-linux-gnu")
} else if cfg!(all(
    target_arch = "aarch64",
    target_os = "linux",
    target_env = "gnu"
)) {
    Some("aarch64-unknown-linux-gnu")
} else {
    None
};

/// The name of this build's target, for a message when the releases carry none for it.
pub fn described() -> String {
    format!(
        "{}-{}{}",
        std::env::consts::ARCH,
        std::env::consts::OS,
        if cfg!(target_env = "gnu") { "-gnu" } else { "" }
    )
}

pub struct Files {
    pub archive: String,
    /// The binary's path inside the archive.
    pub binary: String,
}

pub fn files(version: &str, target: &str) -> Files {
    let folder = format!("quadratique-{version}-{target}");
    let archive = format!("{folder}.tar.gz");
    Files {
        archive,
        binary: format!("{folder}/bin/quadratique"),
    }
}
