use std::io::IsTerminal;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use acid_cat_core::cli::{CliCommand, CliIo, exit};
use acid_cat_core::{CallFailure, Kind};
use futures::FutureExt;
use quadratique_server::app::Context;
use quadratique_server::domain::Binding;
use quadratique_server::server::{self, Paths, ServerError};
use serde_json::{Value, json};

#[cfg(not(unix))]
use crate::background::Found;
use crate::background::{self, Home, Stopped};
use crate::{caller, mcp};

const OUTPUT_FLAG: &str = "--json";

pub fn all() -> Vec<CliCommand<Context>> {
    vec![
        command(
            "serve",
            "Run the server in the foreground, until SIGINT or SIGTERM. Without --port, on the port this home kept from its last start when that port is free, otherwise on one the system chooses, which it keeps (server strict on: a port this home never served, kept nowhere). --port <port>: on that loopback port, 0 letting the system choose; it reads and keeps no port. Other commands start it in the background when it is not running. --take-over: first stop the server that owns the home, of any build, while no command can start another; the user service runs this.",
            |args, io| serve(args, io).boxed(),
        ),
        command(
            "server",
            "server status: whether the server of this home runs, and where. server stop: stop it. server strict on: for a machine whose other accounts you do not trust; from the next start of the server, each start takes an address it never served and a new token, so open tabs and bookmarks of the web app stop working at each restart and quadratique ui --open lets the browser in again. server strict off: back to the default from the next start, which takes a new address and a new token, so each browser is let in once more with quadratique ui --open; after that the server keeps both across restarts. Neither starts a server. server sign-out: a new token for the web app, which signs every browser out; quadratique ui --open lets it in again.",
            |args, io| server_command(args, io).boxed(),
        ),
        command(
            "ui",
            "Make sure the server runs, and print the address of the web app (url), a one-time link that lets a browser in (link, valid 60 seconds and once), and addressKept: whether the server keeps that address across restarts (false under server strict on). ui --open: open the web app in the browser and let it in, through a file only this account can read, instead of printing the link. A browser that was let in keeps its token, and stays in across restarts until server sign-out (in strict mode, until the next restart).",
            |args, io| ui(args, io).boxed(),
        ),
        command("init", crate::init::USAGE, |args, io| {
            async move { crate::init::init(&words(&args), io).await }.boxed()
        }),
        command("upgrade", crate::upgrade::USAGE, |args, io| {
            async move { crate::upgrade::upgrade(&words(&args), io).await }.boxed()
        }),
        command(
            "mcp",
            "Serve the MCP tools of agents over standard input and output, as a client of the server.",
            |args, io| mcp_command(args, io).boxed(),
        ),
    ]
}

fn command<F>(name: &str, description: &str, run: F) -> CliCommand<Context>
where
    F: for<'a> Fn(Vec<String>, &'a mut CliIo) -> futures::future::BoxFuture<'a, i32>
        + Send
        + Sync
        + 'static,
{
    CliCommand {
        name: name.into(),
        description: description.into(),
        run: Arc::new(move |args, io, _context| run(args, io)),
    }
}

pub fn print_json(io: &mut CliIo, value: &Value) {
    let text = if std::io::stdout().is_terminal() {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    };
    (io.stdout)(&text.unwrap_or_default());
}

pub fn report(io: &mut CliIo, kind: Kind, message: impl Into<String>) -> i32 {
    report_to(&mut io.stderr, kind, message)
}

pub fn report_to(stderr: &mut dyn FnMut(&str), kind: Kind, message: impl Into<String>) -> i32 {
    stderr(&CallFailure::new(kind, message).to_json().to_string());
    match kind {
        Kind::UnknownCapability => exit::UNKNOWN_CAPABILITY,
        Kind::InvalidInput => exit::INVALID_INPUT,
        Kind::Unavailable => exit::UNAVAILABLE,
        Kind::Internal => exit::INTERNAL,
        Kind::Cancelled => exit::CANCELLED,
    }
}

fn words(args: &[String]) -> Vec<&str> {
    args.iter()
        .map(String::as_str)
        .filter(|arg| *arg != OUTPUT_FLAG)
        .collect()
}

pub fn home() -> Result<Home, String> {
    let paths = Paths::from_env().map_err(|error| error.to_string())?;
    Home::new(paths)
}

/// The home when its data folder exists, for a command that only reads: it creates nothing. A missing data folder has
/// no server and no store.
pub fn existing_home() -> Result<Option<Home>, String> {
    let paths = Paths::from_env().map_err(|error| error.to_string())?;
    let folder = paths.database.parent().unwrap_or(std::path::Path::new("/"));
    match std::fs::symlink_metadata(folder) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        _ => Home::new(paths).map(Some),
    }
}

const TAKE_OVER_FLAG: &str = "--take-over";

