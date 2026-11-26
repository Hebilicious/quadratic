//! The platform of this build, as the release archives name it, and the names of its files in a
//! release. A new platform is a new arm of `released`, and a new archive format a change here only.

/// The target triple of this build when the releases carry an archive for it.
pub const THIS: Option<&str> = released(
    std::env::consts::ARCH,
    std::env::consts::OS,
    cfg!(target_env = "gnu"),
);

/// The target triple of the release archive for a machine of this architecture and system, with
/// GNU's C library or not; none when the releases carry no archive for it.
const fn released(arch: &str, os: &str, gnu: bool) -> Option<&'static str> {
    match (arch.as_bytes(), os.as_bytes(), gnu) {
        (b"x86_64", b"linux", true) => Some("x86_64-unknown-linux-gnu"),
        (b"aarch64", b"linux", true) => Some("aarch64-unknown-linux-gnu"),
        (b"x86_64", b"macos", _) => Some("x86_64-apple-darwin"),
        (b"aarch64", b"macos", _) => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

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

#[cfg(test)]
mod tests {
    use super::{THIS, files, released};

    #[test]
    fn each_released_platform_names_its_archive_and_the_others_none() {
        let archive =
            |arch, os, gnu| released(arch, os, gnu).map(|target| files("1.2.3", target).archive);
        assert_eq!(
            archive("x86_64", "linux", true).as_deref(),
            Some("quadratique-1.2.3-x86_64-unknown-linux-gnu.tar.gz")
        );
        assert_eq!(
            archive("aarch64", "linux", true).as_deref(),
            Some("quadratique-1.2.3-aarch64-unknown-linux-gnu.tar.gz")
        );
        assert_eq!(
            archive("aarch64", "macos", false).as_deref(),
            Some("quadratique-1.2.3-aarch64-apple-darwin.tar.gz")
        );
        assert_eq!(
            archive("x86_64", "macos", false).as_deref(),
            Some("quadratique-1.2.3-x86_64-apple-darwin.tar.gz")
        );
        assert_eq!(archive("x86_64", "linux", false), None, "musl");
        assert_eq!(archive("x86_64", "windows", false), None);
        assert_eq!(archive("riscv64", "linux", true), None);
        assert_eq!(
            files("1.2.3", "aarch64-apple-darwin").binary,
            "quadratique-1.2.3-aarch64-apple-darwin/bin/quadratique"
        );
    }

    /// Run on each platform of CI: a build for a released platform finds its own archive.
    #[test]
    fn this_build_names_the_archive_of_its_platform() {
        let expected = if cfg!(all(
            target_os = "linux",
            target_env = "gnu",
            target_arch = "x86_64"
        )) {
            Some("x86_64-unknown-linux-gnu")
        } else if cfg!(all(
            target_os = "linux",
            target_env = "gnu",
            target_arch = "aarch64"
        )) {
            Some("aarch64-unknown-linux-gnu")
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some("aarch64-apple-darwin")
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            Some("x86_64-apple-darwin")
        } else {
            None
        };
        assert_eq!(THIS, expected);
    }
}
