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

/// How long one connection to the Unix socket may last, from the first byte of its request to the end of its answer.
/// The socket serves only the control requests, which a client sends whole and with `Connection: close`; a client that
/// sends part of a request, or keeps the connection idle, is cut off here. QUA-53, which moves the commands and their
/// event streams to the socket, gives those routes their own bounds.
pub const SOCKET_REQUEST_LIMIT: Duration = Duration::from_secs(5);

/// The listeners of one server, as its stop request sees them: [`Gate::close`] takes every registered listener out of
/// the listening state at once, and [`Gate::request_stop`] tells the server to shut down.
#[derive(Clone, Default)]
pub struct Gate {
    inner: Arc<GateInner>,
}

#[derive(Default)]
struct GateInner {
    closers: Mutex<Vec<Closer>>,
    stop: CancellationToken,
}

type Closer = Box<dyn Fn() + Send + Sync>;

impl Gate {
    fn register(&self, closer: Closer) {
        self.inner
            .closers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(closer);
    }

    /// No registered listener accepts another connection once this returns; open connections go on.
    pub fn close(&self) {
        let closers = self
            .inner
            .closers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for close in closers.iter() {
            close();
        }
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
/// accepts its first connection, and a handler that sets [`StopAfterAnswer`] stops the server through it.
#[cfg(unix)]
pub fn serve_socket(
    listener: tokio::net::UnixListener,
    file: crate::home::SocketFile,
    routes: Routes,
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
            // Once a stop shuts the socket down, Linux reports it readable and hung up for good, and `accept4` answers
            // `EAGAIN`. tokio's `accept` retries inside one poll and spends no budget on `EAGAIN`, so it would hold its
            // worker thread forever and this loop would never see the cancellation that came before the shutdown.
            // `poll_accept` spends budget on each try and yields once the budget is spent.
            let accepted = tokio::select! {
                biased;
                () = accepted_while.cancelled() => return,
                accepted = std::future::poll_fn(|cx| listening.poll_accept(cx)) => accepted,
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
                let answering = answer(stream, routes, Some(when_answered.after.clone()));
                tokio::select! {
                    _ = tokio::time::timeout(SOCKET_REQUEST_LIMIT, answering) => {}
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

async fn answer<S>(stream: S, routes: Arc<Routes>, stop_after: Option<StopAfterAnswer>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
        let routes = routes.clone();
        let mut request = request_body(request);
        if let Some(stop_after) = &stop_after {
            request.extensions_mut().insert(stop_after.clone());
        }
        async move { Ok::<_, Infallible>(route(&routes, request).await) }
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