async fn serve(args: Vec<String>, io: &mut CliIo) -> i32 {
    let words = words(&args);
    let take_over = words.contains(&TAKE_OVER_FLAG);
    let words: Vec<&str> = words
        .into_iter()
        .filter(|word| *word != TAKE_OVER_FLAG)
        .collect();
    let listen = match words[..] {
        [] => server::Listen::Home,
        ["--port", port] => match port.parse::<u16>() {
            Ok(port) => server::Listen::On(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                port,
            ))),
            Err(_) => {
                return report(
                    io,
                    Kind::InvalidInput,
                    format!("--port takes a port number from 0 to 65535: {port}"),
                );
            }
        },
        _ => {
            return report(
                io,
                Kind::InvalidInput,
                "Give one of: quadratique serve [--take-over], quadratique serve [--take-over] --port <port>.",
            );
        }
    };
    let home = match home() {
        Ok(home) => home,
        Err(message) => return report(io, Kind::Internal, message),
    };
    (io.stderr)(&format!(
        "quadratique serve: pid {}, database {}",
        std::process::id(),
        home.prepared().database().display()
    ));
    let started = match std::env::var(background::STARTED_VARIABLE) {
        Ok(value) if value == background::ON_DEMAND => server::Started::OnDemand,
        _ => server::Started::Serve,
    };
    // Every start holds `start.lock` until its server is ready. A server started on demand runs while the command
    // that spawned it holds the lock for it.
    let reserved = match (take_over, started) {
        (true, _) => background::take_over(&home).await.map(Some),
        (false, server::Started::Serve) => background::reserve_start(&home).await.map(Some),
        (false, server::Started::OnDemand) => Ok(None),
    };
    let taken = match reserved {
        Ok(taken) => taken,
        Err(error) => return report(io, Kind::Unavailable, error.to_string()),
    };
    // The start gets what is left of the deadline of the command that reserved it, so a take-over keeps one deadline.
    let left = taken
        .as_ref()
        .map_or(server::START_LIMIT, background::StartLockHeld::left);
    match server::start_by_within(home.prepared(), listen, started, left).await {
        Ok(server) => {
            drop(taken);
            server::until_stopped(server).await;
            exit::VALUE
        }
        Err(error @ ServerError::Owned(_)) => report(io, Kind::Unavailable, error.to_string()),
        Err(error) => report(io, Kind::Internal, error.to_string()),
    }
}

async fn server_command(args: Vec<String>, io: &mut CliIo) -> i32 {
    let home = match home() {
        Ok(home) => home,
        Err(message) => return report(io, Kind::Internal, message),
    };
    let answer = match words(&args)[..] {
        ["status"] => status(&home).await,
        ["stop"] => stop(&home).await,
        ["strict", "on"] => set_strict(&home, true).await,
        ["strict", "off"] => set_strict(&home, false).await,
        ["sign-out"] => match sign_out(&home).await {
            Ok(value) => Ok(value),
            Err(message) => return report(io, Kind::Unavailable, message),
        },
        _ => {
            return report(
                io,
                Kind::InvalidInput,
                "Give one of: quadratique server status, quadratique server stop, quadratique server strict on, quadratique server strict off, quadratique server sign-out.",
            );
        }
    };
    match answer {
        Ok(value) => {
            print_json(io, &value);
            exit::VALUE
        }
        Err(error) => report(io, Kind::Unavailable, error.to_string()),
    }
}

/// `addressKept` comes from the running server when one answered; with none, it says what the next start does.
fn located(home: &Home, mut value: Value) -> Value {
    if value.get("addressKept").is_none() {
        value["addressKept"] = json!(!server::strict(home.prepared()));
    }
    value["runtime"] = json!(home.prepared().runtime());
    value["log"] = json!(home.log());
    value
}

/// One answer for each state of the record's table (§2), from the same observation that `ensure`, `stop` and
/// `serve --take-over` act on. Only a server of a build before the socket is asked over TCP.
#[cfg(unix)]
async fn status(home: &Home) -> Result<Value, background::LifeError> {
    use background::Observed;

    let database = home.prepared().database();
    let value = match background::observe(home).await? {
        Observed::Serving { identity, .. } => answered_over_socket(&identity),
        Observed::OtherDatabase(identity) => json!({
            "otherDatabase": identity.database,
            "pid": identity.pid,
            "database": database,
        }),
        Observed::Newer(identity) => json!({
            "newer": true,
            "control": identity.control,
            "pid": identity.pid,
            "generation": identity.generation,
            "reason": format!(
                "the server of this home speaks control version {}, which this build does not know: upgrade this command line",
                identity.control.unwrap_or_default()
            ),
            "database": database,
        }),
        Observed::Malformed(why) => json!({
            "malformed": true,
            "reason": format!("the server's identity on {} is not valid: {why}", home.prepared().socket().display()),
            "database": database,
        }),
        Observed::Stalled => json!({
            "stalled": true,
            "reason": format!(
                "the server on {} accepted a connection and gave no full answer within {:?}",
                home.prepared().socket().display(),
                server::PROBE_LIMIT
            ),
            "database": database,
        }),
        Observed::Unreachable(reason) => unreachable(home, reason),
        Observed::Starting { reason } => {
            let mut value = json!({ "running": false, "starting": true, "database": database });
            if let Some(reason) = reason {
                value["reason"] = json!(reason);
            }
            value
        }
        Observed::Free => json!({ "running": false, "database": database }),
        Observed::Legacy => background::legacy_status(home),
    };
    Ok(located(home, value))
}

