//! AcidCat's `remote::serve` does not show the peer of a connection.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use acid_cat_core::remote::{RequestBody, ResponseBody, Routes, request_body};
use bytes::Bytes;
use futures::future::BoxFuture;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Empty};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub const OWNER_ONLY: &str = "This server answers only processes of its own account.";
/// A refused peer's request is read and discarded after the answer, so that its client reads the `403` instead of a
/// reset: up to `drain_bytes`, for up to `drain_for`, and for at most `draining` connections at once.
#[derive(Clone, Copy, Debug)]
pub struct Refusals {
    pub draining: usize,
    pub drain_bytes: u64,
    pub drain_for: Duration,
}

pub const REFUSALS: Refusals = Refusals {
    draining: 16,
    drain_bytes: 64 * 1024,
    drain_for: Duration::from_secs(1),
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Connection {
    pub local: SocketAddr,
    pub peer: SocketAddr,
}

pub type Admit = Arc<dyn Fn(Connection) -> bool + Send + Sync>;

pub struct Served {
    pub address: SocketAddr,
    listener: Arc<TcpListener>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Served {
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub async fn close(mut self) {
        self.stop.cancel();
        let _ = (&mut self.task).await;
    }
}

/// Dropping the server without `close` still stops it: the token ends the accept loop and every connection, and their
/// tasks then drop the routes, and with them the store and the home's lock.
///
/// The listener stops listening here, before any of that. Closing this process's descriptor is not enough: a child
/// process holds a copy of it from its `fork` to its `exec`, and the socket listens until the last copy closes, which
/// can come after the home's lock is free.
impl Drop for Served {
    fn drop(&mut self) {
        self.stop.cancel();
        stop_listening(&*self.listener);
    }
}

/// On Linux, `shutdown` takes a listening socket out of the listening state for every descriptor of it: a TCP socket
/// stops listening, and a Unix socket refuses every later `connect`.
#[cfg(target_os = "linux")]
fn stop_listening(listener: impl std::os::fd::AsFd) {
    let _ = rustix::net::shutdown(listener, rustix::net::Shutdown::Both);
}

#[cfg(not(target_os = "linux"))]
fn stop_listening<L>(_listener: L) {}

pub async fn serve(address: SocketAddr, routes: Routes, admit: Admit) -> std::io::Result<Served> {
    serve_with(address, routes, admit, REFUSALS).await
}

pub async fn serve_with(
    address: SocketAddr,
    routes: Routes,
    admit: Admit,
    refusals: Refusals,
) -> std::io::Result<Served> {
    let listener = Arc::new(TcpListener::bind(address).await?);
    let address = listener.local_addr()?;
    let mut routes = routes;
    routes.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
    let routes = Arc::new(routes);
    let draining = Arc::new(Semaphore::new(refusals.draining));
    let stop = CancellationToken::new();
    let stopped = stop.clone();
    let accepting = Arc::clone(&listener);
    let task = tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                biased;
                () = stopped.cancelled() => return,
                accepted = accepting.accept() => accepted,
            };
            let Ok((stream, peer)) = accepted else {
                continue;
            };
            let Ok(local) = stream.local_addr() else {
                continue;
            };
            let connection = Connection { local, peer };
            let routes = routes.clone();
            let admit = admit.clone();
            let draining = draining.clone();
            let stopped = stopped.clone();
            tokio::spawn(async move {
                tokio::select! {
                    () = handle(stream, connection, routes, admit, Drain { refusals, slots: draining }) => {}
                    () = stopped.cancelled() => {}
                }
            });
        }
    });
    Ok(Served {
        address,
        listener,
        stop,
        task,
    })
}

/// The server's Unix socket: file permissions decide who connects, so it admits every connection it accepts.
#[cfg(unix)]
pub struct ServedSocket {
    listener: Arc<tokio::net::UnixListener>,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    file: crate::home::SocketFile,
}

#[cfg(unix)]
impl ServedSocket {
    pub fn path(&self) -> &std::path::Path {
        self.file.path()
    }

    pub async fn close(mut self) {
        self.stop.cancel();
        let _ = (&mut self.task).await;
    }
}

/// As for [`Served`]: the socket stops taking connections for every copy of its descriptor, then its file goes.
#[cfg(unix)]
impl Drop for ServedSocket {
    fn drop(&mut self) {
        self.stop.cancel();
        stop_listening(&*self.listener);
    }
}

/// Accepts on a socket that `home::bind_socket` bound and checked.
#[cfg(unix)]
pub fn serve_socket(
    listener: tokio::net::UnixListener,
    file: crate::home::SocketFile,
    routes: Routes,
) -> ServedSocket {
    let listener = Arc::new(listener);
    let mut routes = routes;
    routes.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
    let routes = Arc::new(routes);
    let stop = CancellationToken::new();
    let stopped = stop.clone();
    let accepting = Arc::clone(&listener);
    let task = tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                biased;
                () = stopped.cancelled() => return,
                accepted = accepting.accept() => accepted,
            };
            let Ok((stream, _)) = accepted else {
                continue;
            };
            let routes = routes.clone();
            let stopped = stopped.clone();
            tokio::spawn(async move {
                tokio::select! {
                    () = answer(stream, routes) => {}
                    () = stopped.cancelled() => {}
                }
            });
        }
    });
    ServedSocket {
        listener,
        stop,
        task,
        file,
    }
}

async fn answer<S>(stream: S, routes: Arc<Routes>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
        let routes = routes.clone();
        async move { Ok::<_, Infallible>(route(&routes, request_body(request)).await) }
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
        .await;
}

async fn handle(
    stream: TcpStream,
    connection: Connection,
    routes: Arc<Routes>,
    admit: Admit,
    drain: Drain,
) {
    if !admit(connection) {
        refuse(stream, drain).await;
        return;
    }
    answer(stream, routes).await;
}

struct Drain {
    refusals: Refusals,
    slots: Arc<Semaphore>,
}

/// Without a free draining slot the connection closes right after the answer, unread, and its client may see a reset.
async fn refuse(mut stream: TcpStream, Drain { refusals, slots }: Drain) {
    let answer = format!(
        "HTTP/1.1 403 Forbidden\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n{OWNER_ONLY}",
        OWNER_ONLY.len()
    );
    let slot = slots.try_acquire_owned();
    if stream.write_all(answer.as_bytes()).await.is_err() {
        return;
    }
    let Ok(_slot) = slot else {
        return;
    };
    if stream.shutdown().await.is_err() {
        return;
    }
    let mut request = (&mut stream).take(refusals.drain_bytes);
    let mut discarded = tokio::io::sink();
    let drain = tokio::io::copy(&mut request, &mut discarded);
    let _ = tokio::time::timeout(refusals.drain_for, drain).await;
}

fn route(
    routes: &Routes,
    request: Request<RequestBody>,
) -> BoxFuture<'static, Response<ResponseBody>> {
    let path = request.uri().path().to_owned();
    let found = routes.iter().find(|(prefix, _)| {
        prefix == "/" || path == *prefix || path.starts_with(&format!("{prefix}/"))
    });
    match found {
        Some((_, handler)) => handler.handle(request),
        None => Box::pin(std::future::ready(not_found())),
    }
}

fn not_found() -> Response<ResponseBody> {
    let mut response = Response::new(
        Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed_unsync(),
    );
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}
