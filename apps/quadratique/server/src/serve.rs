//! AcidCat's `remote::serve` does not show the peer of a connection.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acid_cat_core::remote::{RequestBody, ResponseBody, Routes, request_body};
use bytes::Bytes;
use futures::future::BoxFuture;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Empty};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
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

/// How long a client of the Unix socket has to send a whole request, head and body, from the moment the connection
/// opens or its last answer ends: a client that sends part of a request, or keeps the connection idle, is cut off. An
/// answer has no limit once its request has arrived whole, because a call may wait and an event stream lasts. TCP has
/// no such limit.
pub const SOCKET_REQUEST_LIMIT: Duration = Duration::from_secs(5);

/// The listeners of one server, as its stop request sees them: [`Gate::close`] takes every registered listener out of
/// the listening state at once, and [`Gate::request_stop`] tells the server to shut down.
#[derive(Clone, Default)]
pub struct Gate {
    inner: Arc<GateInner>,
}

#[derive(Default)]
struct GateInner {
    closers: Mutex<Closers>,
    stop: CancellationToken,
}

#[derive(Default)]
struct Closers {
    registered: Vec<Closer>,
    closed: bool,
}

type Closer = Box<dyn Fn() + Send + Sync>;

impl Gate {
    /// A listener registered after [`Gate::close`] is closed at once: the web app's listener may open while a stop is
    /// being answered.
    fn register(&self, closer: Closer) {
        let mut closers = self.closers();
        if closers.closed {
            closer();
        }
        closers.registered.push(closer);
    }

    /// No registered listener accepts another connection once this returns; open connections go on.
    pub fn close(&self) {
        let mut closers = self.closers();
        closers.closed = true;
        for close in closers.registered.iter() {
            close();
        }
    }

    fn closers(&self) -> std::sync::MutexGuard<'_, Closers> {
        self.inner
            .closers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn request_stop(&self) {
        self.inner.stop.cancel();
    }

    pub async fn stop_requested(&self) {
        self.inner.stop.cancelled().await;
    }
}

/// Set by a handler whose answer must reach its client before the server shuts down: the connection asks the
/// [`Gate`] to stop once it has written and flushed the answer and closed.
#[derive(Clone, Default)]
pub struct StopAfterAnswer {
    set: Arc<AtomicBool>,
}

impl StopAfterAnswer {
    pub fn set(&self) {
        self.set.store(true, Ordering::SeqCst);
    }
}

/// Where a connection to the Unix socket is, for its request limit.
#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
enum Phase {
    /// A request is on its way: its head and its body must have arrived by `by`.
    Receiving { by: tokio::time::Instant },
    /// The request has arrived whole, and its answer is being made or written: an answer may wait or stream, so it has
    /// no limit.
    Answering,
}

/// The request limit of one connection to the Unix socket. The connection starts receiving; a request whose body has
/// arrived whole moves it to answering; the end of that answer moves it back to receiving, with a new deadline for
/// the next request. A connection still receiving at its deadline is cut off.
#[cfg(unix)]
#[derive(Clone)]
struct RequestLimit {
    phase: Arc<tokio::sync::watch::Sender<Phase>>,
    limit: Duration,
}

#[cfg(unix)]
impl RequestLimit {
    fn starting(limit: Duration) -> RequestLimit {
        RequestLimit {
            phase: Arc::new(tokio::sync::watch::Sender::new(Phase::Receiving {
                by: tokio::time::Instant::now() + limit,
            })),
            limit,
        }
    }

    fn received(&self) {
        self.phase.send_replace(Phase::Answering);
    }

    fn answered(&self) {
        self.phase.send_replace(Phase::Receiving {
            by: tokio::time::Instant::now() + self.limit,
        });
    }

    /// Resolves when the connection is still receiving at its deadline.
    async fn expired(&self) {
        let mut phases = self.phase.subscribe();
        loop {
            let phase = *phases.borrow_and_update();
            match phase {
                Phase::Receiving { by } => tokio::select! {
                    () = tokio::time::sleep_until(by) => return,
                    _ = phases.changed() => {}
                },
                Phase::Answering => {
                    let _ = phases.changed().await;
                }
            }
        }
    }
}

