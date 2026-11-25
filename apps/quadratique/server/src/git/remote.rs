//! The web address of a project's repository, read from its local git configuration only.
//!
//! A remote URL may carry a user name or a token. The address is rebuilt from the host and the
//! owner and repository names alone, so nothing else of the URL reaches an answer; the URL itself
//! never leaves this module, not even in an error.

use super::repository::Repository;

const GITHUB: &str = "github.com";
const DEFAULT_REMOTE: &str = "origin";

/// The https address of the github.com repository that the remote of `default_branch` names: the
/// remote that the branch tracks, else `origin`. `None` for any other host, no remote, or a URL
/// this module cannot read.
pub async fn web_address(repository: &Repository, default_branch: &str) -> Option<String> {
    let tracked = repository
        .line(&[
            "config",
            "--get",
            &format!("branch.{default_branch}.remote"),
        ])
        .await;
    let remote = tracked.as_deref().unwrap_or(DEFAULT_REMOTE);
    let url = repository
        .line(&["remote", "get-url", "--end-of-options", remote])
        .await?;
    github_address(&url)
}

/// The https address that a remote URL of github.com names, in any of the forms git accepts:
/// `https://`, `ssh://` and scp-like `git@github.com:owner/repo.git`.
pub fn github_address(url: &str) -> Option<String> {
    let url = url.trim();
    let (authority, path) = match url.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            if !matches!(
                scheme.as_str(),
                "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git"
            ) {
                return None;
            }
            rest.split_once('/')?
        }
        None => {
            let (authority, path) = url.split_once(':')?;
            if authority.contains('/') {
                return None;
            }
            (authority, path)
        }
    };
    let host_and_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host_and_port
        .split_once(':')
        .map_or(host_and_port, |(host, _)| host);
    if !host.eq_ignore_ascii_case(GITHUB) {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    (is_name(owner) && is_name(name)).then(|| format!("https://{GITHUB}/{owner}/{name}"))
}

fn is_name(text: &str) -> bool {
    !text.is_empty()
        && text != "."
        && text != ".."
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
}

/// `<web address>/pull/<branch>`: github.com redirects it to the branch's pull request.
pub fn pull_request(web_address: &str, branch: &str) -> String {
    format!("{web_address}/pull/{}", path_of(branch))
}

pub fn commit(web_address: &str, git_commit: &str) -> String {
    format!("{web_address}/commit/{git_commit}")
}

fn path_of(branch: &str) -> String {
    let mut path = String::with_capacity(branch.len());
    for byte in branch.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            path.push(char::from(byte));
        } else {
            path.push_str(&format!("%{byte:02X}"));
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::{github_address, pull_request};

    const ADDRESS: &str = "https://github.com/Hebilicious/quadratique";

    #[test]
    fn https_ssh_and_scp_like_remotes_give_the_same_https_address() {
        for url in [
            "https://github.com/Hebilicious/quadratique.git",
            "https://github.com/Hebilicious/quadratique",
            "https://github.com/Hebilicious/quadratique/",
            "http://github.com/Hebilicious/quadratique.git",
            "ssh://git@github.com/Hebilicious/quadratique.git",
            "ssh://git@github.com:22/Hebilicious/quadratique.git",
            "git+ssh://git@github.com/Hebilicious/quadratique",
            "git@github.com:Hebilicious/quadratique.git",
            "github.com:Hebilicious/quadratique",
            "  git@GitHub.com:Hebilicious/quadratique.git\n",
        ] {
            assert_eq!(github_address(url).as_deref(), Some(ADDRESS), "{url}");
        }
    }

    #[test]
    fn a_user_name_or_a_token_in_the_remote_never_reaches_the_address() {
        for url in [
            "https://made-up-user:ghp_MadeUpToken123@github.com/Hebilicious/quadratique.git",
            "https://ghp_MadeUpToken123@github.com/Hebilicious/quadratique.git",
            "https://x-access-token:p@ss@github.com/Hebilicious/quadratique.git",
            "ssh://deploy:ghp_MadeUpToken123@github.com:22/Hebilicious/quadratique.git",
        ] {
            assert_eq!(github_address(url).as_deref(), Some(ADDRESS), "{url}");
        }
    }

    #[test]
    fn a_token_the_parser_cannot_place_gives_no_address_rather_than_leak() {
        for url in [
            "https://user:pa/ss@github.com/Hebilicious/quadratique.git",
            "https://github.com/Hebilicious/quadratique.git?token=ghp_MadeUpToken123",
            "https://github.com/Hebilicious/quadratique.git#ghp_MadeUpToken123",
        ] {
            assert_eq!(github_address(url), None, "{url}");
        }
    }

    #[test]
    fn other_hosts_local_paths_and_odd_paths_give_no_address() {
        for url in [
            "https://gitlab.com/Hebilicious/quadratique.git",
            "git@codeberg.org:Hebilicious/quadratique.git",
            "https://github.com.evil.example/Hebilicious/quadratique.git",
            "https://evil.example/github.com/quadratique.git",
            "/home/heb/GitHub/quadratique",
            "../quadratique.git",
            "file:///home/heb/GitHub/quadratique",
            "C:/repositories/quadratique",
            "https://github.com/Hebilicious",
            "https://github.com/Hebilicious/quadratique/pull/7",
            "https://github.com/../quadratique",
            "",
        ] {
            assert_eq!(github_address(url), None, "{url}");
        }
    }

    #[test]
    fn a_branch_is_kept_whole_in_the_pull_request_address_with_its_other_characters_escaped() {
        assert_eq!(
            pull_request(ADDRESS, "feature/QUA-24 links#1"),
            format!("{ADDRESS}/pull/feature/QUA-24%20links%231")
        );
    }
}
