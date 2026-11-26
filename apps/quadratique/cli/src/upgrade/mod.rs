//! `quadratique upgrade`: installs the latest release over the running binary, stops the
//! server of this home that ran the replaced file, and says which repositories hold an older skill. The command makes
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

/// The legacy stop (`background::legacy`) counts the installed binary only in a folder that this rule accepts.
#[cfg(target_os = "linux")]
pub use folder::check as check_install_folder;

use std::path::Path;
use std::process::Stdio;

use acid_cat_core::Kind;
use acid_cat_core::cli::{CliIo, exit};
use quadratique_server::store::{SCHEMA_VERSION, read_recorded, refuse_git_work_tree};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::background::{self, Home};
use crate::commands::{self, print_json, report};
use release::{Release, Source, Version};
use route::Route;

pub const USAGE: &str = "upgrade [--check] [--allow-downgrade] | upgrade --skills: install the latest release of Quadratique over this binary. Finds the latest release with the GitHub API, downloads this platform's archive from github.com, checks it against the SHA-256 that the API gives for it, runs the new binary with --version in an empty environment, renames it over this one, and stops the server of this home if it ran the replaced file: before the rename it reads, on the home's socket, the server's generation and the file it runs, and after it asks that generation to stop, so that the next command starts the new build. When quadratique.service ran that server, it then starts the unit again and waits until the new build serves; a server started with quadratique serve is stopped, and the person restarts it. A server that runs another file is left running, and a server from before the socket (release 0.1) gets what to stop. Any failure before the rename leaves this binary in place. It refuses a binary inside a git working tree, an install folder that other accounts can write, a store that the release cannot open, and to run as root. When mise or cargo installed this binary, it replaces nothing and prints that manager's command. It never downgrades unless given --allow-downgrade. Then it reads the main checkout of each project's repository, as init --dry-run does, and lists the repositories whose skill is older than the new build, with the command to run in each, and the skill files a person changed, with the command that prints the new text to merge into them. It writes into no repository. --check: say the installed and the latest version and list the repositories, changing nothing. --skills: only list the repositories, for this build, without the network. Neither starts a server: the home's server lists the projects when it runs; otherwise the command reads the store file once, read-only, and the store engine may leave its lock file (.workspace.db-lock) beside it.";

/// A release binary is about 23 MiB.
const BINARY_LIMIT: u64 = 128 << 20;
const DECODED_LIMIT: u64 = BINARY_LIMIT + (1 << 20);
const REPORT_LIMIT: std::time::Duration = std::time::Duration::from_secs(300);
/// The first release that opened each store schema, oldest first. A build that adds a migration
/// adds its row with the version it will be released as; a test fails until the build's schema
/// is here.
const SCHEMA_RELEASES: &[(i64, &str)] = &[(1, "0.1.0"), (2, "0.2.0")];

/// The file at the binary's path: device, inode, size and modification time.
type Identity = (u64, u64, u64, Option<std::time::SystemTime>);

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
        print_json(io, &json!({ "skills": skills::report().await }));
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
fn identity(binary: &Path) -> Result<Identity, String> {
    let meta = std::fs::metadata(binary)
        .map_err(|error| format!("{} cannot be read: {error}", binary.display()))?;
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    };
    #[cfg(not(unix))]
    let file = (0, 0);
    Ok((file.0, file.1, meta.len(), meta.modified().ok()))
}

async fn run(request: &Request) -> Result<Value, String> {
    let unchanged = |what: String| format!("{what}. Nothing was changed.");
    let running = Version::parse(env!("CARGO_PKG_VERSION"))
        .ok_or("this build's version is not a release version")?;
    let binary = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| unchanged(format!("this binary's file cannot be found: {error}")))?;
    let started_as = identity(&binary)?;
    let home = commands::existing_home().map_err(unchanged)?;
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
        if let (None, Some(home)) = (route.manager(), &home) {
            finish_an_earlier_upgrade(home, &binary, &running, request.check, &mut answer).await;
        }
        answer["skills"] = skills::report().await;
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
        answer["skills"] = skills::report().await;
        return Ok(answer);
    }

    let home = match home {
        Some(home) => home,
        None => commands::home().map_err(|error| unchanged(error.to_string()))?,
    };
    install_release(&home, &source, &release, &binary, started_as, &running).await
}