/// Runs `at_end` once, when the body has given its last frame or is dropped, whichever comes first. It keeps the body's
/// size, so that an answer of known length is still sent with its `Content-Length`.
#[cfg(unix)]
fn on_end<E: Send + 'static>(
    body: http_body_util::combinators::UnsyncBoxBody<Bytes, E>,
    at_end: impl FnOnce() + Send + Unpin + 'static,
) -> http_body_util::combinators::UnsyncBoxBody<Bytes, E> {
    OnEnd {
        body,
        at_end: Some(at_end),
    }
    .boxed_unsync()
}

#[cfg(unix)]
struct OnEnd<B, F: FnOnce()> {
    body: B,
    at_end: Option<F>,
}

#[cfg(unix)]
impl<B, F: FnOnce()> OnEnd<B, F> {
    fn fire(&mut self) {
        if let Some(at_end) = self.at_end.take() {
            at_end();
        }
    }
}

#[cfg(unix)]
impl<B, F: FnOnce()> Drop for OnEnd<B, F> {
    fn drop(&mut self) {
        self.fire();
    }
}

#[cfg(unix)]
impl<B, F> hyper::body::Body for OnEnd<B, F>
where
    B: hyper::body::Body + Unpin,
    F: FnOnce() + Unpin,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<B::Data>, B::Error>>> {
        let this = self.get_mut();
        let polled = std::pin::Pin::new(&mut this.body).poll_frame(context);
        if matches!(polled, std::task::Poll::Ready(None)) || this.body.is_end_stream() {
            this.fire();
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.body.size_hint()
    }
}

/// What a connection to the Unix socket gives each of its requests and their answers.
#[cfg(unix)]
#[derive(Clone)]
struct OnSocket {
    stop_after: StopAfterAnswer,
    limit: RequestLimit,
}

#[cfg(unix)]
impl OnSocket {
    fn receive(&self, mut request: Request<RequestBody>) -> Request<RequestBody> {
        use hyper::body::Body;

        request.extensions_mut().insert(self.stop_after.clone());
        if request.body().is_end_stream() {
            self.limit.received();
            return request;
        }
        let limit = self.limit.clone();
        request.map(|body| on_end(body, move || limit.received()))
    }

    fn answer(&self, response: Response<ResponseBody>) -> Response<ResponseBody> {
        let limit = self.limit.clone();
        response.map(|body| on_end(body, move || limit.answered()))
    }
}

/// Fires on drop, so that a connection whose task ends in any way still asks for the stop it promised.
#[cfg(unix)]
struct StopWhenAnswered {
    after: StopAfterAnswer,
    gate: Gate,
}

#[cfg(unix)]
impl Drop for StopWhenAnswered {
    fn drop(&mut self) {
        if self.after.set.load(Ordering::SeqCst) {
            self.gate.request_stop();
        }
    }
}

/// The accept loop and the open connections of one listener. `accepting` ends the loop only; `stop` ends the loop and
/// every connection. Each connection's task holds a sender of `open`, so the receiver ends when the last one ends.
struct Tasks {
    accepting: CancellationToken,
    stop: CancellationToken,
    open: mpsc::Receiver<()>,
    task: tokio::task::JoinHandle<()>,
}

impl Tasks {
    /// Stops accepting, then waits up to `within` for the open connections to end by themselves.
    async fn drain(&mut self, within: Duration) {
        self.accepting.cancel();
        let _ = tokio::time::timeout(within, self.open.recv()).await;
    }

    async fn close(&mut self) {
        self.stop.cancel();
        let _ = (&mut self.task).await;
    }
}

pub struct Served {
    pub address: SocketAddr,
    listener: Arc<TcpListener>,
    tasks: Tasks,
}

impl Served {
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Lets a stop request take this listener out of the listening state.
    pub fn close_with(&self, gate: &Gate) {
        gate.register(closer(
            Arc::clone(&self.listener),
            self.tasks.accepting.clone(),
            self.address.to_string(),
        ));
    }

    pub async fn drain(&mut self, within: Duration) {
        self.tasks.drain(within).await;
    }

    pub async fn close(mut self) {
        self.tasks.close().await;
    }

