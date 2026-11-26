use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

use acid_cat_core::Application;
use acid_cat_core::http::{HttpOptions, HttpSurface};
use acid_cat_core::remote::{Handler, RequestBody, ResponseBody, Routes};
use bytes::Bytes;
use futures::future::BoxFuture;
use http::{HeaderValue, Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};

use crate::app::Context;
use crate::assets::media_type;
use crate::domain::{Actor, Binding, Caller, WorkingCopy};
use crate::pairing::{BearerGuard, Pairing};
use crate::store::Store;

pub const WEB_TARGET: &str = "web";
pub const WEB_BASE: &str = "/web";
pub const WEBMCP_TARGET: &str = "webmcp";
pub const WEBMCP_BASE: &str = "/webmcp";
pub const APP_BASE: &str = "/";
pub const APP_VARIABLE: &str = "QUADRATIQUE_WEB";
#[cfg(not(embedded_web))]
const BUILT_APP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../web/dist");
const INDEX: &str = "index.html";
const CONTENT_SECURITY_POLICY: &str =
    "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

#[cfg(embedded_web)]
mod embedded {
    use super::EmbeddedFile;

    include!(concat!(env!("OUT_DIR"), "/web_app.rs"));
}

#[derive(Debug, PartialEq, Eq)]
pub struct EmbeddedFile {
    pub relative_path: &'static str,
    pub media_type: &'static str,
    pub bytes: &'static [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebApp {
    Directory(PathBuf),
    Embedded(&'static [EmbeddedFile]),
}

impl WebApp {
    pub fn from_env() -> WebApp {
        match std::env::var_os(APP_VARIABLE).filter(|value| !value.is_empty()) {
            Some(directory) => WebApp::Directory(PathBuf::from(directory)),
            None => WebApp::built(),
        }
    }

    #[cfg(embedded_web)]
    fn built() -> WebApp {
        WebApp::Embedded(embedded::FILES)
    }

    #[cfg(not(embedded_web))]
    fn built() -> WebApp {
        WebApp::Directory(PathBuf::from(BUILT_APP))
    }

    fn read(&self, relative: &Path) -> Option<AppFile> {
        match self {
            WebApp::Directory(directory) => {
                let file = inside(directory, relative)?;
                let bytes = std::fs::read(&file).ok()?;
                Some(AppFile {
                    bytes: Bytes::from(bytes),
                    media_type: media_type(&file),
                })
            }
            WebApp::Embedded(files) => {
                let wanted = relative.to_str()?;
                let file = files.iter().find(|file| file.relative_path == wanted)?;
                Some(AppFile {
                    bytes: Bytes::from_static(file.bytes),
                    media_type: file.media_type,
                })
            }
        }
    }
}

struct AppFile {
    bytes: Bytes,
    media_type: &'static str,
}

#[derive(Clone, Default)]
pub struct Origin(Arc<OnceLock<SocketAddr>>);

impl Origin {
    pub fn set(&self, address: SocketAddr) {
        let _ = self.0.set(address);
    }

    pub fn address(&self) -> Option<SocketAddr> {
        self.0.get().copied()
    }

