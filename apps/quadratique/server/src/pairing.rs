//! How the web app gets in: `quadratique ui` asks the server, over the socket, for a one-time code; the page redeems
//! the code at `POST /session` for the bearer, saves it, and sends it on every request.
//! docs/decisions/0004-a-socket-for-commands-and-a-token-for-the-browser.md, §4 "Pairing contract".
//!
//! By default the bearer is one per data folder, kept in `web-token`, so that a bookmark or an open tab keeps working
//! after a restart or an upgrade; `quadratique server sign-out` makes a new one, which signs every browser out. In
//! strict mode the bearer is made at each start and kept in memory only.
//!
//! No secret is ever written to a log, an error or a `Debug` output: [`Secret`] prints as `Secret(…)`.

use std::fs::File;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant};

use acid_cat_core::remote::{Handler, RequestBody, ResponseBody};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use futures::future::BoxFuture;
use http::{HeaderValue, Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};
use serde::{Deserialize, Serialize};

use crate::home::{self, Prepared};

/// The page redeems its one-time code here, on TCP.
pub const SESSION: &str = "/session";
/// `ui` asks for a code here, on the socket.
pub const PAIR: &str = "/server/pair";
/// `server sign-out` makes a new bearer here, on the socket.
pub const SIGN_OUT: &str = "/server/sign-out";
pub const TOKEN_FILE: &str = "web-token";
/// The fragment of the link that carries the code: `http://127.0.0.1:<port>/#pair=<code>`.
pub const FRAGMENT: &str = "pair";
pub const CODE_LIFETIME: Duration = Duration::from_secs(60);
pub const PENDING_LIMIT: usize = 16;
/// The longest body of `POST /session`, and of the socket's pairing requests.
pub const BODY_LIMIT: usize = 1024;
/// Whether a request of the web app must carry the bearer. Until the cutover (QUA-58) a request without one is served,
/// as the kernel check still guards TCP; a request with a bearer that is not the current one is refused already.
pub const REQUIRED: Requirement = Requirement::Optional;
/// The one sentence of a page that is not paired, also the message of a refused bearer.
pub const NOT_PAIRED: &str =
    "To open Quadratique in this browser, run `quadratique ui --open` in a terminal.";

const SECRET_BYTES: usize = 32;
/// A base64url secret of 32 bytes is 43 characters; a file of the bearer is that and a line end.
const TOKEN_FILE_LIMIT: u64 = 64;
const BOOTSTRAP_PREFIX: &str = "open-";
const BOOTSTRAP_SUFFIX: &str = ".html";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requirement {
    Optional,
    Required,
}

/// 32 random bytes. Compared in constant time, and never printed.
#[derive(Clone)]
pub struct Secret([u8; SECRET_BYTES]);

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Secret(…)")
    }
}

impl Secret {
    fn random() -> std::io::Result<Secret> {
        let mut bytes = [0u8; SECRET_BYTES];
        getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
        Ok(Secret(bytes))
    }

    fn encoded(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }

    fn decode(text: &str) -> Option<Secret> {
        let bytes = URL_SAFE_NO_PAD.decode(text.trim()).ok()?;
        Some(Secret(bytes.try_into().ok()?))
    }

    /// Whether `presented` encodes this secret. Its length is public: every bearer has the same.
    fn matches(&self, presented: &str) -> bool {
        let Ok(bytes) = URL_SAFE_NO_PAD.decode(presented) else {
            return false;
        };
        bytes.len() == SECRET_BYTES
            && bytes
                .iter()
                .zip(self.0.iter())
                .fold(0u8, |difference, (a, b)| difference | (a ^ b))
                == 0
    }
}

/// How long the bearer lives.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Lifetime {
    /// Kept in this file of the data folder, across restarts, until it is rotated.
    Folder(PathBuf),
    /// Made at this start, in memory only (strict mode).
    Start,
}

pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

struct Pending {
    code: Secret,
    expires: Instant,
    bootstrap: Option<PathBuf>,
}

/// The bearer of the web app and the codes waiting to be redeemed.
pub struct Pairing {
    bearer: RwLock<Secret>,
    lifetime: Lifetime,
    folder: PathBuf,
    pending: Mutex<Vec<Pending>>,
    clock: Clock,
}

impl std::fmt::Debug for Pairing {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Pairing")
            .field("lifetime", &self.lifetime)
            .finish_non_exhaustive()
    }
}

