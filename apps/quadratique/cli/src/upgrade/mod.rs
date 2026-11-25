//! `quadratique upgrade`: installs the latest release over the running binary, brings the
//! server of this home to it, and says which repositories hold an older skill. The command makes
//! the network requests itself; the server makes none.
//!
//! Installing and restarting are written for Unix. Elsewhere the releases carry no archive yet,
//! so the code that only installs is compiled there but never reached.
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

mod archive;
#[cfg(unix)]
mod folder;
mod http;
mod install;
mod release;
mod route;
#[cfg(unix)]
mod server;
mod skills;
mod target;

use std::path::Path;
use std::process::Stdio;

use acid_cat_core::Kind;
use acid_cat_core::cli::{CliIo, exit};
use quadratique_server::store::{SCHEMA_VERSION, read_recorded, refuse_git_work_tree};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::background::Home;
use crate::commands::{self, print_json, report};
use release::{Release, Source, Version};
use route::Route;

pub const USAGE: &str = "upgrade [--check] [--allow-downgrade] | upgrade --skills: install the latest release of Quadratique over this binary. Finds the latest release with the GitHub API, downloads this platform's archive from github.com, checks it against the SHA-256 that the API gives for it, runs the new binary with --version in an empty environment, renames it over this one, and restarts the server of this home (through quadratique.service when systemd runs it, waiting until the new build serves; else it stops the server, so that the next command starts the new build). Any failure before the rename leaves this binary in place. It refuses a binary inside a git working tree, an install folder that other accounts can write, a store that the release cannot open, and to run as root. When mise or cargo installed this binary, it replaces nothing and prints that manager's command. It never downgrades unless given --allow-downgrade. Then it reads the main checkout of each project's repository, as init --dry-run does, and lists the repositories whose skill is older than the new build, with the command to run in each, and the skill files a person changed, with the command that prints the new text to merge into them. It writes into no repository. --check: say the installed and the latest version and list the repositories, changing nothing. --skills: only list the repositories, for this build, without the network. Neither starts a server.";

/// A release binary is about 23 MiB.
const BINARY_LIMIT: u64 = 128 << 20;
const DECODED_LIMIT: u64 = BINARY_LIMIT + (1 << 20);
const REPORT_LIMIT: std::time::Duration = std::time::Duration::from_secs(300);
/// The store schema that every release knows: release 0.1.0 shipped it.
const FIRST_RELEASE_SCHEMA: i64 = 1;

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
    #[cfg(unix)]
    if rustix::process::geteuid().is_root() {
        return report(
            io,
            Kind::InvalidInput,
            "quadratique upgrade does not run as root: run it as the user who installed Quadratique. Nothing was changed.",
        );
    }
    if request.skills {
        print_json(io, &json!({ "skills": skills::report() }));
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

/// The file as it was when this upgrade started, to tell whether another upgrade replaced it.
fn identity(binary: &Path) -> Result<(u64, u64, Option<std::time::SystemTime>), String> {
    let meta = std::fs::metadata(binary)
        .map_err(|error| format!("{} cannot be read: {error}", binary.display()))?;
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    };
    #[cfg(not(unix))]
    let file = (0, meta.len());
    Ok((file.0, file.1, meta.modified().ok()))
}

async fn run(request: &Request) -> Result<Value, String> {
    let unchanged = |what: String| format!("{what}. Nothing was changed.");
    let running = Version::parse(env!("CARGO_PKG_VERSION"))
        .ok_or("this build's version is not a release version")?;
    let binary = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| unchanged(format!("this binary's file cannot be found: {error}")))?;
    let started_as = identity(&binary)?;
    let home = commands::home().map_err(unchanged)?;
    let route = Route::of(&binary);
    if let Route::Unclear(why) = &route {
        return Err(unchanged(why.clone()));
    }
    let source = Source::chosen()?;
    let release = source
        .latest()
        .await
        .map_err(|error| unchanged(format!("The latest release could not be found: {error}")))?;
    let latest = release.version.clone();

    if latest == running || (latest < running && !request.allow_downgrade) {
        let mut answer = json!({ "upToDate": true, "installed": running.to_string() });
        if latest < running {
            answer["latest"] = json!(latest.to_string());
            answer["note"] = json!(format!(
                "This build is newer than the latest release; --allow-downgrade installs {latest}."
            ));
        }
        if route.manager().is_none() {
            finish_an_earlier_upgrade(&home, &binary, &running, request.check, &mut answer).await;
        }
        answer["skills"] = skills::report();
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
            answer["then"] = json!(after_the_manager(&route, &binary).await);
        }
        answer["run"] = json!(route.command());
        answer["skills"] = skills::report();
        return Ok(answer);
    }

    install_release(&home, &source, &release, &binary, started_as, &running).await
}