    /// Only the numeric address: `localhost` may resolve to `[::1]`, where another account can listen on the same port.
    fn hosts(&self) -> Vec<String> {
        self.0
            .get()
            .map(|address| vec![address.to_string()])
            .unwrap_or_default()
    }
}

pub fn caller(binding: Binding) -> Caller {
    let login = ["USER", "LOGNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
    let actor = Actor::detect(&BTreeMap::new(), binding, None, login.as_deref())
        .expect("an empty environment names no role, so detection cannot fail");
    Caller {
        actor,
        working_copy: WorkingCopy::default(),
    }
}

/// The web app's routes on TCP: the two data targets behind the bearer, and the app's files. Nothing else answers on
/// TCP: the command line and the control requests use the socket.
pub fn routes(
    app: &Arc<Application<Context>>,
    store: &Store,
    web_app: WebApp,
    pairing: &Arc<Pairing>,
) -> Routes {
    let target = |target: &str, base: &str, binding: Binding| {
        let context = Arc::new(Context::new(store.clone(), caller(binding)));
        let options = HttpOptions {
            target: target.into(),
            base: base.into(),
            ..HttpOptions::default()
        };
        let surface = Arc::new(HttpSurface::new(app.clone(), context, options));
        Arc::new(BearerGuard {
            pairing: Arc::clone(pairing),
            inner: surface,
        }) as Arc<dyn Handler>
    };
    vec![
        (WEB_BASE.into(), target(WEB_TARGET, WEB_BASE, Binding::Web)),
        (
            WEBMCP_BASE.into(),
            target(WEBMCP_TARGET, WEBMCP_BASE, Binding::WebMcp),
        ),
        (
            APP_BASE.into(),
            Arc::new(Files(web_app)) as Arc<dyn Handler>,
        ),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginHeader {
    Refused,
    OwnOnly,
    /// Present, and exactly this server's own origin.
    Required,
}

pub fn guarded(origin: &Origin, routes: Routes, origin_header: OriginHeader) -> Routes {
    routes
        .into_iter()
        .map(|(base, inner)| {
            let guard: Arc<dyn Handler> = Arc::new(SameOrigin {
                origin: origin.clone(),
                origin_header,
                inner,
            });
            (base, guard)
        })
        .collect()
}

/// The `Host` check refuses a page of another site whose name was made to resolve to the loopback interface; a
/// refused request gets `403` with no CORS header, so a browser's preflight from another origin fails too. A request
/// names one `Host`, and an absolute-form request line names that same authority.
struct SameOrigin {
    origin: Origin,
    origin_header: OriginHeader,
    inner: Arc<dyn Handler>,
}

impl Handler for SameOrigin {
    fn handle(
        &self,
        mut request: Request<RequestBody>,
    ) -> BoxFuture<'static, Response<ResponseBody>> {
        let hosts = self.origin.hosts();
        let mut named = request.headers().get_all(header::HOST).iter();
        // Exactly one `Host`, and an absolute-form request line that names the same authority or none.
        let host = match (named.next(), named.next()) {
            (Some(value), None) => value.to_str().ok(),
            _ => None,
        }
        .filter(|host| {
            request
                .uri()
                .authority()
                .is_none_or(|authority| authority.as_str() == *host)
        });
        let own_host = host.filter(|host| hosts.iter().any(|own| own == host));
        if self.origin_header == OriginHeader::Refused
            && request.headers().contains_key(header::ORIGIN)
        {
            return Box::pin(std::future::ready(text(
                StatusCode::FORBIDDEN,
                "No page can call this route: only the quadratique command line can.",
            )));
        }
        let origins = request.headers().get_all(header::ORIGIN);
        // A browser sends one `Origin` at most: more than one is refused on every route.
        let count = origins.iter().count();
        let own_origin = own_host.is_some_and(|host| {
            origins
                .iter()
                .all(|origin| origin.as_bytes() == format!("http://{host}").as_bytes())
        }) && count <= 1
            && (count == 1 || self.origin_header != OriginHeader::Required);
        if !own_origin {
            return Box::pin(std::future::ready(text(
                StatusCode::FORBIDDEN,
                "Only the Quadratique page served from this address can call this route.",
            )));
        }
        request.headers_mut().remove(header::ORIGIN);
        self.inner.handle(request)
    }
}

struct Files(WebApp);

impl Handler for Files {
    fn handle(&self, request: Request<RequestBody>) -> BoxFuture<'static, Response<ResponseBody>> {
        let path = request.uri().path();
        let response = if answers_only_on_the_socket(path) {
            not_found()
        } else if request.method() == Method::GET || request.method() == Method::HEAD {
            let head = request.method() == Method::HEAD;
            let mut response = self.answer(path);
            if head {
                *response.body_mut() = Full::new(Bytes::new()).boxed_unsync();
            }
            response
        } else if self.serves(path) {
            let mut response = text(StatusCode::METHOD_NOT_ALLOWED, "Only GET and HEAD.");
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
            response
        } else {
            not_found()
        };
        Box::pin(std::future::ready(response))
    }
}

/// The command line's routes and the server's control routes answer only on the socket. On TCP they are not found,
/// before anything else: the app's shell never stands in for them.
fn answers_only_on_the_socket(path: &str) -> bool {
    [crate::server::BASE, crate::server::IDENTITY]
        .iter()
        .any(|base| path == *base || path.starts_with(&format!("{base}/")))
}

fn not_found() -> Response<ResponseBody> {
    text(StatusCode::NOT_FOUND, "No such file.")
}

impl Files {
    /// Whether a `GET` of `path` would answer a file or the app's shell.
    fn serves(&self, path: &str) -> bool {
        relative(path).is_some_and(|relative| {
            (!relative.as_os_str().is_empty() && self.0.read(&relative).is_some())
                || (relative.extension().is_none() && is_deep_link(path))
        })
    }

    fn answer(&self, path: &str) -> Response<ResponseBody> {
        let app = &self.0;
        let Some(relative) = relative(path) else {
            return not_found();
        };
        if !relative.as_os_str().is_empty()
            && let Some(file) = app.read(&relative)
        {
            return file_response(file, relative.starts_with("assets"));
        }
        if relative.extension().is_some() || !is_deep_link(path) {
            return not_found();
        }
        if let Some(index) = app.read(Path::new(INDEX)) {
            return file_response(index, false);
        }
        match app {
            WebApp::Directory(directory) if !directory.join(INDEX).exists() => text(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!(
                    "The web app is not built at {}: run `moon run quadratique-web:build`, or set {APP_VARIABLE} to its folder.",
                    directory.display()
                ),
            ),
            WebApp::Directory(_) | WebApp::Embedded(_) => not_found(),
        }
    }
}

/// The paths of the app's pages (web/src/route.ts), which answer the app's shell. `/session` and the data targets are
/// never one of them, so no other route's path falls back to the shell.
fn is_deep_link(path: &str) -> bool {
    if path.contains("//") {
        return false;
    }
    let path = path
        .strip_suffix('/')
        .filter(|rest| !rest.is_empty())
        .unwrap_or(path);
    let parts: Vec<&str> = path.split('/').skip(1).collect();
    let page = |part: &str| matches!(part, "board" | "history");
    match parts.as_slice() {
        [""] | ["now"] | ["all"] => true,
        ["all", part] => page(part),
        ["projects", key] | ["issues", key] => !key.is_empty(),
        ["projects", key, part] => !key.is_empty() && page(part),
        _ => false,
    }
}

/// The bootstrap pages that `quadratique ui` opens hold a one-time code: the data folder must never be served, so the
/// built app and the data folder must not contain each other.
pub fn refuse_overlap(web_app: &WebApp, folder: &Path) -> Result<(), String> {
    let WebApp::Directory(directory) = web_app else {
        return Ok(());
    };
    let resolved = directory
        .canonicalize()
        .or_else(|_| std::path::absolute(directory))
        .unwrap_or_else(|_| directory.clone());
    if resolved.starts_with(folder) || folder.starts_with(&resolved) {
        return Err(format!(
            "the web app at {} and the data folder {} are inside each other; set {APP_VARIABLE} to a folder outside the data folder",
            resolved.display(),
            folder.display()
        ));
    }
    Ok(())
}

fn relative(path: &str) -> Option<PathBuf> {
    let relative = Path::new(path.trim_start_matches('/'));
    let safe = !path.contains(['\\', '\0'])
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    safe.then(|| relative.components().collect())
}

fn inside(directory: &Path, relative: &Path) -> Option<PathBuf> {
    let root = directory.canonicalize().ok()?;
    let file = root.join(relative).canonicalize().ok()?;
    (file.starts_with(&root) && file.is_file()).then_some(file)
}

fn file_response(file: AppFile, immutable: bool) -> Response<ResponseBody> {
    let mut response = Response::new(Full::new(file.bytes).boxed_unsync());
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(file.media_type),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if immutable {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        }),
    );
    secure(headers);
    response
}