/// What `ui` gets: the address of the web app, the link that carries a code, and the bootstrap file that opens it.
#[derive(Clone, Serialize, Deserialize)]
pub struct Issued {
    pub url: String,
    pub link: String,
    pub file: Option<PathBuf>,
}

impl std::fmt::Debug for Issued {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Issued")
            .field("url", &self.url)
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IssueError {
    #[error(
        "{PENDING_LIMIT} links of quadratique ui wait to be opened; open one of them, or wait a minute and run it again"
    )]
    TooMany,
    #[error("the link could not be made: {0}")]
    Io(#[from] std::io::Error),
}

impl Pairing {
    /// The pairing of the server of `home`: it removes the bootstrap files that an earlier server left, then loads the
    /// folder's bearer, or makes it when it is missing or damaged. A strict home gets a bearer of this start, and the
    /// folder's file is removed.
    pub fn open(home: &Prepared, strict: bool) -> std::io::Result<Arc<Pairing>> {
        Pairing::open_with(home.folder(), strict, Arc::new(Instant::now))
    }

    pub fn open_with(folder: &Path, strict: bool, clock: Clock) -> std::io::Result<Arc<Pairing>> {
        remove_bootstraps(folder);
        let file = folder.join(TOKEN_FILE);
        let (bearer, lifetime) = if strict {
            remove(&file)?;
            (Secret::random()?, Lifetime::Start)
        } else {
            let bearer = match read_token(&file) {
                Some(bearer) => bearer,
                None => {
                    let bearer = Secret::random()?;
                    write_token(&file, &bearer)?;
                    bearer
                }
            };
            (bearer, Lifetime::Folder(file))
        };
        Ok(Arc::new(Pairing {
            bearer: RwLock::new(bearer),
            lifetime,
            folder: folder.to_path_buf(),
            pending: Mutex::new(Vec::new()),
            clock,
        }))
    }

    /// A new code for the web app at `origin`, valid for [`CODE_LIFETIME`], with a bootstrap file when asked. The
    /// server's listener and routes exist before this is called, so the link names a listener of this server.
    pub fn issue(
        self: &Arc<Self>,
        origin: SocketAddr,
        bootstrap: bool,
    ) -> Result<Issued, IssueError> {
        let now = (self.clock)();
        let code = Secret::random()?;
        let url = format!("http://{origin}/");
        let link = format!("{url}#{FRAGMENT}={}", code.encoded());
        let mut pending = self.pending();
        sweep_locked(&mut pending, now);
        if pending.len() >= PENDING_LIMIT {
            return Err(IssueError::TooMany);
        }
        let file = if bootstrap {
            Some(write_bootstrap(&self.folder, &link)?)
        } else {
            None
        };
        pending.push(Pending {
            code,
            expires: now + CODE_LIFETIME,
            bootstrap: file.clone(),
        });
        drop(pending);
        self.sweep_when_expired();
        Ok(Issued { url, link, file })
    }

    /// The bearer for a code that is pending and not expired; the code is deleted in the same step, so it redeems once.
    pub fn redeem(&self, code: &str) -> Option<String> {
        let now = (self.clock)();
        let mut pending = self.pending();
        sweep_locked(&mut pending, now);
        let found = pending
            .iter()
            .position(|waiting| waiting.code.matches(code))?;
        let redeemed = pending.swap_remove(found);
        drop(pending);
        if let Some(file) = redeemed.bootstrap {
            let _ = std::fs::remove_file(file);
        }
        Some(self.current().encoded())
    }

    /// Whether `presented` is the current bearer.
    pub fn admits(&self, presented: &str) -> bool {
        self.current().matches(presented)
    }

    /// A new bearer: every saved one stops working, and the codes that wait are dropped with their bootstrap files.
    pub fn rotate(&self) -> std::io::Result<()> {
        let bearer = Secret::random()?;
        if let Lifetime::Folder(file) = &self.lifetime {
            write_token(file, &bearer)?;
        }
        *self.bearer.write().unwrap_or_else(PoisonError::into_inner) = bearer;
        for dropped in self.pending().drain(..) {
            if let Some(file) = dropped.bootstrap {
                let _ = std::fs::remove_file(file);
            }
        }
        Ok(())
    }

    /// Drops the expired codes and removes their bootstrap files.
    pub fn sweep(&self) {
        let now = (self.clock)();
        sweep_locked(&mut self.pending(), now);
    }

