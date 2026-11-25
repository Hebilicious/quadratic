use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use acid_cat_core::http::{HttpBinding, HttpOptions};
use acid_cat_core::remote::{self, Handler, RequestBody, ResponseBody};
use bytes::Bytes;
use futures::future::BoxFuture;
use http::{Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::app::{Context, application, caller_of, interface};
use crate::credentials::{CREDENTIALS_FILE, Credentials, CredentialsError};
use crate::git::repository::GIT_LIMIT;
use crate::git::watcher::{self, Watcher};
use crate::home::{self, HomeError, Prepared};
use crate::serve::{self, Admit, Connection, OWNER_ONLY};
use crate::store::{Store, StoreError, canonical_folder};
use crate::web::{self, WebApp};

mod closing;

pub const TARGET: &str = "cli";
pub const BASE: &str = "/cli";
pub const IDENTITY: &str = "/server";
/// The version of the control protocol: `GET /server`, and later `POST /server/stop`. A build keeps answering every
/// version it ever served.
pub const CONTROL: u32 = 1;
/// The authority that a request on the Unix socket names: the socket, not a host, decides where it goes.
pub const SOCKET_AUTHORITY: &str = "quadratique.invalid";
pub(crate) const DATABASE_FILE: &str = "workspace.db";
pub(crate) const RUNTIME_FILE: &str = "server.json";
pub(crate) const LOCK_FILE: &str = "server.lock";
const DIRECTORY: &str = "quadratique";
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
    #[error(
        "no directory for the workspace: set QUADRATIQUE_HOME, or HOME (LOCALAPPDATA on Windows)"
    )]
    NoHome,
    #[error(
        "{0} is not an IPv4 loopback address: Quadratique serves only this machine, on 127.0.0.1"
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
    #[error("the Quadratique home is refused: {0}")]
    Home(#[from] HomeError),
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
        Paths::from_variables(|name| std::env::var_os(name))
    }

    /// `QUADRATIQUE_HOME`, else the platform's folder for an application's data: `%LOCALAPPDATA%` on Windows,
    /// `~/Library/Application Support` on macOS, `$XDG_DATA_HOME` or `~/.local/share` elsewhere.
    fn from_variables(read: impl Fn(&str) -> Option<OsString>) -> Result<Paths, ServerError> {
        let variable = |name: &str| -> std::io::Result<Option<PathBuf>> {
            read(name)
                .filter(|value| !value.is_empty())
                .map(std::path::absolute)
                .transpose()
        };
        if let Some(home) = variable("QUADRATIQUE_HOME")? {
            return Paths::in_home(&home);
        }
        let data = if cfg!(windows) {
            variable("LOCALAPPDATA")?.ok_or(ServerError::NoHome)?
        } else {
            let user_home = variable("HOME")?.ok_or(ServerError::NoHome)?;
            if cfg!(target_os = "macos") {
                user_home.join("Library/Application Support")
            } else {
                variable("XDG_DATA_HOME")?.unwrap_or_else(|| user_home.join(".local/share"))
            }
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
    /// The store schema version the server opened its store at. Absent in the answer of a server
    /// built before the field: those builds know schema 1 only.
    #[serde(default)]
    pub schema: Option<i64>,
    /// 128 random bits, new at each start of a server. This field and the ones below are absent in the answer of a
    /// server built before the Unix socket.
    #[serde(default)]
    pub generation: Option<String>,
    #[serde(default)]
    pub control: Option<u32>,
    #[serde(default)]
    pub executable: Option<Executable>,
    #[serde(default)]
    pub started: Option<Started>,
    /// The address of the command line's routes on TCP, as the runtime file names it.
    #[serde(default)]
    pub url: Option<String>,
}

/// The file the server runs, as it found it at its start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Executable {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
}