fn secure(headers: &mut http::HeaderMap) {
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
}

fn text(status: StatusCode, message: &str) -> Response<ResponseBody> {
    let mut response = Response::new(Full::new(Bytes::from(message.to_owned())).boxed_unsync());
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    secure(headers);
    response
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{WebApp, is_deep_link, refuse_overlap};

    #[test]
    fn only_the_pages_of_the_app_are_deep_links() {
        for page in [
            "/",
            "/now",
            "/all",
            "/all/",
            "/all/board",
            "/all/history",
            "/projects/DEM",
            "/projects/DEM/board",
            "/projects/DEM/history/",
            "/issues/DEM-1",
        ] {
            assert!(is_deep_link(page), "{page}");
        }
        for other in [
            "/cli",
            "/cli/capabilities",
            "/server",
            "/server/stop",
            "/session",
            "/web",
            "/webmcp/x",
            "/projects",
            "/projects/DEM/other",
            "/issues/DEM-1/more",
            "/elsewhere",
            "//",
        ] {
            assert!(!is_deep_link(other), "{other}");
        }
    }

    #[test]
    fn the_web_app_and_the_data_folder_may_not_contain_each_other() {
        let folder = tempfile::tempdir().expect("a folder");
        let home = folder.path().canonicalize().expect("its path");
        let inside = home.join("dist");
        let outside = tempfile::tempdir().expect("another folder");

        assert!(refuse_overlap(&WebApp::Directory(inside), &home).is_err());
        assert!(refuse_overlap(&WebApp::Directory(home.clone()), &home.join("data")).is_err());
        assert!(refuse_overlap(&WebApp::Directory(outside.path().to_path_buf()), &home).is_ok());
        assert!(refuse_overlap(&WebApp::Embedded(&[]), Path::new("/")).is_ok());
    }
}