    /// Whether the bearer lasts across restarts (false in strict mode).
    pub fn lasting(&self) -> bool {
        matches!(self.lifetime, Lifetime::Folder(_))
    }

    fn current(&self) -> Secret {
        self.bearer
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Vec<Pending>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Outside a runtime (a unit test), the next call sweeps instead.
    fn sweep_when_expired(self: &Arc<Self>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let pairing: Weak<Pairing> = Arc::downgrade(self);
        runtime.spawn(async move {
            tokio::time::sleep(CODE_LIFETIME + Duration::from_secs(1)).await;
            if let Some(pairing) = pairing.upgrade() {
                pairing.sweep();
            }
        });
    }
}

/// A server that stops removes the bootstrap files of the codes that still wait.
impl Drop for Pairing {
    fn drop(&mut self) {
        for dropped in self.pending().drain(..) {
            if let Some(file) = dropped.bootstrap {
                let _ = std::fs::remove_file(file);
            }
        }
    }
}

fn sweep_locked(pending: &mut Vec<Pending>, now: Instant) {
    pending.retain(|waiting| {
        let live = waiting.expires > now;
        if !live && let Some(file) = &waiting.bootstrap {
            let _ = std::fs::remove_file(file);
        }
        live
    });
}

fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The bearer in `file`, or nothing when it is missing or is not a small regular file holding one.
fn read_token(file: &Path) -> Option<Secret> {
    let looked = std::fs::symlink_metadata(file).ok()?;
    if !looked.is_file() || looked.len() > TOKEN_FILE_LIMIT {
        return None;
    }
    let opened = File::open(file).ok()?;
    let mut text = String::new();
    opened
        .take(TOKEN_FILE_LIMIT)
        .read_to_string(&mut text)
        .ok()?;
    Secret::decode(&text)
}

/// Writes the bearer as every file of the home is written: a private partial file, renamed over `file`.
fn write_token(file: &Path, bearer: &Secret) -> std::io::Result<()> {
    home::write_atomically(file, format!("{}\n", bearer.encoded()).as_bytes())
}

/// `<folder>/open-<random>.html`, which sends the browser to `link` without the code ever being in a process's
/// arguments.
fn write_bootstrap(folder: &Path, link: &str) -> std::io::Result<PathBuf> {
    let mut name = [0u8; 16];
    getrandom::fill(&mut name).map_err(std::io::Error::other)?;
    let file = folder.join(format!(
        "{BOOTSTRAP_PREFIX}{}{BOOTSTRAP_SUFFIX}",
        URL_SAFE_NO_PAD.encode(name)
    ));
    let page = format!(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<meta name=\"referrer\" content=\"no-referrer\">\n<meta http-equiv=\"refresh\" content=\"0; url={link}\">\n<title>Quadratique</title>\n"
    );
    let written =
        home::create_private(&file).and_then(|mut opened| opened.write_all(page.as_bytes()));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&file);
        return Err(error);
    }
    Ok(file)
}

/// The bootstrap files an earlier server left, after a crash.
fn remove_bootstraps(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(BOOTSTRAP_PREFIX)
            && name.ends_with(BOOTSTRAP_SUFFIX)
            && entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[derive(Deserialize)]
struct Redemption {
    code: String,
}

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub token: String,
}

/// `POST /session`: a code for the bearer. The `Host` and the exact `Origin` are checked before, by the route's guard.
pub struct SessionRoute {
    pub pairing: Arc<Pairing>,
}

impl Handler for SessionRoute {
    fn handle(&self, request: Request<RequestBody>) -> BoxFuture<'static, Response<ResponseBody>> {
        if request.method() != Method::POST {
            let mut response = answer(StatusCode::METHOD_NOT_ALLOWED, "Only POST.");
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("POST"));
            return Box::pin(std::future::ready(response));
        }
        let pairing = Arc::clone(&self.pairing);
        Box::pin(async move {
            let Some(redemption) = read_bounded(request.into_body(), BODY_LIMIT)
                .await
                .and_then(|body| serde_json::from_slice::<Redemption>(&body).ok())
            else {
                return answer(StatusCode::BAD_REQUEST, "The body is not a code.");
            };
            match pairing.redeem(&redemption.code) {
                Some(token) => {
                    let body = serde_json::to_vec(&Session { token }).unwrap_or_default();
                    let mut response = Response::new(Full::new(Bytes::from(body)).boxed_unsync());
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    no_store(&mut response);
                    response
                }
                None => answer(
                    StatusCode::FORBIDDEN,
                    "This link was used already or has expired: run `quadratique ui --open` in a terminal.",
                ),
            }
        })
    }
}