/// Installs the release over `binary`, brings the server to it, and reports the repositories.
#[cfg(unix)]
async fn install_release(
    home: &Home,
    source: &Source,
    release: &Release,
    binary: &Path,
    started_as: Identity,
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
    store_opens(home, latest, running)
        .await
        .map_err(unchanged)?;
    let mut lock = folder::Lock::take(folder).map_err(unchanged)?;
    // From here on the lock file exists and leftovers may be gone: only the binary is known to be as it was.
    let kept = |what: String| format!("{what}. The installed binary was not changed.");
    if identity(binary)? != started_as {
        return Err(kept(format!(
            "{} changed while this upgrade ran, by another upgrade or install; run quadratique upgrade again",
            binary.display()
        )));
    }
    let leftovers = folder::remove_leftovers(folder, &lock);
    let staging = prepare(source, release, target, folder).await?;
    // No server may start between the last look and the stop: `start.lock` is held from before the look through the
    // rename and the stop, and given up before a service's own start.
    let start = background::reserve_start(home)
        .await
        .map_err(|error| kept(error.to_string()))?;
    let before = store_opens(home, latest, running).await.map_err(kept)?;
    let replaced = (started_as.0, started_as.1);
    lock.record_replaced(replaced).map_err(kept)?;
    staging.install(binary).map_err(kept)?;

    let mut answer = json!({
        "from": running.to_string(),
        "to": latest.to_string(),
        "binary": binary,
        "sha256": "verified against the GitHub API",
    });
    if !leftovers.removed.is_empty() {
        answer["removedLeftovers"] = json!(leftovers.removed);
    }
    if !leftovers.left.is_empty() {
        answer["leftBesideTheBinary"] = json!(leftovers.left);
    }
    match server::bring_up(home, binary, &latest.to_string(), replaced, before, start).await {
        Ok(done) => answer["server"] = json!(done),
        Err(problem) => answer["warnings"] = json!([problem]),
    }
    answer["skills"] = new_build_report(binary).await;
    drop(lock);
    Ok(answer)
}

/// The releases carry archives for Linux only; QUA-6 brings the other platforms.
#[cfg(not(unix))]
async fn install_release(
    _home: &Home,
    _source: &Source,
    _release: &Release,
    _binary: &Path,
    _started_as: Identity,
    _running: &Version,
) -> Result<Value, String> {
    Err(format!(
        "The releases carry no archive for this platform ({}). Nothing was changed.",
        target::described()
    ))
}

