use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

use acid_cat_core::Application;
use acid_cat_core::http::{HttpBinding, HttpOptions};
use acid_cat_core::remote::{Handler, RequestBody, ResponseBody, Routes};
use bytes::Bytes;
use futures::future::BoxFuture;
use http::{HeaderValue, Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full};

use crate::app::Context;
use crate::assets::media_type;
use crate::domain::{Actor, Binding, Caller, WorkingCopy};
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

    fn hosts(&self) -> Vec<String> {
        self.0
            .get()
            .map(|address| {
                let port = address.port();
                vec![address.to_string(), format!("localhost:{port}")]
            })
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

pub fn routes(app: &Arc<Application<Context>>, store: &Store, web_app: WebApp) -> Routes {
    let target = |target: &str, base: &str, binding: Binding| {
        let context = Arc::new(Context::new(store.clone(), caller(binding)));
        let options = HttpOptions {
            target: target.into(),
            base: base.into(),
            ..HttpOptions::default()
        };
        Arc::new(HttpBinding::new(app.clone(), context, options)) as Arc<dyn Handler>
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
/// refused request gets `403` with no CORS header, so a browser's preflight from another origin fails too.
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
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok());
        let own_host = host.filter(|host| hosts.iter().any(|own| own == host));
        if self.origin_header == OriginHeader::Refused
            && request.headers().contains_key(header::ORIGIN)
        {
            return Box::pin(std::future::ready(text(
                StatusCode::FORBIDDEN,
                "No page can call this route: only the quadratique command line can.",
            )));
        }
        let own_origin = own_host.is_some_and(|host| {
            request
                .headers()
                .get_all(header::ORIGIN)
                .iter()
                .all(|origin| origin.as_bytes() == format!("http://{host}").as_bytes())
        });
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
        let response = if request.method() == Method::GET || request.method() == Method::HEAD {
            let head = request.method() == Method::HEAD;
            let mut response = self.answer(request.uri().path());
            if head {
                *response.body_mut() = Full::new(Bytes::new()).boxed_unsync();
            }
            response
        } else {
            let mut response = text(StatusCode::METHOD_NOT_ALLOWED, "Only GET and HEAD.");
            response
                .headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
            response
        };
        Box::pin(std::future::ready(response))
    }
}

impl Files {
    fn answer(&self, path: &str) -> Response<ResponseBody> {
        let app = &self.0;
        let Some(relative) = relative(path) else {
            return text(StatusCode::NOT_FOUND, "No such file.");
        };
        if !relative.as_os_str().is_empty()
            && let Some(file) = app.read(&relative)
        {
            return file_response(file, relative.starts_with("assets"));
        }
        if relative.extension().is_some() {
            return text(StatusCode::NOT_FOUND, "No such file.");
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
            WebApp::Directory(_) | WebApp::Embedded(_) => {
                text(StatusCode::NOT_FOUND, "No such file.")
            }
        }
    }
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