impl Executable {
    fn of_this_process() -> Option<Executable> {
        let path = std::env::current_exe().ok()?;
        let meta = std::fs::metadata(&path).ok()?;
        #[cfg(unix)]
        let (device, inode) = {
            use std::os::unix::fs::MetadataExt;
            (meta.dev(), meta.ino())
        };
        #[cfg(not(unix))]
        let (device, inode) = {
            let _ = meta;
            (0, 0)
        };
        Some(Executable {
            path,
            device,
            inode,
        })
    }
}

/// Who started the server: a command that needed it, or a person or a supervisor with `quadratique serve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Started {
    OnDemand,
    Serve,
}

fn generation() -> std::io::Result<String> {
    let mut bits = [0u8; 16];
    getrandom::fill(&mut bits).map_err(std::io::Error::other)?;
    Ok(bits.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The answer is complete once the TCP listener's address is known; until then the route answers `503`.
struct Identify {
    answer: Arc<OnceLock<Bytes>>,
}

impl Handler for Identify {
    fn handle(&self, request: Request<RequestBody>) -> BoxFuture<'static, Response<ResponseBody>> {
        let (status, body) = match (request.method() == Method::GET, self.answer.get()) {
            (false, _) => (StatusCode::METHOD_NOT_ALLOWED, Bytes::new()),
            (true, Some(answer)) => (StatusCode::OK, answer.clone()),
            (true, None) => (StatusCode::SERVICE_UNAVAILABLE, Bytes::new()),
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
    #[cfg(unix)]
    socket: serve::ServedSocket,
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

    #[cfg(unix)]
    pub fn socket_path(&self) -> &Path {
        self.socket.path()
    }

    pub async fn look_at_repositories(&self) {
        self.watcher.look().await;
    }

    pub async fn shutdown(self) {
        self.watcher.stop().await;
        #[cfg(unix)]
        self.socket.close().await;
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

/// As `start`, in a home that the caller prepared, saying who started the server.
pub async fn start_by(
    home: &Prepared,
    address: SocketAddr,
    started: Started,
) -> Result<Server, ServerError> {
    start_prepared(
        home,
        address,
        WebApp::from_env(),
        watcher::EVERY,
        Waits::default(),
        started,
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
    start_as(paths, address, web_app, look_every, waits, Started::Serve).await
}

pub async fn start_as(
    paths: &Paths,
    address: SocketAddr,
    web_app: WebApp,
    look_every: Duration,
    waits: Waits,
    started: Started,
) -> Result<Server, ServerError> {
    if !matches!(address, SocketAddr::V4(v4) if v4.ip().is_loopback()) {
        return Err(ServerError::NotLoopback(address));
    }
    let own_account = callers::own_account()?;
    callers::check_own_connection(&own_account)?;
    let home = home::prepare(paths)?;
    start_checked(
        &home,
        own_account,
        address,
        web_app,
        look_every,
        waits,
        started,
    )
    .await
}

async fn start_prepared(
    home: &Prepared,
    address: SocketAddr,
    web_app: WebApp,
    look_every: Duration,
    waits: Waits,
    started: Started,
) -> Result<Server, ServerError> {
    if !matches!(address, SocketAddr::V4(v4) if v4.ip().is_loopback()) {
        return Err(ServerError::NotLoopback(address));
    }
    let own_account = callers::own_account()?;
    callers::check_own_connection(&own_account)?;
    start_checked(
        home,
        own_account,
        address,
        web_app,
        look_every,
        waits,
        started,
    )
    .await
}

/// Every file of the home is named through `home`, which only `home::prepare` makes.
#[allow(clippy::too_many_arguments)]
async fn start_checked(
    home: &Prepared,
    own_account: callers::Account,
    address: SocketAddr,
    web_app: WebApp,
    look_every: Duration,
    waits: Waits,
    started: Started,
) -> Result<Server, ServerError> {
    let owner: closing::HomeLock = Arc::new(own(home, waits).await?);
    #[cfg(unix)]
    let (socket_listener, socket_file) = home::bind_socket(home).await?;
    let credentials = Credentials::open(home.credentials())?.holding(Arc::clone(&owner));
    let store = Store::open(home.database())?;
    store.hold_until_closed(Arc::clone(&owner));
    let database = store.path().to_path_buf();
    let app = application().map_err(ServerError::Application)?;
    let mut identity = Identity {
        pid: std::process::id(),
        database: database.clone(),
        interface: Some(interface()),
        schema: Some(crate::store::SCHEMA_VERSION),
        generation: Some(generation()?),
        control: Some(CONTROL),
        executable: Executable::of_this_process(),
        started: Some(started),
        url: None,
    };
    let answer = Arc::new(OnceLock::new());
    let identify: Arc<dyn Handler> = Arc::new(Identify {
        answer: Arc::clone(&answer),
    });
    let watcher = Watcher::start(store.clone(), look_every, GIT_LIMIT, watcher::READ_WAIT);
    let looks = watcher.looks();
    let origin = web::Origin::default();
    let web_routes = web::routes(&app, &store, web_app);
    let command_line_credentials = credentials.clone();
    let binding = HttpBinding::with_request_context(
        app,
        move |parts: &http::request::Parts| {
            let context = caller_of(&parts.headers).map(|caller| {
                Context::watching(store.clone(), caller, looks.clone())
                    .with_credentials(command_line_credentials.clone())
            });
            std::future::ready(context)
        },
        HttpOptions {
            target: TARGET.into(),
            base: BASE.into(),
            ..HttpOptions::default()
        },
    );
    let command_line: remote::Routes = vec![
        (BASE.into(), Arc::new(binding) as Arc<dyn Handler>),
        (IDENTITY.into(), Arc::clone(&identify)),
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
    identity.url = Some(runtime.url.clone());
    let _ = answer.set(Bytes::from(
        serde_json::to_vec(&identity).map_err(std::io::Error::from)?,
    ));
    if let Err(error) = write_runtime(&home.runtime(), &runtime) {
        watcher.stop().await;
        served.close().await;
        return Err(error.into());
    }
    #[cfg(unix)]
    let socket = serve::serve_socket(
        socket_listener,
        socket_file,
        vec![(IDENTITY.into(), identify)],
    );
    Ok(Server {
        #[cfg(unix)]
        socket,
        served,
        published: closing::Published::new(home.runtime(), &runtime),
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

    pub type Account = u32;

    fn unknown(error: peer::CallersUnknown) -> ServerError {
        ServerError::CallersUnknown(error.to_string())
    }

    pub fn own_account() -> Result<u32, ServerError> {
        peer::own_account().map_err(unknown)
    }

    pub fn owner_only(own_uid: u32) -> Admit {
        peer::owner_only(own_uid)
    }

    pub fn check_own_connection(own_uid: &u32) -> Result<(), ServerError> {
        peer::check_own_connection(*own_uid).map_err(unknown)
    }
}

/// Only Linux tells this server which account owns the other end of a loopback connection. Elsewhere it refuses to
/// start rather than answer another account of the machine: docs/decisions/0003-the-caller-check-on-macos-and-windows.md.
#[cfg(not(target_os = "linux"))]
mod callers {
    use std::sync::Arc;

    use super::ServerError;
    use crate::serve::Admit;

    /// Never made: `own_account` always fails here.
    pub struct NoAccount;

    pub type Account = NoAccount;

    pub fn own_account() -> Result<NoAccount, ServerError> {
        Err(ServerError::CallersUnknown(format!(
            "Quadratique cannot yet tell which account connects to its local listener on {}, so it does not serve there: another account of this machine could read and change your issues. Linux is supported; macOS is QUA-33, Windows QUA-34",
            std::env::consts::OS
        )))
    }

    pub fn owner_only(_: NoAccount) -> Admit {
        Arc::new(|_| false)
    }

    pub fn check_own_connection(_: &NoAccount) -> Result<(), ServerError> {
        Ok(())
    }
}

/// Every child process between its `fork` and its `exec` holds the lock too, because a `flock` stays taken while any
/// copy of its descriptor is open.
async fn own(home: &Prepared, waits: Waits) -> Result<File, ServerError> {
    let database = canonical_folder(home.folder())?.join(DATABASE_FILE);
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.lock())?;
    let deadline = Instant::now() + waits.start;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || answers_for(&home.runtime(), &database, waits.probe.min(left)).await {
            return Err(ServerError::Owned(home.database()));
        }
        tokio::time::sleep(POLL).await;
    }
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

/// Why a server gave no identity. A probe over the socket keeps these apart, as the record's state table needs them;
/// the TCP path takes all but a refusal as nobody.
#[derive(Debug)]
pub enum Unanswered {
    /// Confirmed absence or refusal: no entry at the socket's name, `ENOENT` or `ECONNREFUSED` at `connect`, or a
    /// connection that the server closed. Over TCP, any failed `connect` but a denied one.
    NotListening(std::io::Error),
    /// `EPERM` or `EACCES`: a sandbox, or another account's folder.
    NotPermitted(std::io::Error),
    /// Any other failure to reach the socket: `ENOTDIR`, `ELOOP`, `ENAMETOOLONG`, …
    PathError(std::io::Error),
    /// The entry at the socket's name is not one to trust yet: not a socket of this account with mode 0600.
    NotReady(String),
    OwnerOnly,
    /// It accepted the connection and gave no full answer within the probe limit.
    Stalled,
    /// It answered with something that is not an identity.
    Malformed(String),
}

impl Unanswered {
    pub fn of_connect(error: std::io::Error) -> Unanswered {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            Unanswered::NotPermitted(error)
        } else {
            Unanswered::NotListening(error)
        }
    }

    #[cfg(unix)]
    fn of_socket_connect(error: std::io::Error) -> Unanswered {
        match error.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                Unanswered::NotListening(error)
            }
            std::io::ErrorKind::PermissionDenied => Unanswered::NotPermitted(error),
            _ => Unanswered::PathError(error),
        }
    }
}

/// The longest identity answer, head and body, that a probe reads.
pub const IDENTITY_LIMIT: usize = 64 * 1024;

/// The answer of the server at `address` to `GET /server` within the probe limit, or why it gave none.
pub async fn identity(address: SocketAddr) -> Result<Identity, Unanswered> {
    identity_within(address, PROBE_LIMIT).await
}

async fn identity_within(address: SocketAddr, limit: Duration) -> Result<Identity, Unanswered> {
    tokio::time::timeout(limit, ask_identity(address))
        .await
        .unwrap_or(Err(Unanswered::Stalled))
}

async fn ask_identity(address: SocketAddr) -> Result<Identity, Unanswered> {
    let stream = TcpStream::connect(address)
        .await
        .map_err(Unanswered::of_connect)?;
    ask_identity_on(stream, &address.to_string()).await
}

/// The answer of the server of `home` to `GET /server` on its socket within the probe limit, or why it gave none.
/// The socket is trusted only as a socket of this account with mode 0600. `EINTR` and `EAGAIN` at `connect` are
/// tried again within the same limit.
#[cfg(unix)]
pub async fn identity_over_socket(home: &Prepared) -> Result<Identity, Unanswered> {
    identity_over_socket_within(home, PROBE_LIMIT).await
}

#[cfg(unix)]
pub async fn identity_over_socket_within(
    home: &Prepared,
    limit: Duration,
) -> Result<Identity, Unanswered> {
    let socket = home.socket();
    match home::socket_entry(&socket) {
        home::Entry::Ready => {}
        home::Entry::Missing => {
            return Err(Unanswered::NotListening(
                std::io::ErrorKind::NotFound.into(),
            ));
        }
        home::Entry::Denied(error) => return Err(Unanswered::NotPermitted(error)),
        home::Entry::Untrusted(why) => return Err(Unanswered::NotReady(why)),
        home::Entry::PathError(error) => return Err(Unanswered::PathError(error)),
    }
    let asking = async {
        let stream = loop {
            match tokio::net::UnixStream::connect(&socket).await {
                Ok(stream) => break stream,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    tokio::time::sleep(POLL).await;
                }
                Err(error) => return Err(Unanswered::of_socket_connect(error)),
            }
        };
        ask_identity_on(stream, SOCKET_AUTHORITY).await
    };
    tokio::time::timeout(limit, asking)
        .await
        .unwrap_or(Err(Unanswered::Stalled))
}

/// The one reader of an identity answer, for both transports: it reads at most `IDENTITY_LIMIT` bytes and one more,
/// and refuses a longer answer before it parses anything.
async fn ask_identity_on(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    host: &str,
) -> Result<Identity, Unanswered> {
    let gone = |error: std::io::Error| match error.kind() {
        std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::ConnectionAborted => Unanswered::NotListening(error),
        _ => Unanswered::Malformed(format!("the connection failed: {error}")),
    };
    let request = format!("GET {IDENTITY} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.map_err(gone)?;
    let mut answer = Vec::new();
    (&mut stream)
        .take(IDENTITY_LIMIT as u64 + 1)
        .read_to_end(&mut answer)
        .await
        .map_err(gone)?;
    if answer.is_empty() {
        return Err(Unanswered::NotListening(
            std::io::ErrorKind::UnexpectedEof.into(),
        ));
    }
    if answer.len() > IDENTITY_LIMIT {
        return Err(Unanswered::Malformed(format!(
            "the answer is longer than {IDENTITY_LIMIT} bytes"
        )));
    }
    let malformed = |why: &str| Unanswered::Malformed(why.to_owned());
    let answer = String::from_utf8(answer).map_err(|_| malformed("the answer is not UTF-8"))?;
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .ok_or_else(|| malformed("the answer is not HTTP"))?;
    match head.split_whitespace().nth(1) {
        Some("200") => serde_json::from_str(body)
            .map_err(|error| Unanswered::Malformed(format!("the identity is not valid: {error}"))),
        Some("403") if body == OWNER_ONLY => Err(Unanswered::OwnerOnly),
        Some(status) => Err(Unanswered::Malformed(format!(
            "the answer has status {status}"
        ))),
        None => Err(malformed("the answer has no status")),
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

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::{Paths, ServerError};

    fn paths_with(variables: &[(&str, &str)]) -> Result<Paths, ServerError> {
        Paths::from_variables(|name| {
            variables
                .iter()
                .find(|(variable, _)| *variable == name)
                .map(|(_, value)| OsString::from(value))
        })
    }

    fn folder(parts: &[&str]) -> PathBuf {
        let mut root = std::path::absolute(std::path::MAIN_SEPARATOR_STR).expect("the root");
        root.extend(parts);
        root
    }

    #[test]
    fn quadratique_home_names_the_folder_on_every_platform() {
        let home = folder(&["q"]);
        let paths =
            paths_with(&[("QUADRATIQUE_HOME", home.to_str().expect("a path"))]).expect("paths");

        assert_eq!(paths.database, home.join("workspace.db"));
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_the_store_is_in_local_app_data() {
        let local = folder(&["Users", "me", "AppData", "Local"]);
        let paths =
            paths_with(&[("LOCALAPPDATA", local.to_str().expect("a path"))]).expect("paths");

        assert_eq!(
            paths.database,
            local.join("quadratique").join("workspace.db")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn on_macos_the_store_is_in_application_support() {
        let paths = paths_with(&[("HOME", "/Users/me")]).expect("paths");

        assert_eq!(
            paths.database,
            PathBuf::from("/Users/me/Library/Application Support/quadratique/workspace.db")
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn elsewhere_the_store_is_in_the_xdg_data_folder() {
        let paths = paths_with(&[("HOME", "/home/me")]).expect("paths");

        assert_eq!(
            paths.database,
            PathBuf::from("/home/me/.local/share/quadratique/workspace.db")
        );
    }
}