/// The bearer on a route of the web app: `Authorization: Bearer <bearer>`. A request with another bearer gets `401`;
/// one with none is served until the requirement is [`Requirement::Required`].
pub struct BearerGuard {
    pub pairing: Arc<Pairing>,
    pub requirement: Requirement,
    pub inner: Arc<dyn Handler>,
}

impl Handler for BearerGuard {
    fn handle(
        &self,
        mut request: Request<RequestBody>,
    ) -> BoxFuture<'static, Response<ResponseBody>> {
        let presented: Vec<Option<String>> = request
            .headers()
            .get_all(header::AUTHORIZATION)
            .iter()
            .map(|value| {
                value
                    .to_str()
                    .ok()
                    .and_then(|value| value.strip_prefix("Bearer "))
                    .map(str::to_owned)
            })
            .collect();
        let admitted = match presented.as_slice() {
            [] => self.requirement == Requirement::Optional,
            [Some(bearer)] => self.pairing.admits(bearer),
            _ => false,
        };
        if !admitted {
            return Box::pin(std::future::ready(unauthorized()));
        }
        request.headers_mut().remove(header::AUTHORIZATION);
        self.inner.handle(request)
    }
}

/// A call failure of AcidCat's HTTP binding, so that a client of the route reads a message, and the status the page
/// acts on.
fn unauthorized() -> Response<ResponseBody> {
    let body = serde_json::json!({ "kind": "unavailable", "message": NOT_PAIRED }).to_string();
    let mut response = Response::new(Full::new(Bytes::from(body)).boxed_unsync());
    *response.status_mut() = StatusCode::UNAUTHORIZED;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    no_store(&mut response);
    response
}

fn answer(status: StatusCode, message: &str) -> Response<ResponseBody> {
    let mut response = Response::new(Full::new(Bytes::from(message.to_owned())).boxed_unsync());
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    no_store(&mut response);
    response
}

fn no_store(response: &mut Response<ResponseBody>) {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
}

