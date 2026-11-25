//! The latest release, as the GitHub API describes it, and the comparison of versions.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use serde_json::Value;

use super::http::{self, Limits, PlainOrigin};

const GITHUB_API: &str = "https://api.github.com";
const REPOSITORY: &str = "Hebilicious/quadratique";

const DESCRIPTION_LIMITS: Limits = Limits {
    bytes: 1 << 20,
    time: Duration::from_secs(60),
};
const CHECKSUM_LIMITS: Limits = Limits {
    bytes: 4 << 10,
    time: Duration::from_secs(60),
};
const ARCHIVE_LIMITS: Limits = Limits {
    bytes: 256 << 20,
    time: Duration::from_secs(15 * 60),
};

/// Where the releases are described. A release build always asks GitHub over HTTPS. A debug
/// build, which the tests run, takes another source from `QUADRATIQUE_UPGRADE_SOURCE`, and
/// reaches it over plain HTTP only on the loopback interface. No release build reads that
/// variable, so neither a repository's files nor a tool's output can point a released binary at
/// another source.
pub struct Source {
    api: String,
    plain: Option<PlainOrigin>,
}

#[cfg(debug_assertions)]
const SOURCE_VARIABLE: &str = "QUADRATIQUE_UPGRADE_SOURCE";

impl Source {
    pub fn chosen() -> Result<Source, String> {
        #[cfg(debug_assertions)]
        if let Some(api) = std::env::var_os(SOURCE_VARIABLE) {
            let api = api
                .into_string()
                .map_err(|_| format!("{SOURCE_VARIABLE} is not UTF-8 text"))?;
            return Source::debug(api);
        }
        Ok(Source {
            api: GITHUB_API.to_owned(),
            plain: None,
        })
    }

    #[cfg(debug_assertions)]
    fn debug(api: String) -> Result<Source, String> {
        let uri: ::http::Uri = api
            .parse()
            .map_err(|error| format!("{SOURCE_VARIABLE} is not a URL: {error}"))?;
        let plain = match (uri.scheme_str(), uri.host(), uri.port_u16()) {
            (Some("https"), _, _) => None,
            (Some("http"), Some(host @ ("127.0.0.1" | "localhost")), Some(port)) => {
                Some(PlainOrigin {
                    host: host.to_owned(),
                    port,
                })
            }
            _ => {
                return Err(format!(
                    "{SOURCE_VARIABLE} is an HTTPS URL, or plain HTTP on 127.0.0.1 with a port"
                ));
            }
        };
        Ok(Source {
            api: api.trim_end_matches('/').to_owned(),
            plain,
        })
    }

    async fn get(&self, url: &str, accept: &str, limits: &Limits) -> Result<Bytes, String> {
        http::get(url, accept, limits, self.plain.as_ref()).await
    }

    pub async fn latest(&self) -> Result<Release, String> {
        let url = format!("{}/repos/{REPOSITORY}/releases/latest", self.api);
        let body = self
            .get(&url, "application/vnd.github+json", &DESCRIPTION_LIMITS)
            .await?;
        let description: Value =
            serde_json::from_slice(&body).map_err(|error| format!("{url} is not JSON: {error}"))?;
        Release::from_description(&description).ok_or_else(|| {
            format!("{url} describes no release with a version tag (v1.2.3) and its assets")
        })
    }

    pub async fn checksum(&self, url: &str) -> Result<Bytes, String> {
        self.get(url, "application/octet-stream", &CHECKSUM_LIMITS)
            .await
    }

    pub async fn archive(&self, url: &str) -> Result<Bytes, String> {
        self.get(url, "application/octet-stream", &ARCHIVE_LIMITS)
            .await
    }
}

pub struct Release {
    pub version: Version,
    /// The download address of each asset, by its name.
    pub assets: BTreeMap<String, String>,
}

impl Release {
    fn from_description(description: &Value) -> Option<Release> {
        let tag = description["tag_name"].as_str()?;
        let version = Version::parse(tag.strip_prefix('v')?)?;
        let assets = description["assets"]
            .as_array()?
            .iter()
            .filter_map(|asset| {
                Some((
                    asset["name"].as_str()?.to_owned(),
                    asset["browser_download_url"].as_str()?.to_owned(),
                ))
            })
            .collect();
        Some(Release { version, assets })
    }
}

/// A version as release-please writes it: `1.2.3`, with an optional pre-release after `-`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    numbers: [u64; 3],
    pre: Option<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let numbers: Vec<u64> = core
            .split('.')
            .map(|part| {
                (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| part.parse().ok())
                    .flatten()
            })
            .collect::<Option<_>>()?;
        let numbers: [u64; 3] = numbers.try_into().ok()?;
        if let Some(pre) = pre
            && (pre.is_empty()
                || !pre
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.'))
        {
            return None;
        }
        Some(Version {
            numbers,
            pre: pre.map(str::to_owned),
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [major, minor, patch] = self.numbers;
        write!(formatter, "{major}.{minor}.{patch}")?;
        match &self.pre {
            Some(pre) => write!(formatter, "-{pre}"),
            None => Ok(()),
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Pre-releases are ordered by their text, which is enough for the releases of one project.
impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        self.numbers
            .cmp(&other.numbers)
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(mine), Some(theirs)) => mine.cmp(theirs),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::Version;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn versions_compare_by_their_numbers_and_a_pre_release_comes_before_its_release() {
        assert!(version("0.10.0") > version("0.9.9"));
        assert!(version("1.0.0") > version("1.0.0-rc.1"));
        assert_eq!(version("0.1.0").to_string(), "0.1.0");
        for refused in [
            "1.0",
            "1.0.0.0",
            "1.x.0",
            "1.0.0-",
            "../1.0.0",
            "1.0.0-a/b",
            "",
        ] {
            assert!(Version::parse(refused).is_none(), "{refused}");
        }
    }
}
