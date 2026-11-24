use std::fs::{File, OpenOptions, TryLockError};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use acid_cat_core::http::{HttpBinding, HttpOptions};
use acid_cat_core::remote::{self, Handler, RequestBody, ResponseBody};
use bytes::Bytes;
use futures::future::BoxFuture;
use http::{Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::app::{Context, application, caller_of, interface, with_branches};
use crate::credentials::{CREDENTIALS_FILE, Credentials, CredentialsError};
use crate::git::repository::GIT_LIMIT;
use crate::git::watcher::{self, Watcher};
use crate::serve::{self, Admit, Connection, OWNER_ONLY};
use crate::store::{Store, StoreError, refuse_git_work_tree};
use crate::web::{self, WebApp};

mod closing;

pub const TARGET: &str = "cli";
pub const BASE: &str = "/cli";
pub const IDENTITY: &str = "/server";
const DATABASE_FILE: &str = "workspace.db";
const RUNTIME_FILE: &str = "server.json";
const LOCK_FILE: &str = "server.lock";
const DIRECTORY: &str = "quadratic";
/// How long a server may take to start: a command waits this long for the server it started, and a server waits
/// this long for a holder of the lock that is not a server to let go.
pub const START_LIMIT: Duration = Duration::from_secs(30);
pub const PROBE_LIMIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Copy, Debug)]
pub struct Waits {
    pub start: Duration,
    pub probe: Duration,
}