    /// Accepts no other connection, for every copy of the listener's descriptor; open connections go on.
    pub fn stop_listening(&self) {
        closer(
            Arc::clone(&self.listener),
            self.tasks.accepting.clone(),
            self.address.to_string(),
        )();
    }
}

/// What `stop_listening` can act on: a descriptor on Linux, anything elsewhere.
#[cfg(target_os = "linux")]
trait Listening: std::os::fd::AsFd + Send + Sync + 'static {}
#[cfg(target_os = "linux")]
impl<L: std::os::fd::AsFd + Send + Sync + 'static> Listening for L {}
#[cfg(not(target_os = "linux"))]
trait Listening: Send + Sync + 'static {}
#[cfg(not(target_os = "linux"))]
impl<L: Send + Sync + 'static> Listening for L {}

fn closer<L: Listening>(listener: Arc<L>, accepting: CancellationToken, name: String) -> Closer {
    Box::new(move || {
        accepting.cancel();
        if let Err(error) = stop_listening(&*listener) {
            eprintln!("quadratique server: could not stop listening on {name}: {error}");
        }
    })
}

/// Dropping the server without `close` still stops it: the token ends the accept loop and every connection, and their
/// tasks then drop the routes, and with them the store and the home's lock.
///
/// The listener stops listening here, before any of that. Closing this process's descriptor is not enough: a child
/// process holds a copy of it from its `fork` to its `exec`, and the socket listens until the last copy closes, which
/// can come after the home's lock is free.
impl Drop for Served {
    fn drop(&mut self) {
        self.tasks.stop.cancel();
        if let Err(error) = stop_listening(&*self.listener) {
            eprintln!(
                "quadratique server: could not stop listening on {}: {error}; a child process may keep the port until it starts its program",
                self.address
            );
        }
    }
}

/// On Linux, `shutdown` takes a listening socket out of the listening state for every descriptor of it: a TCP socket
/// stops listening, and a Unix socket refuses every later `connect`. A sandbox or a policy may refuse the call: the
/// error then goes to the server's log.
/// `ENOTCONN` means the socket no longer listens: a stop request took it out of the listening state before the drop.
#[cfg(target_os = "linux")]
fn stop_listening(listener: impl std::os::fd::AsFd) -> std::io::Result<()> {
    match rustix::net::shutdown(listener, rustix::net::Shutdown::Both) {
        Ok(()) | Err(rustix::io::Errno::NOTCONN) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(target_os = "linux"))]
fn stop_listening<L>(_listener: L) -> std::io::Result<()> {
    Ok(())
}

pub async fn serve(address: SocketAddr, routes: Routes, admit: Admit) -> std::io::Result<Served> {
    serve_with(address, routes, admit, REFUSALS).await
}

pub async fn serve_with(
    address: SocketAddr,
    routes: Routes,
    admit: Admit,
    refusals: Refusals,
) -> std::io::Result<Served> {
    serve_on(TcpListener::bind(address).await?, routes, admit, refusals)
}

/// Serves on a listener bound beforehand. From here the listener belongs to the returned `Served`, which stops it
/// listening when it is dropped.
pub fn serve_on(
    listener: TcpListener,
    routes: Routes,
    admit: Admit,
    refusals: Refusals,
) -> std::io::Result<Served> {
    let listener = Arc::new(listener);
    let address = listener.local_addr()?;
    let mut routes = routes;
    routes.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
    let routes = Arc::new(routes);
    let draining = Arc::new(Semaphore::new(refusals.draining));
    let stop = CancellationToken::new();
    let accepting = stop.child_token();
    let (opened, open) = mpsc::channel(1);
    let stopped = stop.clone();
    let accepted_while = accepting.clone();
    let listening = Arc::clone(&listener);
    let task = tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                biased;
                () = accepted_while.cancelled() => return,
                accepted = listening.accept() => accepted,
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
            let open = opened.clone();
            tokio::spawn(async move {
                tokio::select! {
                    () = handle(stream, connection, routes, admit, Drain { refusals, slots: draining }) => {}
                    () = stopped.cancelled() => {}
                }
                drop(open);
            });
        }
    });
    Ok(Served {
        address,
        listener,
        tasks: Tasks {
            accepting,
            stop,
            open,
            task,
        },
    })
}