/// Windows has no socket yet (QUA-61): the runtime file and TCP find the server.
#[cfg(not(unix))]
async fn status(home: &Home) -> Result<Value, background::LifeError> {
    let database = home.prepared().database();
    let value = match background::find(home).await? {
        Found::Ours(runtime) => json!({
            "running": true,
            "sameInterface": true,
            "pid": runtime.pid,
            "url": runtime.url,
            "database": runtime.database,
        }),
        Found::OtherInterface { runtime, theirs } => json!({
            "running": true,
            "sameInterface": false,
            "interface": theirs.named(),
            "pid": runtime.pid,
            "url": runtime.url,
            "database": runtime.database,
        }),
        Found::Refused { reason } => match home.held() {
            Ok(true) => unreachable(home, reason),
            Ok(false) => json!({ "running": false, "database": database }),
            Err(error) => match home.unusable(error, background::Attempt::Start) {
                background::LifeError::Unreachable { reason } => unreachable(home, reason),
                other => return Err(other),
            },
        },
        Found::Nobody => json!({ "running": false, "database": database }),
    };
    Ok(located(home, value))
}

#[cfg(unix)]
fn answered_over_socket(identity: &server::Identity) -> Value {
    let same = identity.interface.as_deref() == Some(quadratique_server::app::interface().as_str());
    let mut value = json!({
        "running": true,
        "sameInterface": same,
        "pid": identity.pid,
        "url": identity.url,
        "database": identity.database,
        "generation": identity.generation,
        "control": identity.control,
        "started": identity.started,
    });
    if !same {
        value["interface"] = json!(identity.interface);
    }
    if let Some(kept) = identity.address_kept {
        value["addressKept"] = json!(kept);
    }
    if let Some(origin) = identity
        .url
        .as_deref()
        .and_then(|url| url.strip_suffix(server::BASE))
    {
        value["web"] = json!(format!("{origin}/"));
    }
    value
}

fn unreachable(home: &Home, reason: String) -> Value {
    json!({
        "reachable": false,
        "reason": reason,
        "database": home.prepared().database(),
    })
}

/// The setting applies from the next start of the server: neither turning it on nor off starts or changes one.
async fn set_strict(home: &Home, on: bool) -> Result<Value, background::LifeError> {
    server::set_strict(home.prepared(), on)
        .map_err(|error| home.unusable(error, background::Attempt::Start))?;
    Ok(located(
        home,
        json!({ "strict": on, "from": "the next start of the server" }),
    ))
}

async fn stop(home: &Home) -> Result<Value, background::LifeError> {
    let value = match background::stop(home).await? {
        Stopped::Ended { pid, generation } => {
            json!({ "stopped": true, "pid": pid, "generation": generation })
        }
        Stopped::NotRunning => json!({ "stopped": false }),
        Stopped::Closing { pid, generation } => {
            json!({ "stopped": false, "stopping": true, "pid": pid, "generation": generation })
        }
    };
    Ok(located(home, value))
}

async fn ui(args: Vec<String>, io: &mut CliIo) -> i32 {
    let open = match words(&args)[..] {
        [] => false,
        ["--open"] => true,
        _ => {
            return report(
                io,
                Kind::InvalidInput,
                "Give one of: quadratique ui, quadratique ui --open.",
            );
        }
    };
    let home = match home() {
        Ok(home) => home,
        Err(message) => return report(io, Kind::Internal, message),
    };
    let paired = match pair(&home, open).await {
        Ok(paired) => paired,
        Err(message) => return report(io, Kind::Unavailable, message),
    };
    let kept = address_kept(&home).await;
    let url = paired.url.clone();
    let answer = match (open, paired.file) {
        (false, _) | (true, None) => {
            json!({ "url": url, "link": paired.link, "opened": false, "addressKept": kept })
        }
        (true, Some(file)) => match open_in_browser(&file.display().to_string()).await {
            Ok(Launch::Opened) => json!({ "url": url, "opened": true, "addressKept": kept }),
            Ok(Launch::Requested) => {
                json!({ "url": url, "opened": false, "launch": "requested", "addressKept": kept })
            }
            Ok(Launch::Failed(status)) => {
                let ended = match status.code() {
                    Some(code) => format!("exited with code {code}"),
                    None => format!("ended: {status}"),
                };
                return report(
                    io,
                    Kind::Unavailable,
                    format!(
                        "the browser opener {OPENER} {ended}; run `quadratique ui` for a link to open yourself"
                    ),
                );
            }
            Err(error) => {
                return report(
                    io,
                    Kind::Unavailable,
                    format!(
                        "the browser did not open ({error}); run `quadratique ui` for a link to open yourself"
                    ),
                );
            }
        },
    };
    print_json(io, &answer);
    exit::VALUE
}