/// Installs the release over `binary`, brings the server to it, and reports the repositories.
#[cfg(unix)]
async fn install_release(
    home: &Home,
    source: &Source,
    release: &Release,
    binary: &Path,
    started_as: (u64, u64, Option<std::time::SystemTime>),
    running: &Version,
) -> Result<Value, String> {
    let unchanged = |what: String| format!("{what}. Nothing was changed.");
    let latest = &release.version;
    let Some(target) = target::THIS else {
        return Err(unchanged(format!(
            "The releases carry no archive for this platform ({})",
            target::described()
        )));
    };
    let folder = binary.parent().unwrap_or(Path::new("/"));
    refuse_git_work_tree(folder).map_err(|_| {
        unchanged(format!(
            "{} is inside a git working tree, and quadratique upgrade writes into no repository; install a release outside it",
            binary.display()
        ))
    })?;
    folder::check(folder).map_err(unchanged)?;
    store_opens(home, latest, running).map_err(unchanged)?;
    let lock = folder::Lock::take(folder).map_err(unchanged)?;
    let removed = folder::remove_leftovers(folder, &lock);
    if identity(binary)? != started_as {
        return Err(unchanged(format!(
            "{} changed while this upgrade ran, by another upgrade or install; run quadratique upgrade again",
            binary.display()
        )));
    }
    replace(source, release, target, binary, folder).await?;
    drop(lock);

    let mut answer = json!({
        "from": running.to_string(),
        "to": latest.to_string(),
        "binary": binary,
        "sha256": "verified against the GitHub API",
    });
    if !removed.is_empty() {
        answer["removedLeftovers"] = json!(removed);
    }
    match server::bring_up(home, binary, &latest.to_string()).await {
        Ok(done) => answer["server"] = json!(done),
        Err(problem) => answer["warnings"] = json!([problem]),
    }
    answer["skills"] = new_build_report(binary).await;
    Ok(answer)
}

/// The releases carry archives for Linux only; QUA-6 brings the other platforms.
#[cfg(not(unix))]
async fn install_release(
    _home: &Home,
    _source: &Source,
    _release: &Release,
    _binary: &Path,
    _started_as: (u64, u64, Option<std::time::SystemTime>),
    _running: &Version,
) -> Result<Value, String> {
    Err(format!(
        "The releases carry no archive for this platform ({}). Nothing was changed.",
        target::described()
    ))
}

/// After a run that was stopped between the rename and the restart, the binary is current while
/// the server of this home still runs the earlier build: restart it now.
#[cfg(unix)]
async fn finish_an_earlier_upgrade(
    home: &Home,
    binary: &Path,
    running: &Version,
    check: bool,
    answer: &mut Value,
) {
    let Some(pid) = server::home_server(home).await else {
        return;
    };
    if server::what_runs(pid, binary) != server::Runs::EarlierBuild {
        return;
    }
    if check {
        answer["server"] = json!(format!(
            "the server of this home (pid {pid}) runs an earlier build of {}; quadratique upgrade restarts it",
            binary.display()
        ));
        return;
    }
    match server::bring_up(home, binary, &running.to_string()).await {
        Ok(done) => answer["server"] = json!(done),
        Err(problem) => answer["warnings"] = json!([problem]),
    }
}

#[cfg(not(unix))]
async fn finish_an_earlier_upgrade(
    _home: &Home,
    _binary: &Path,
    _running: &Version,
    _check: bool,
    _answer: &mut Value,
) {
}

