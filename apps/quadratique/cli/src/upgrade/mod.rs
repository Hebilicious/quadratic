//! `quadratique upgrade`: installs the latest release over the running binary, restarts the
//! server, and says which repositories hold an older skill. The command makes the network
//! requests itself; the server makes none.

mod archive;
mod http;
mod install;
mod release;
mod route;
mod skills;
mod target;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use acid_cat_core::Kind;
use acid_cat_core::cli::{CliIo, exit};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::commands::{self, print_json, report};
use install::Staged;
use release::{Release, Source, Version};
use route::Route;

pub const USAGE: &str = "upgrade [--check] [--allow-downgrade] | upgrade --skills: install the latest release of Quadratique over this binary. Finds the latest release on GitHub, downloads this platform's archive and its SHA-256 file over HTTPS, checks the archive against it, runs the new binary with --version, renames it over this one, and restarts the server (the systemd user service quadratique.service where install.sh set it up, else stops the server so that the next command starts the new build). Any failure before the rename leaves this binary in place. When mise or cargo installed this binary, it replaces nothing and prints that manager's command. It never downgrades unless given --allow-downgrade, and refuses to run as root. Then it reads the main checkout of each project's repository, as init --dry-run does, and lists the repositories whose skill is older than the new build, with the command to run in each, and the skill files a person changed, with the command that prints the new text to merge into them. It writes into no repository. --check: say the installed and the latest version and list the repositories, changing nothing. --skills: only list the repositories, for this build, without the network.";

const BINARY_LIMIT: u64 = 256 << 20;
const REPORT_LIMIT: std::time::Duration = std::time::Duration::from_secs(300);

#[derive(Default)]
struct Request {
    check: bool,
    allow_downgrade: bool,
    skills: bool,
}

fn request(words: &[&str]) -> Option<Request> {
    let mut request = Request::default();
    for word in words {
        let flag = match *word {
            "--check" => &mut request.check,
            "--allow-downgrade" => &mut request.allow_downgrade,
            "--skills" => &mut request.skills,
            _ => return None,
        };
        if *flag {
            return None;
        }
        *flag = true;
    }
    (!request.skills || !(request.check || request.allow_downgrade)).then_some(request)
}

pub async fn upgrade(words: &[&str], io: &mut CliIo) -> i32 {
    let Some(request) = request(words) else {
        return report(io, Kind::InvalidInput, format!("Give: quadratique {USAGE}"));
    };
    if rustix::process::geteuid().is_root() {
        return report(
            io,
            Kind::InvalidInput,
            "quadratique upgrade does not run as root: run it as the user who installed Quadratique. Nothing was changed.",
        );
    }
    if request.skills {
        let answer = json!({ "skills": skill_report().await });
        print_json(io, &answer);
        return exit::VALUE;
    }
    match run(&request).await {
        Ok(answer) => {
            print_json(io, &answer);
            exit::VALUE
        }
        Err(failure) => report(io, Kind::Unavailable, failure),
    }
}

/// The report of the repositories, or why it could not be made and how to make it later.
async fn skill_report() -> Value {
    match skills::report().await {
        Ok(report) => report,
        Err(problem) => json!({
            "error": format!("the repositories could not be read: {problem}"),
            "run": "quadratique upgrade --skills",
        }),
    }
}

async fn run(request: &Request) -> Result<Value, String> {
    let running = Version::parse(env!("CARGO_PKG_VERSION"))
        .ok_or("this build's version is not a release version")?;
    let binary = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("this binary's file cannot be found: {error}"))?;
    let route = Route::of(&binary);
    let source = Source::chosen()?;
    let release = source.latest().await.map_err(|error| {
        format!("The latest release could not be found: {error}. Nothing was changed.")
    })?;
    let latest = &release.version;

    if *latest == running || (*latest < running && !request.allow_downgrade) {
        let mut answer = json!({ "upToDate": true, "installed": running.to_string() });
        if *latest < running {
            answer["latest"] = json!(latest.to_string());
            answer["note"] = json!(format!(
                "This build is newer than the latest release; --allow-downgrade installs {latest}."
            ));
        }
        answer["skills"] = skill_report().await;
        return Ok(answer);
    }
    let mut answer = json!({
        "upToDate": false,
        "installed": running.to_string(),
        "latest": latest.to_string(),
    });
    if request.check || route.manager().is_some() {
        if let Some(manager) = route.manager() {
            answer["managedBy"] = json!(manager);
            answer["binary"] = json!(binary);
        }
        answer["run"] = json!(route.command());
        if route.manager().is_some() {
            answer["then"] = json!(after_the_manager(&route, &binary));
        }
        answer["skills"] = skill_report().await;
        return Ok(answer);
    }

    replace(&source, &release, &binary).await?;
    let mut answer = json!({
        "from": running.to_string(),
        "to": latest.to_string(),
        "binary": binary,
        "checksum": "verified",
    });
    let home = commands::home()?;
    match install::restart(&home, &binary).await {
        Ok(done) => answer["server"] = json!(done),
        Err(problem) => answer["warnings"] = json!([problem]),
    }
    answer["skills"] = new_build_report(&binary).await;
    Ok(answer)
}