/// A one-time link to the web app, from the server itself over its socket, so that the code only ever reaches a
/// listener of this home's server.
#[cfg(unix)]
async fn pair(home: &Home, open: bool) -> Result<quadratique_server::pairing::Issued, String> {
    background::ensure(home)
        .await
        .map_err(|error| error.to_string())?;
    match server::pair_over_socket(home.prepared(), open).await {
        Ok(server::Asked::Answered(issued)) => Ok(issued),
        Ok(server::Asked::Refused(reason)) => Err(reason),
        Err(unanswered) => Err(format!(
            "the server on {} gave no link: {}",
            home.prepared().socket().display(),
            unanswered.describe()
        )),
    }
}

/// Pairing goes through the socket, which Windows does not have yet (QUA-61).
#[cfg(not(unix))]
async fn pair(home: &Home, _open: bool) -> Result<quadratique_server::pairing::Issued, String> {
    background::ensure(home)
        .await
        .map_err(|error| error.to_string())?;
    Err(
        "pairing the web app needs the server's socket, which Windows does not have yet (QUA-61)"
            .to_owned(),
    )
}

/// A new token for the web app: every browser is signed out, and `quadratique ui` pairs it again.
async fn sign_out(home: &Home) -> Result<Value, String> {
    background::ensure(home)
        .await
        .map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        match server::sign_out_over_socket(home.prepared()).await {
            Ok(server::Asked::Answered(signed_out)) => Ok(located(
                home,
                json!({ "signedOut": signed_out.signed_out, "lasting": signed_out.lasting }),
            )),
            Ok(server::Asked::Refused(reason)) => Err(reason),
            Err(unanswered) => Err(format!(
                "the server on {} did not sign out: {}",
                home.prepared().socket().display(),
                unanswered.describe()
            )),
        }
    }
    #[cfg(not(unix))]
    Err(
        "signing out needs the server's socket, which Windows does not have yet (QUA-61)"
            .to_owned(),
    )
}

/// Whether the running server listens on the port its home keeps, as it says itself; a server that does not say gets the
/// home's setting.
async fn address_kept(home: &Home) -> bool {
    #[cfg(unix)]
    if let Ok(identity) = server::identity_over_socket(home.prepared()).await
        && let Some(kept) = identity.address_kept
    {
        return kept;
    }
    !server::strict(home.prepared())
}

const OPENER: &str = if cfg!(target_os = "macos") {
    "open"
} else {
    "xdg-open"
};
const OPENER_LIMIT: Duration = Duration::from_secs(5);

enum Launch {
    Opened,
    Failed(ExitStatus),
    Requested,
}

async fn open_in_browser(url: &str) -> std::io::Result<Launch> {
    let mut opener = tokio::process::Command::new(OPENER)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    match tokio::time::timeout(OPENER_LIMIT, opener.wait()).await {
        Ok(status) => {
            let status = status?;
            Ok(if status.success() {
                Launch::Opened
            } else {
                Launch::Failed(status)
            })
        }
        Err(_) => Ok(Launch::Requested),
    }
}

async fn mcp_command(args: Vec<String>, io: &mut CliIo) -> i32 {
    if let [extra, ..] = words(&args)[..] {
        return report(
            io,
            Kind::InvalidInput,
            format!("mcp takes no argument: {extra}"),
        );
    }
    let caller = match caller::detect(Binding::Mcp) {
        Ok(caller) => caller,
        Err(error) => return report(io, Kind::InvalidInput, error.to_string()),
    };
    let home = match home() {
        Ok(home) => home,
        Err(message) => return report(io, Kind::Internal, message),
    };
    if let Err(error @ background::LifeError::Unreachable { .. }) = background::ensure(&home).await
    {
        return report(io, Kind::Unavailable, error.to_string());
    }
    match mcp::serve(mcp::Upstream::new(home, caller)).await {
        Ok(()) => exit::VALUE,
        Err(problems) => report(io, Kind::Internal, problems.join("; ")),
    }
}