/// The highest store schema that release `version` is known to open. Schemas only grow, so a
/// release at least as new as this build opens every schema this build knows; an older release
/// is known to open only the schema of the first release.
fn known_schema(version: &Version, running: &Version) -> i64 {
    if version >= running {
        SCHEMA_VERSION
    } else {
        FIRST_RELEASE_SCHEMA
    }
}

/// Refuses a release that could not open the store, before anything changes.
fn store_opens(home: &Home, latest: &Version, running: &Version) -> Result<(), String> {
    let database = &home.paths.database;
    let recorded = read_recorded(database)
        .map_err(|error| format!("the store {} cannot be read: {error}", database.display()))?;
    let known = known_schema(latest, running);
    match recorded {
        Some(recorded) if recorded.schema > known => Err(format!(
            "the store {} has schema {}, and release {latest} is known to open schema {known} at most: its server could not start",
            database.display(),
            recorded.schema
        )),
        _ => Ok(()),
    }
}

/// What to run after the manager's upgrade: a service that runs a copy that `install.sh` made is
/// not upgraded by the manager.
async fn after_the_manager(route: &Route, binary: &Path) -> Vec<String> {
    let mut then = Vec::new();
    #[cfg(unix)]
    let service = server::service_binary().await;
    #[cfg(not(unix))]
    let service: Option<std::path::PathBuf> = None;
    if let Some(service) = service.filter(|service| service != binary) {
        then.push(match route {
            Route::Mise { tool } => format!(
                "sh \"$(mise where {tool})/install.sh\" \"$(mise which quadratique)\""
            ),
            _ => format!(
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
/// as it was and no staging folder is left.
#[cfg(unix)]
async fn replace(
    source: &Source,
    release: &Release,
    target: &str,
    binary: &Path,
    folder: &Path,
) -> Result<(), String> {
    let unchanged = |what: String| format!("{what}. Nothing was changed.");
    let latest = release.version.to_string();
    let files = target::files(&latest, target);
    let asset = release.assets.get(&files.archive).ok_or_else(|| {
        unchanged(format!(
            "Release {latest} has no {} for this platform",
            files.archive
        ))
    })?;
    let expected = asset.sha256.as_deref().ok_or_else(|| {
        unchanged(format!(
            "The GitHub API gives no SHA-256 digest for {}",
            files.archive
        ))
    })?;
    let archive = source
        .archive(asset)
        .await
        .map_err(|error| unchanged(format!("The download of {} failed: {error}", files.archive)))?;
    let digest = hex(&Sha256::digest(&archive));
    if digest != expected {
        return Err(unchanged(format!(
            "{} does not match the SHA-256 that the GitHub API gives for it: it is {digest}, not {expected}",
            files.archive
        )));
    }
    let bytes = archive::binary(&archive, &files.binary, BINARY_LIMIT, DECODED_LIMIT)
        .map_err(|error| unchanged(format!("{} is refused: {error}", files.archive)))?;
    let staging = folder::Staging::create(folder, &bytes).map_err(unchanged)?;
    install::runs_as(&staging.binary, &latest)
        .await
        .map_err(unchanged)?;
    staging.install(binary).map_err(unchanged)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The new build knows its own skill text, so the new binary makes the report. It reads the
/// store and starts no server.
async fn new_build_report(binary: &Path) -> Value {
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
    use super::{FIRST_RELEASE_SCHEMA, SCHEMA_VERSION, Version, known_schema, request};

    #[test]
    fn a_newer_release_opens_this_build_s_schema_and_an_older_one_only_the_first_release_s() {
        let running = Version::parse("1.4.0").unwrap();
        let newer = Version::parse("1.5.0").unwrap();
        let older = Version::parse("1.3.9").unwrap();
        assert_eq!(known_schema(&newer, &running), SCHEMA_VERSION);
        assert_eq!(known_schema(&running, &running), SCHEMA_VERSION);
        assert_eq!(known_schema(&older, &running), FIRST_RELEASE_SCHEMA);
    }

    #[test]
    fn skills_stands_alone_and_a_flag_is_given_once() {
        assert!(request(&["--check", "--allow-downgrade"]).is_some());
        assert!(request(&["--skills", "--check"]).is_none());
        assert!(request(&["--check", "--check"]).is_none());
        assert!(request(&["--force"]).is_none());
    }
}