impl Default for Waits {
    fn default() -> Self {
        Waits {
            start: START_LIMIT,
            probe: PROBE_LIMIT,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("no directory for the workspace: set QUADRATIC_HOME, or HOME")]
    NoHome,
    #[error(
        "{0} is not an IPv4 loopback address: Quadratic serves only this machine, on 127.0.0.1"
    )]
    NotLoopback(SocketAddr),
    #[error("another server owns the database at {}", .0.display())]
    Owned(PathBuf),
    #[error("the store cannot open: {0}")]
    Store(#[from] StoreError),
    #[error("the runtime file or the listener failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("the application breaks the rules of AcidCat: {}", .0.join("; "))]
    Application(Vec<String>),
    #[error("{0}")]
    CallersUnknown(String),
    #[error("the agent tokens cannot load: {0}")]
    Credentials(#[from] CredentialsError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub database: PathBuf,
    pub runtime: PathBuf,
    pub home: Option<PathBuf>,
}

impl Paths {
    pub fn lock(&self) -> PathBuf {
        self.database.with_file_name(LOCK_FILE)
    }

    pub fn credentials(&self) -> PathBuf {
        self.database.with_file_name(CREDENTIALS_FILE)
    }

    pub fn in_home(home: &Path) -> Result<Paths, ServerError> {
        let home = std::path::absolute(home)?;
        Ok(Paths {
            database: home.join(DATABASE_FILE),
            runtime: home.join(RUNTIME_FILE),
            home: Some(home),
        })
    }

    pub fn from_env() -> Result<Paths, ServerError> {
        let variable = |name: &str| -> std::io::Result<Option<PathBuf>> {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(std::path::absolute)
                .transpose()
        };
        if let Some(home) = variable("QUADRATIC_HOME")? {
            return Paths::in_home(&home);
        }
        let user_home = variable("HOME")?.ok_or(ServerError::NoHome)?;
        let data = if cfg!(target_os = "macos") {
            user_home.join("Library/Application Support")
        } else {
            variable("XDG_DATA_HOME")?.unwrap_or_else(|| user_home.join(".local/share"))
        }
        .join(DIRECTORY);
        Ok(Paths {
            database: data.join(DATABASE_FILE),
            runtime: data.join(RUNTIME_FILE),
            home: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeFile {
    pub pid: u32,
    pub address: SocketAddr,
    pub url: String,
    pub database: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub pid: u32,
    pub database: PathBuf,
    /// Absent in the answer of a server built before the interface was part of it.
    #[serde(default)]
    pub interface: Option<String>,
}

struct Identify {
    answer: Bytes,
}

impl Handler for Identify {
    fn handle(&self, request: Request<RequestBody>) -> BoxFuture<'static, Response<ResponseBody>> {
        let (status, body) = if request.method() == Method::GET {
            (StatusCode::OK, self.answer.clone())
        } else {
            (StatusCode::METHOD_NOT_ALLOWED, Bytes::new())
        };
        let mut response = Response::new(Full::new(body).boxed_unsync());
        *response.status_mut() = status;
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        Box::pin(std::future::ready(response))
    }
}

pub struct Server {
    admit: Admit,
    credentials: Credentials,
    served: serve::Served,
    watcher: Watcher,
    runtime: RuntimeFile,
    published: closing::Published,
    _home: closing::HomeLock,
}

impl Server {
    pub fn address(&self) -> SocketAddr {
        self.runtime.address
    }

    pub fn url(&self) -> &str {
        &self.runtime.url
    }

    /// What this server's admission decides for a connection, as its accept loop asks it.
    pub fn admits(&self, connection: Connection) -> bool {
        (self.admit)(connection)
    }

    pub fn credentials(&self) -> &Credentials {
        &self.credentials
    }

    pub fn runtime_path(&self) -> &Path {
        self.published.path()
    }

    pub async fn look_at_repositories(&self) {
        self.watcher.look().await;
    }

    pub async fn shutdown(self) {
        self.watcher.stop().await;
        self.served.close().await;
    }
}

pub fn loopback() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

pub async fn start(paths: &Paths, address: SocketAddr) -> Result<Server, ServerError> {
    start_with(
        paths,
        address,
        WebApp::from_env(),
        watcher::EVERY,
        Waits::default(),
    )
    .await
}

pub async fn start_with(
    paths: &Paths,
    address: SocketAddr,
    web_app: WebApp,
    look_every: Duration,
    waits: Waits,
) -> Result<Server, ServerError> {
    if !matches!(address, SocketAddr::V4(v4) if v4.ip().is_loopback()) {
        return Err(ServerError::NotLoopback(address));
    }
    let own_account = callers::own_account()?;
    callers::check_own_connection(own_account)?;
    let owner: closing::HomeLock = Arc::new(own(paths, waits).await?);
    let credentials = Credentials::open(paths.credentials())?.holding(Arc::clone(&owner));
    let store = Store::open(&paths.database)?;
    store.hold_until_closed(Arc::clone(&owner));
    let database = store.path().to_path_buf();
    let app = application().map_err(ServerError::Application)?;
    let identity = Identity {
        pid: std::process::id(),
        database: database.clone(),
        interface: Some(interface()),
    };
    let identify = Identify {
        answer: Bytes::from(serde_json::to_vec(&identity).map_err(std::io::Error::from)?),
    };
    let watcher = Watcher::start(store.clone(), look_every, GIT_LIMIT, watcher::READ_WAIT);
    let looks = watcher.looks();
    let origin = web::Origin::default();
    let web_routes = web::routes(&app, &store, web_app);
    let command_line_credentials = credentials.clone();
    let binding = HttpBinding::with_request_context(
        app,
        move |parts: &http::request::Parts| {
            let caller = caller_of(&parts.headers);
            let (store, looks) = (store.clone(), looks.clone());
            let credentials = command_line_credentials.clone();
            async move {
                let caller = with_branches(caller?).await;
                Ok(Context::watching(store, caller, looks).with_credentials(credentials))
            }
        },
        HttpOptions {
            target: TARGET.into(),
            base: BASE.into(),
            ..HttpOptions::default()
        },
    );
    let command_line: remote::Routes = vec![
        (BASE.into(), Arc::new(binding) as Arc<dyn Handler>),
        (IDENTITY.into(), Arc::new(identify) as Arc<dyn Handler>),
    ];
    let mut routes = web::guarded(&origin, command_line, web::OriginHeader::Refused);
    routes.extend(web::guarded(
        &origin,
        web_routes,
        web::OriginHeader::OwnOnly,
    ));
    let admit = callers::owner_only(own_account);
    let served = match serve::serve(address, routes, Arc::clone(&admit)).await {
        Ok(served) => served,
        Err(error) => {
            watcher.stop().await;
            return Err(error.into());
        }
    };
    origin.set(served.address);
    let runtime = RuntimeFile {
        pid: std::process::id(),
        address: served.address,
        url: format!("{}{BASE}", served.url()),
        database,
    };
    if let Err(error) = write_runtime(&paths.runtime, &runtime) {
        watcher.stop().await;
        served.close().await;
        return Err(error.into());
    }
    Ok(Server {
        served,
        published: closing::Published::new(paths.runtime.clone(), &runtime),
        runtime,
        watcher,
        _home: owner,
        admit,
        credentials,
    })
}

#[cfg(target_os = "linux")]
mod callers {
    use super::ServerError;
    use crate::peer;
    use crate::serve::Admit;

    fn unknown(error: peer::CallersUnknown) -> ServerError {
        ServerError::CallersUnknown(error.to_string())
    }

    pub fn own_account() -> Result<u32, ServerError> {
        peer::own_account().map_err(unknown)
    }

    pub fn owner_only(own_uid: u32) -> Admit {
        peer::owner_only(own_uid)
    }

    pub fn check_own_connection(own_uid: u32) -> Result<(), ServerError> {
        peer::check_own_connection(own_uid).map_err(unknown)
    }
}

#[cfg(not(target_os = "linux"))]
mod callers {
    use std::sync::Arc;

    use super::ServerError;
    use crate::serve::Admit;

    pub fn own_account() -> Result<(), ServerError> {
        Ok(())
    }

    pub fn owner_only((): ()) -> Admit {
        Arc::new(|_| true)
    }

    pub fn check_own_connection((): ()) -> Result<(), ServerError> {
        Ok(())
    }
}

/// Every child process between its `fork` and its `exec` holds the lock too, because a `flock` stays taken while any
/// copy of its descriptor is open.
async fn own(paths: &Paths, waits: Waits) -> Result<File, ServerError> {
    let directory = paths.database.parent().unwrap_or(Path::new("."));
    refuse_git_work_tree(directory)?;
    std::fs::create_dir_all(directory)?;
    let database = directory
        .canonicalize()?
        .join(paths.database.file_name().unwrap_or_default());
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.lock())?;
    let deadline = Instant::now() + waits.start;
    loop {
        match lock.try_lock() {
            Ok(()) => {
                private(directory)?;
                return Ok(lock);
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || answers_for(&paths.runtime, &database, waits.probe.min(left)).await {
            return Err(ServerError::Owned(paths.database.clone()));
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(unix)]
fn private(directory: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn private(_directory: &Path) -> std::io::Result<()> {
    Ok(())
}

async fn answers_for(runtime: &Path, database: &Path, limit: Duration) -> bool {
    let Some(runtime) = std::fs::read(runtime)
        .ok()
        .and_then(|text| serde_json::from_slice::<RuntimeFile>(&text).ok())
    else {
        return false;
    };
    identity_within(runtime.address, limit)
        .await
        .is_ok_and(|identity| identity.pid == runtime.pid && identity.database == database)
}

#[derive(Debug)]
pub enum Unanswered {
    NotListening(std::io::Error),
    /// The connect failed with `EPERM` or `EACCES`: a sandbox forbids it.
    NotPermitted(std::io::Error),
    OwnerOnly,
    NoIdentity,
}

impl Unanswered {
    pub fn of_connect(error: std::io::Error) -> Unanswered {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            Unanswered::NotPermitted(error)
        } else {
            Unanswered::NotListening(error)
        }
    }
}

/// The answer of the server at `address` to `GET /server` within the probe limit, or why it gave none.
pub async fn identity(address: SocketAddr) -> Result<Identity, Unanswered> {
    identity_within(address, PROBE_LIMIT).await
}

async fn identity_within(address: SocketAddr, limit: Duration) -> Result<Identity, Unanswered> {
    tokio::time::timeout(limit, ask_identity(address))
        .await
        .unwrap_or(Err(Unanswered::NoIdentity))
}

async fn ask_identity(address: SocketAddr) -> Result<Identity, Unanswered> {
    let mut stream = TcpStream::connect(address)
        .await
        .map_err(Unanswered::of_connect)?;
    let request =
        format!("GET {IDENTITY} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|_| Unanswered::NoIdentity)?;
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .await
        .map_err(|_| Unanswered::NoIdentity)?;
    let answer = String::from_utf8(answer).map_err(|_| Unanswered::NoIdentity)?;
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .ok_or(Unanswered::NoIdentity)?;
    match head.split_whitespace().nth(1) {
        Some("200") => serde_json::from_str(body).map_err(|_| Unanswered::NoIdentity),
        Some("403") if body == OWNER_ONLY => Err(Unanswered::OwnerOnly),
        _ => Err(Unanswered::NoIdentity),
    }
}

fn write_runtime(path: &Path, runtime: &RuntimeFile) -> std::io::Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let partial = path.with_extension(format!("json.{}", runtime.pid));
    std::fs::write(&partial, serde_json::to_vec_pretty(runtime)?)?;
    std::fs::rename(&partial, path)
}

pub async fn run(paths: &Paths, address: SocketAddr) -> Result<(), ServerError> {
    let server = start(paths, address).await?;
    until_stopped(server).await;
    Ok(())
}

/// Serves until SIGINT or SIGTERM, then shuts the server down.
pub async fn until_stopped(server: Server) {
    stopped().await;
    server.shutdown().await;
}

async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