/// What to run after the manager's upgrade: the service's binary is a copy that `install.sh`
/// made, which the manager does not upgrade.
fn after_the_manager(route: &Route, binary: &Path) -> Vec<String> {
    let mut then = Vec::new();
    if let Some(service) = install::service_binary().filter(|service| service != binary) {
        then.push(match route {
            Route::Mise { tool } => format!(
                "sh \"$(mise where {tool})/install.sh\" \"$(mise which quadratique)\""
            ),
            Route::Cargo | Route::File => format!(
                "The service quadratique.service runs {}, a copy that install.sh made: run the install.sh of the new release with {}",
                service.display(),
                binary.display()
            ),
        });
    }
    then.push("quadratique upgrade --skills".to_owned());
    then
}

/// Downloads, checks and installs the release's binary over `binary`. On an error, `binary` is
/// as it was.
async fn replace(source: &Source, release: &Release, binary: &Path) -> Result<(), String> {
    let latest = release.version.to_string();
    let Some(target) = target::THIS else {
        return Err(format!(
            "The releases carry no archive for this platform ({}). Nothing was changed.",
            target::described()
        ));
    };
    let files = target::files(&latest, target);
    let asset = |name: &str| {
        release.assets.get(name).ok_or_else(|| {
            format!("Release {latest} has no {name} for this platform. Nothing was changed.")
        })
    };
    let (archive_url, checksum_url) = (asset(&files.archive)?, asset(&files.checksum)?);
    let unchanged = |what: String| format!("{what}. Nothing was changed.");
    let checksum = source.checksum(checksum_url).await.map_err(|error| {
        unchanged(format!(
            "The download of {} failed: {error}",
            files.checksum
        ))
    })?;
    let expected = expected_digest(&checksum, &files.archive).map_err(unchanged)?;
    let archive = source
        .archive(archive_url)
        .await
        .map_err(|error| unchanged(format!("The download of {} failed: {error}", files.archive)))?;
    let digest = hex(&Sha256::digest(&archive));
    if digest != expected {
        return Err(unchanged(format!(
            "{} does not match its checksum: its SHA-256 is {digest}, {} says {expected}",
            files.archive, files.checksum
        )));
    }
    let bytes = archive::binary(&archive, &files.binary, BINARY_LIMIT)
        .map_err(|error| unchanged(format!("{} is refused: {error}", files.archive)))?;
    let staged = Staged::write(&bytes, binary).map_err(unchanged)?;
    staged.runs_as(&latest).await.map_err(unchanged)?;
    staged.install().map_err(unchanged)
}

/// The SHA-256 that a `sha256sum` line gives for `archive`.
fn expected_digest(checksum: &[u8], archive: &str) -> Result<String, String> {
    let text = std::str::from_utf8(checksum).unwrap_or_default();
    let mut words = text.split_whitespace();
    let digest = words.next().unwrap_or_default().to_ascii_lowercase();
    let named = words.next().map(|name| name.trim_start_matches('*'));
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("The checksum file of {archive} holds no SHA-256"));
    }
    if named.is_some_and(|name| name != archive) {
        return Err(format!("The checksum file of {archive} names another file"));
    }
    Ok(digest)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The new build knows its own skill text, so the new binary makes the report.
async fn new_build_report(binary: &PathBuf) -> Value {
    let mut command = tokio::process::Command::new(binary);
    command
        .args(["upgrade", "--skills"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let failed = |problem: String| {
        json!({
            "error": format!("the new build could not read the repositories: {problem}"),
            "run": "quadratique upgrade --skills",
        })
    };
    let output = match tokio::time::timeout(REPORT_LIMIT, command.output()).await {
        Ok(output) => output,
        Err(_) => {
            return failed(format!(
                "no answer within {} seconds",
                REPORT_LIMIT.as_secs()
            ));
        }
    };
    match output {
        Ok(output) if output.status.success() => {
            match serde_json::from_slice::<Value>(&output.stdout) {
                Ok(answer) => answer["skills"].clone(),
                Err(error) => failed(error.to_string()),
            }
        }
        Ok(output) => failed(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
        Err(error) => failed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{expected_digest, request};

    #[test]
    fn a_checksum_line_gives_the_digest_of_the_archive_it_names() {
        let digest = "ab".repeat(32);
        assert_eq!(
            expected_digest(format!("{digest}  a.tar.gz\n").as_bytes(), "a.tar.gz").unwrap(),
            digest
        );
        assert!(expected_digest(format!("{digest}  b.tar.gz\n").as_bytes(), "a.tar.gz").is_err());
        assert!(expected_digest(b"<html>not found</html>", "a.tar.gz").is_err());
    }

    #[test]
    fn skills_stands_alone_and_a_flag_is_given_once() {
        assert!(request(&["--check", "--allow-downgrade"]).is_some());
        assert!(request(&["--skills", "--check"]).is_none());
        assert!(request(&["--check", "--check"]).is_none());
        assert!(request(&["--force"]).is_none());
    }
}
