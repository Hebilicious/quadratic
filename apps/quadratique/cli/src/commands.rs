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

use crate::background::{self, Attempt, Found, Home, Stopped};
use crate::{caller, mcp};

const OUTPUT_FLAG: &str = "--json";

pub fn all() -> Vec<CliCommand<Context>> {
    vec![
        command(
            "serve",
            "Run the server in the foreground, until SIGINT or SIGTERM, on a port the system chooses, or on the loopback port that --port <port> names. Other commands start it in the background when it is not running. --take-over: first stop the server that owns the home, of any build, while no command can start another; the user service runs this.",
            |args, io| serve(args, io).boxed(),
        ),
        command(
            "server",
            "server status: whether the server of this home runs, and where. server stop: stop it.",
            |args, io| server_command(args, io).boxed(),
        ),
        command(
            "ui",
            "Make sure the server runs, and print the address of the web app. ui --open: also open it in the browser.",
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

const TAKE_OVER_FLAG: &str = "--take-over";

async fn serve(args: Vec<String>, io: &mut CliIo) -> i32 {
    let words = words(&args);
    let take_over = words.contains(&TAKE_OVER_FLAG);
    let words: Vec<&str> = words
        .into_iter()
        .filter(|word| *word != TAKE_OVER_FLAG)
        .collect();
    let port = match words[..] {
        [] => 0,
        ["--port", port] => match port.parse::<u16>() {
            Ok(port) => port,
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
    let address = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    let taken = if take_over {
        match background::take_over(&home).await {
            Ok(taken) => Some(taken),
            Err(error) => return report(io, Kind::Unavailable, error.to_string()),
        }
    } else {
        None
    };
    let started = match std::env::var(background::STARTED_VARIABLE) {
        Ok(value) if value == background::ON_DEMAND => server::Started::OnDemand,
        _ => server::Started::Serve,
    };
    match server::start_by(home.prepared(), address, started).await {
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
        _ => {
            return report(
                io,
                Kind::InvalidInput,
                "Give one of: quadratique server status, quadratique server stop.",
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

fn located(home: &Home, mut value: Value) -> Value {
    value["runtime"] = json!(home.prepared().runtime());
    value["log"] = json!(home.log());
    value
}

/// One answer for each state of the record's table (§2). The socket decides, except when it is confirmed absent or
/// refusing: then the home's lock tells a stopped home from one whose server is starting or stopping, or is of a build
/// before the socket, which the runtime file and TCP still find.
#[cfg(unix)]
async fn status(home: &Home) -> Result<Value, background::LifeError> {
    use background::OnSocket;

    let database = home.prepared().database();
    let value = match background::on_socket(home).await {
        OnSocket::Serving(identity) => answered_over_socket(&identity),
        OnSocket::OtherDatabase(identity) => json!({
            "otherDatabase": identity.database,
            "pid": identity.pid,
            "database": database,
        }),
        OnSocket::Newer(identity) => json!({
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
        OnSocket::Malformed(why) => json!({
            "malformed": true,
            "reason": format!("the server's identity on {} is not valid: {why}", home.prepared().socket().display()),
            "database": database,
        }),
        OnSocket::Stalled => json!({
            "stalled": true,
            "reason": format!(
                "the server on {} accepted a connection and gave no full answer within {:?}",
                home.prepared().socket().display(),
                server::PROBE_LIMIT
            ),
            "database": database,
        }),
        OnSocket::Denied(reason) | OnSocket::PathError(reason) => unreachable(home, reason),
        OnSocket::NotReady(why) => match held(home)? {
            Held::Yes => {
                json!({ "running": false, "starting": true, "reason": why, "database": database })
            }
            Held::No => json!({ "running": false, "database": database }),
            Held::Unreachable(reason) => unreachable(home, reason),
        },
        OnSocket::Absent => match held(home)? {
            Held::No => json!({ "running": false, "database": database }),
            Held::Unreachable(reason) => unreachable(home, reason),
            Held::Yes => match background::find(home).await? {
                Found::Ours(runtime) => json!({
                    "running": true,
                    "legacy": true,
                    "sameInterface": true,
                    "pid": runtime.pid,
                    "url": runtime.url,
                    "database": runtime.database,
                }),
                Found::OtherInterface { runtime, theirs } => json!({
                    "running": true,
                    "legacy": true,
                    "sameInterface": false,
                    "interface": theirs.named(),
                    "pid": runtime.pid,
                    "url": runtime.url,
                    "database": runtime.database,
                }),
                Found::Refused { reason } => unreachable(home, reason),
                Found::Nobody => {
                    json!({ "running": false, "starting": true, "database": database })
                }
            },
        },
    };
    Ok(located(home, value))
}

#[cfg(unix)]
enum Held {
    Yes,
    No,
    Unreachable(String),
}

#[cfg(unix)]
fn held(home: &Home) -> Result<Held, background::LifeError> {
    match home.held() {
        Ok(true) => Ok(Held::Yes),
        Ok(false) => Ok(Held::No),
        Err(error) => match home.unusable(error, Attempt::Start) {
            background::LifeError::Unreachable { reason } => Ok(Held::Unreachable(reason)),
            other => Err(other),
        },
    }
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
            Err(error) => match home.unusable(error, Attempt::Start) {
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
    value
}

fn unreachable(home: &Home, reason: String) -> Value {
    json!({
        "reachable": false,
        "reason": reason,
        "database": home.prepared().database(),
    })
}

async fn stop(home: &Home) -> Result<Value, background::LifeError> {
    let value = match background::stop(home).await? {
        Stopped::Ended { pid } => json!({ "stopped": true, "pid": pid }),
        Stopped::NotRunning => json!({ "stopped": false }),
        Stopped::Closing { pid } => json!({ "stopped": false, "stopping": true, "pid": pid }),
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
    let runtime = match background::ensure(&home).await {
        Ok(runtime) => runtime,
        Err(error) => return report(io, Kind::Unavailable, error.to_string()),
    };
    let url = format!("http://{}/", runtime.address);
    let answer = if !open {
        json!({ "url": url, "opened": false })
    } else {
        match open_in_browser(&url).await {
            Ok(Launch::Opened) => json!({ "url": url, "opened": true }),
            Ok(Launch::Requested) => json!({ "url": url, "opened": false, "launch": "requested" }),
            Ok(Launch::Failed(status)) => {
                let ended = match status.code() {
                    Some(code) => format!("exited with code {code}"),
                    None => format!("ended: {status}"),
                };
                return report(
                    io,
                    Kind::Unavailable,
                    format!("the browser opener {OPENER} {ended}; the web app is at {url}"),
                );
            }
            Err(error) => {
                return report(
                    io,
                    Kind::Unavailable,
                    format!("the browser did not open ({error}); the web app is at {url}"),
                );
            }
        }
    };
    print_json(io, &answer);
    exit::VALUE
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