/// The server's Unix socket: file permissions decide who connects, so it admits every connection it accepts.
#[cfg(unix)]
pub struct ServedSocket {
    listener: Arc<tokio::net::UnixListener>,
    tasks: Tasks,
    file: crate::home::SocketFile,
}

#[cfg(unix)]
impl ServedSocket {
    pub fn path(&self) -> &std::path::Path {
        self.file.path()
    }

    pub async fn drain(&mut self, within: Duration) {
        self.tasks.drain(within).await;
    }

    pub async fn close(mut self) {
        self.tasks.close().await;
    }
}

/// As for [`Served`]: the socket stops taking connections for every copy of its descriptor, then its file goes.
#[cfg(unix)]
impl Drop for ServedSocket {
    fn drop(&mut self) {
        self.tasks.stop.cancel();
        if let Err(error) = stop_listening(&*self.listener) {
            eprintln!(
                "quadratique server: could not stop listening on {}: {error}; a child process may connect to it until it starts its program",
                self.file.path().display()
            );
        }
    }
}

/// Accepts on a socket that `home::bind_socket` bound and checked. The socket is registered with `gate` before it
/// accepts its first connection, and a handler that sets [`StopAfterAnswer`] stops the server through it. Each request
/// must arrive whole within `limit` (the product's is [`SOCKET_REQUEST_LIMIT`]); an answer has no limit.
#[cfg(unix)]
pub fn serve_socket(
    listener: tokio::net::UnixListener,
    file: crate::home::SocketFile,
    routes: Routes,
    limit: Duration,
    gate: &Gate,
) -> ServedSocket {
    let listener = Arc::new(listener);
    let mut routes = routes;
    routes.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
    let routes = Arc::new(routes);
    let stop = CancellationToken::new();
    let accepting = stop.child_token();
    gate.register(closer(
        Arc::clone(&listener),
        accepting.clone(),
        file.path().display().to_string(),
    ));
    let (opened, open) = mpsc::channel(1);
    let stopped = stop.clone();
    let accepted_while = accepting.clone();
    let listening = Arc::clone(&listener);
    let gate = gate.clone();
    let task = tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                biased;
                () = accepted_while.cancelled() => return,
                accepted = listening.accept() => accepted,
            };
            let Ok((stream, _)) = accepted else {
                continue;
            };
            let routes = routes.clone();
            let stopped = stopped.clone();
            let open = opened.clone();
            let when_answered = StopWhenAnswered {
                after: StopAfterAnswer::default(),
                gate: gate.clone(),
            };
            tokio::spawn(async move {
                let on_socket = OnSocket {
                    stop_after: when_answered.after.clone(),
                    limit: RequestLimit::starting(limit),
                };
                let limit = on_socket.limit.clone();
                tokio::select! {
                    () = answer(stream, routes, Some(on_socket)) => {}
                    () = limit.expired() => {}
                    () = stopped.cancelled() => {}
                }
                drop(when_answered);
                drop(open);
            });
        }
    });
    ServedSocket {
        listener,
        tasks: Tasks {
            accepting,
            stop,
            open,
            task,
        },
        file,
    }
}

#[cfg(not(unix))]
#[derive(Clone)]
enum OnSocket {}

#[cfg(not(unix))]
impl OnSocket {
    fn receive(&self, _request: Request<RequestBody>) -> Request<RequestBody> {
        match *self {}
    }

    fn answer(&self, _response: Response<ResponseBody>) -> Response<ResponseBody> {
        match *self {}
    }
}

async fn answer<S>(stream: S, routes: Arc<Routes>, on_socket: Option<OnSocket>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
        let routes = routes.clone();
        let on_socket = on_socket.clone();
        let request = request_body(request);
        let request = match &on_socket {
            Some(on_socket) => on_socket.receive(request),
            None => request,
        };
        async move {
            let response = route(&routes, request).await;
            Ok::<_, Infallible>(match &on_socket {
                Some(on_socket) => on_socket.answer(response),
                None => response,
            })
        }
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
    answer(stream, routes, None).await;
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