/// The body, or nothing when it fails or is longer than `limit`.
pub async fn read_bounded(mut body: RequestBody, limit: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame.ok()?.into_data() {
            if data.len() > limit - bytes.len() {
                return None;
            }
            bytes.extend_from_slice(&data);
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::{CODE_LIFETIME, IssueError, PENDING_LIMIT, Pairing, TOKEN_FILE};

    struct Fixture {
        pairing: Arc<Pairing>,
        now: Arc<Mutex<Instant>>,
        folder: tempfile::TempDir,
    }

    impl Fixture {
        fn new(strict: bool) -> Fixture {
            let folder = tempfile::tempdir().expect("a folder");
            Fixture::in_folder(folder, strict)
        }

        fn in_folder(folder: tempfile::TempDir, strict: bool) -> Fixture {
            let now = Arc::new(Mutex::new(Instant::now()));
            let clock = Arc::clone(&now);
            let pairing = Pairing::open_with(
                folder.path(),
                strict,
                Arc::new(move || *clock.lock().expect("the clock")),
            )
            .expect("a pairing");
            Fixture {
                pairing,
                now,
                folder,
            }
        }

        fn advance(&self, by: Duration) {
            *self.now.lock().expect("the clock") += by;
        }

        fn issue(&self, bootstrap: bool) -> super::Issued {
            self.pairing
                .issue(origin(), bootstrap)
                .expect("a code is issued")
        }
    }

    fn origin() -> SocketAddr {
        "127.0.0.1:4100".parse().expect("an address")
    }

    fn code_of(link: &str) -> String {
        link.split_once("#pair=")
            .expect("a code in the link")
            .1
            .to_owned()
    }

    #[test]
    fn a_code_redeems_once_for_the_bearer() {
        let fixture = Fixture::new(false);
        let issued = fixture.issue(false);
        assert_eq!(issued.url, "http://127.0.0.1:4100/");
        let code = code_of(&issued.link);

        let first = fixture.pairing.redeem(&code);
        let again = fixture.pairing.redeem(&code);

        let bearer = first.expect("the bearer");
        assert!(fixture.pairing.admits(&bearer));
        assert_eq!(again, None);
    }

    #[test]
    fn a_code_after_its_lifetime_is_refused_and_its_bootstrap_file_is_removed() {
        let fixture = Fixture::new(false);
        let issued = fixture.issue(true);
        let file = issued.file.expect("a bootstrap file");
        assert!(file.exists());

        fixture.advance(CODE_LIFETIME);

        assert_eq!(fixture.pairing.redeem(&code_of(&issued.link)), None);
        assert!(!file.exists(), "the expired code's bootstrap file stays");
        let other = fixture.issue(true).file.expect("another bootstrap file");
        fixture.advance(CODE_LIFETIME + Duration::from_secs(1));
        fixture.pairing.sweep();
        assert!(!other.exists(), "the sweep left an expired bootstrap file");
    }

    #[test]
    fn a_code_beyond_the_pending_limit_is_refused_until_one_expires() {
        let fixture = Fixture::new(false);
        for _ in 0..PENDING_LIMIT {
            fixture.issue(false);
        }

        let refused = fixture.pairing.issue(origin(), false);

        assert!(matches!(refused, Err(IssueError::TooMany)), "{refused:?}");
        fixture.advance(CODE_LIFETIME);
        assert!(fixture.pairing.issue(origin(), false).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn the_bootstrap_file_is_private_holds_the_link_and_is_gone_after_use() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new(false);
        let issued = fixture.issue(true);
        let file = issued.file.clone().expect("a bootstrap file");

        let mode = std::fs::metadata(&file)
            .expect("the file")
            .permissions()
            .mode();
        let page = std::fs::read_to_string(&file).expect("the page");
        fixture
            .pairing
            .redeem(&code_of(&issued.link))
            .expect("the bearer");

        assert_eq!(mode & 0o777, 0o600);
        assert!(page.contains(&format!("url={}", issued.link)), "{page}");
        assert!(!file.exists(), "the used code's bootstrap file stays");
    }

    #[test]
    fn a_private_file_is_never_made_over_something_already_there() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = folder.path().join("taken");
        std::fs::write(&path, "already here").expect("a file");

        let made = crate::home::create_private(&path);

        assert_eq!(
            made.map(|_| ()).map_err(|error| error.kind()),
            Err(std::io::ErrorKind::AlreadyExists)
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file"),
            "already here"
        );
    }

    #[test]
    fn the_folders_bearer_survives_a_restart_and_a_rotation_changes_it() {
        let fixture = Fixture::new(false);
        let bearer = fixture
            .pairing
            .redeem(&code_of(&fixture.issue(false).link))
            .expect("the bearer");
        let Fixture {
            pairing, folder, ..
        } = fixture;
        drop(pairing);

        let restarted = Fixture::in_folder(folder, false);

        assert!(
            restarted.pairing.admits(&bearer),
            "a restart changed the bearer"
        );
        restarted.pairing.rotate().expect("a rotation");
        assert!(
            !restarted.pairing.admits(&bearer),
            "the rotation kept the bearer"
        );
        let rotated = restarted
            .pairing
            .redeem(&code_of(&restarted.issue(false).link))
            .expect("the new bearer");
        let Fixture {
            pairing, folder, ..
        } = restarted;
        drop(pairing);
        let again = Fixture::in_folder(folder, false);
        assert!(again.pairing.admits(&rotated), "the rotation was not kept");
    }

    #[test]
    fn a_strict_bearer_changes_at_each_start_and_leaves_no_file() {
        let fixture = Fixture::new(false);
        let lasting = fixture
            .pairing
            .redeem(&code_of(&fixture.issue(false).link))
            .expect("the bearer");
        let Fixture {
            pairing, folder, ..
        } = fixture;
        drop(pairing);

        let strict = Fixture::in_folder(folder, true);
        let first = strict
            .pairing
            .redeem(&code_of(&strict.issue(false).link))
            .expect("a bearer");
        let Fixture {
            pairing, folder, ..
        } = strict;
        drop(pairing);
        let restarted = Fixture::in_folder(folder, true);

        assert!(!restarted.pairing.admits(&lasting));
        assert!(
            !restarted.pairing.admits(&first),
            "a strict restart kept the bearer"
        );
        assert!(!restarted.folder.path().join(TOKEN_FILE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_folders_bearer_is_in_a_private_file_and_a_damaged_one_is_replaced() {
        use std::os::unix::fs::PermissionsExt;

        let folder = tempfile::tempdir().expect("a folder");
        std::fs::write(folder.path().join(TOKEN_FILE), "not a bearer\n").expect("a damaged file");

        let fixture = Fixture::in_folder(folder, false);

        let file = fixture.folder.path().join(TOKEN_FILE);
        let mode = std::fs::metadata(&file)
            .expect("the file")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let text = std::fs::read_to_string(&file).expect("the bearer");
        assert!(fixture.pairing.admits(text.trim()));
    }

    #[test]
    fn a_restart_removes_the_bootstrap_files_left_by_a_crash() {
        let folder = tempfile::tempdir().expect("a folder");
        let left = folder.path().join("open-left.html");
        std::fs::write(&left, "left").expect("a left file");
        let kept = folder.path().join("opened.txt");
        std::fs::write(&kept, "not a bootstrap file").expect("another file");

        let _fixture = Fixture::in_folder(folder, false);

        assert!(!left.exists());
        assert!(kept.exists());
    }

    struct Answers;

    impl acid_cat_core::remote::Handler for Answers {
        fn handle(
            &self,
            request: http::Request<acid_cat_core::remote::RequestBody>,
        ) -> futures::future::BoxFuture<'static, http::Response<acid_cat_core::remote::ResponseBody>>
        {
            use http_body_util::BodyExt;
            let forwarded = request.headers().contains_key(http::header::AUTHORIZATION);
            let body = if forwarded { "forwarded" } else { "ok" };
            Box::pin(std::future::ready(http::Response::new(
                http_body_util::Full::new(bytes::Bytes::from(body)).boxed_unsync(),
            )))
        }
    }

    async fn guarded(
        requirement: super::Requirement,
        pairing: &Arc<Pairing>,
        bearer: Option<&str>,
    ) -> (u16, String) {
        use acid_cat_core::remote::Handler;
        use http_body_util::BodyExt;

        let guard = super::BearerGuard {
            pairing: Arc::clone(pairing),
            requirement,
            inner: Arc::new(Answers),
        };
        let mut request = http::Request::builder().uri("/web/capabilities/project.list");
        if let Some(bearer) = bearer {
            request = request.header(http::header::AUTHORIZATION, format!("Bearer {bearer}"));
        }
        let request = request
            .body(
                http_body_util::Empty::<bytes::Bytes>::new()
                    .map_err(|never| match never {})
                    .boxed_unsync(),
            )
            .expect("a request");
        let response = guard.handle(request).await;
        let status = response.status().as_u16();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("the body")
            .to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    /// The cutover (QUA-58) sets the requirement to `Required`: a request without the bearer then gets the `401` that
    /// the page acts on, as one with a stale bearer already does.
    #[tokio::test]
    async fn once_the_bearer_is_required_a_request_without_it_gets_401_and_the_bearer_never_reaches_the_route()
     {
        use super::Requirement::{Optional, Required};

        let fixture = Fixture::new(false);
        let bearer = fixture
            .pairing
            .redeem(&code_of(&fixture.issue(false).link))
            .expect("the bearer");

        let missing = guarded(Required, &fixture.pairing, None).await;
        let stale = guarded(Required, &fixture.pairing, Some(&"A".repeat(43))).await;
        let current = guarded(Required, &fixture.pairing, Some(&bearer)).await;
        let missing_today = guarded(Optional, &fixture.pairing, None).await;
        let stale_today = guarded(Optional, &fixture.pairing, Some(&"A".repeat(43))).await;

        assert_eq!(missing.0, 401);
        assert!(missing.1.contains("quadratique ui --open"), "{}", missing.1);
        assert_eq!(stale.0, 401);
        assert_eq!(current, (200, "ok".to_owned()));
        assert_eq!(missing_today, (200, "ok".to_owned()));
        assert_eq!(stale_today.0, 401);
        assert_eq!(
            super::REQUIRED,
            Optional,
            "QUA-58 requires the bearer, not this step"
        );
    }

    #[test]
    fn no_debug_output_shows_a_secret() {
        let fixture = Fixture::new(false);
        let issued = fixture.issue(false);
        let code = code_of(&issued.link);

        let shown = format!("{issued:?} {:?}", fixture.pairing);

        assert!(!shown.contains(&code), "{shown}");
    }
}