/// After a run that was stopped between the rename and the stop, the binary is current while the server of this home
/// may still run the replaced file: stop it now. The record of the replaced file counts only in an install folder that
/// no other account can change, and only for a server that says, over the socket, that it was started from this path
/// and from that file. With `check`, nothing is created or stopped: the record is read without the lock.
#[cfg(unix)]
async fn finish_an_earlier_upgrade(
    home: &Home,
    binary: &Path,
    running: &Version,
    check: bool,
    answer: &mut Value,
) {
    let Some(folder) = binary.parent() else {
        return;
    };
    let note = |answer: &mut Value, problem: String| {
        if check {
            answer["server"] = json!(problem);
        } else {
            answer["warnings"] = json!([problem]);
        }
    };
    if let Err(problem) = folder::check(folder) {
        note(
            answer,
            format!("the record of an earlier upgrade was not read: {problem}"),
        );
        return;
    }
    if folder::recorded_replaced(folder).is_none() {
        return;
    }
    let (replaced, _lock, start) = if check {
        let Some(replaced) = folder::recorded_replaced(folder) else {
            return;
        };
        (replaced, None, None)
    } else {
        let mut lock = match folder::Lock::take(folder) {
            Ok(lock) => lock,
            Err(problem) => {
                note(answer, problem);
                return;
            }
        };
        let Some(replaced) = lock.replaced() else {
            return;
        };
        let start = match background::reserve_start(home).await {
            Ok(start) => start,
            Err(error) => {
                note(answer, error.to_string());
                return;
            }
        };
        (replaced, Some(lock), Some(start))
    };
    let seen = match server::look(home).await {
        Ok(seen) => seen,
        Err(problem) => {
            note(
                answer,
                format!("the server of this home cannot be reached: {problem}"),
            );
            return;
        }
    };
    let identity = match &seen.before {
        server::Before::Socket(identity) => identity,
        server::Before::Nobody => {
            drop(start);
            if let Some(stopped) = server::service_not_running(binary).await {
                note(answer, stopped);
            }
            return;
        }
        _ => return,
    };
    if !server::left_on_the_replaced_file(identity, binary, replaced) {
        return;
    }
    let Some(start) = start else {
        answer["server"] = json!(format!(
            "the server of this home (pid {}) runs the file that an earlier upgrade replaced; quadratique upgrade stops it",
            identity.pid
        ));
        return;
    };
    match server::bring_up(
        home,
        binary,
        &running.to_string(),
        replaced,
        seen.before,
        start,
    )
    .await
    {
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
/// opens the schemas whose first release it is at or after.
fn known_schema(version: &Version, running: &Version) -> i64 {
    known_schema_in(SCHEMA_RELEASES, version, running, SCHEMA_VERSION)
}

fn known_schema_in(
    table: &[(i64, &str)],
    version: &Version,
    running: &Version,
    current: i64,
) -> i64 {
    if version >= running {
        return current;
    }
    table
        .iter()
        .filter(|(_, first)| Version::parse(first).is_some_and(|first| first <= *version))
        .map(|(schema, _)| *schema)
        .max()
        .unwrap_or(0)
}

/// What served the home, and the store's schema: from the home's server when one answers on the socket, so that the
/// command opens no store file; else from the file, read once; `None` when there is no store.
#[cfg(unix)]
async fn store_schema(home: &Home) -> Result<(server::Before, Option<i64>), String> {
    let seen = server::look(home).await?;
    if seen.schema.is_some() {
        return Ok((seen.before, seen.schema));
    }
    Ok((seen.before, recorded_schema(home)?))
}

#[cfg(not(unix))]
async fn store_schema(home: &Home) -> Result<((), Option<i64>), String> {
    Ok(((), recorded_schema(home)?))
}

fn recorded_schema(home: &Home) -> Result<Option<i64>, String> {
    let database = &home.prepared().database();
    read_recorded(database)
        .map(|recorded| recorded.map(|recorded| recorded.schema))
        .map_err(|error| format!("the store {} cannot be read: {error}", database.display()))
}

#[cfg(unix)]
type Before = server::Before;
#[cfg(not(unix))]
type Before = ();

/// Refuses a release that could not open the store, before anything changes. Returns what served the home.
async fn store_opens(home: &Home, latest: &Version, running: &Version) -> Result<Before, String> {
    let known = known_schema(latest, running);
    match store_schema(home).await? {
        (_, Some(schema)) if schema > known => Err(format!(
            "the store {} has schema {schema}, and release {latest} is known to open schema {known} at most: its server could not start",
            home.prepared().database().display(),
        )),
        (before, _) => Ok(before),
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

/// Downloads and checks the release's binary, and leaves it staged beside the binary, run once
/// with `--version`. On an error no staging folder is left.
#[cfg(unix)]
async fn prepare(
    source: &Source,
    release: &Release,
    target: &str,
    folder: &Path,
) -> Result<folder::Staging, String> {
    let unchanged = |what: String| format!("{what}. The installed binary was not changed.");
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
    let runnable = staging.runnable().map_err(unchanged)?;
    install::runs_as(&runnable, &latest)
        .await
        .map_err(unchanged)?;
    Ok(staging)
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
    use super::{SCHEMA_RELEASES, SCHEMA_VERSION, Version, known_schema, known_schema_in, request};

    #[test]
    fn a_newer_release_opens_this_build_s_schema_and_an_older_one_those_of_its_releases() {
        let version = |text| Version::parse(text).unwrap();
        let table = &[(1, "0.1.0"), (2, "0.3.1")];
        let running = version("0.4.0");
        let known = |text| known_schema_in(table, &version(text), &running, 2);
        assert_eq!(known("0.5.0"), 2);
        assert_eq!(known("0.4.0"), 2);
        assert_eq!(known("0.3.1"), 2);
        assert_eq!(known("0.3.0"), 1);
        assert_eq!(known("0.1.0"), 1);
        assert_eq!(known("0.0.9"), 0);
        assert_eq!(known_schema(&version("0.0.1"), &version("0.1.0")), 0);
    }

    #[test]
    fn the_schema_of_this_build_has_its_first_release_in_the_table() {
        assert!(
            SCHEMA_RELEASES
                .iter()
                .any(|(schema, _)| *schema == SCHEMA_VERSION),
            "add ({SCHEMA_VERSION}, \"<the release that ships it>\") to SCHEMA_RELEASES in apps/quadratique/cli/src/upgrade/mod.rs"
        );
        assert!(
            SCHEMA_RELEASES
                .iter()
                .all(|(_, first)| Version::parse(first).is_some())
        );
    }

    #[test]
    fn skills_stands_alone_and_a_flag_is_given_once() {
        assert!(request(&["--check", "--allow-downgrade"]).is_some());
        assert!(request(&["--skills", "--check"]).is_none());
        assert!(request(&["--check", "--check"]).is_none());
        assert!(request(&["--force"]).is_none());
    }
}
