//! The caller check of the local listener on macOS (docs/decisions/0003-the-caller-check-on-macos-and-windows.md).
//!
//! For each accepted connection the server reads the kernel's table of TCP connections (the sysctl
//! `net.inet.tcp.pcblist_n`, which any account may read) and finds the caller's socket in it: the accepted connection
//! seen from the other side. It admits the connection when that socket's own account (`so_uid`, from the socket's
//! credential) is the server's, as Linux does with `idiag_uid`.

use std::io;
use std::net::{SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use sysctl::{Ctl, Sysctl, SysctlError};

use crate::serve::{Admit, Connection};
use crate::tcp_table::{self, Found, Record};

const TABLE: &str = "net.inet.tcp.pcblist_n";
/// The kernel sizes its answer for the connections it has, plus an eighth; when more connections open between the
/// size and the copy, it answers `ENOMEM` and the table is read again, at most this many times.
const READS: usize = 4;
/// Lookups that may run at once. A connection that comes while this many are running is refused: it costs the server
/// nothing, and its client may connect again.
const IN_FLIGHT: usize = 16;

#[derive(Clone, Copy, Debug)]
pub struct PeerSocket {
    pub peer: SocketAddrV4,
    pub server: SocketAddrV4,
}

pub type Lookup = Arc<dyn Fn(PeerSocket) -> io::Result<Option<u32>> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum CallersUnknown {
    #[error(
        "macOS cannot tell this server who is connecting: reading its table of TCP connections ({TABLE}) failed ({0}). Quadratique does not serve without knowing who connects; start it from a normal session of your account, outside any sandbox, and if it still refuses, report your macOS version at https://github.com/Hebilicious/quadratique/issues"
    )]
    LookupFailed(io::Error),
    #[error(
        "macOS's table of TCP connections ({TABLE}) does not show a test connection that this server made to itself, so it cannot tell who connects. Quadratique does not serve without knowing who connects; report your macOS version at https://github.com/Hebilicious/quadratique/issues"
    )]
    NoSocket,
    #[error(
        "macOS names uid {found} for a test connection of this server, not uid {own}; Quadratique does not serve without knowing who connects"
    )]
    OtherUid { found: u32, own: u32 },
}

/// The effective uid: the account whose credential the server's own sockets carry.
pub fn own_account() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// The table, read once.
pub fn table() -> io::Result<Vec<Record>> {
    let control = Ctl::new(TABLE).map_err(as_io)?;
    read_table(&control)
}

fn read_table(control: &Ctl) -> io::Result<Vec<Record>> {
    let mut reads = 0;
    let bytes = loop {
        reads += 1;
        match control.value_as_bytes() {
            Err(error) if reads < READS && is_out_of_room(&error) => continue,
            answer => break answer.map_err(as_io)?,
        }
    };
    tcp_table::records(&bytes)
        .map_err(|malformed| io::Error::new(io::ErrorKind::InvalidData, malformed))
}

trait AsBytes {
    fn value_as_bytes(&self) -> Result<Vec<u8>, SysctlError>;
}

impl AsBytes for Ctl {
    fn value_as_bytes(&self) -> Result<Vec<u8>, SysctlError> {
        match self.value()? {
            sysctl::CtlValue::Struct(bytes) | sysctl::CtlValue::Node(bytes) => Ok(bytes),
            _ => Err(SysctlError::ExtractionError),
        }
    }
}

fn is_out_of_room(error: &SysctlError) -> bool {
    matches!(error, SysctlError::IoError(error) if error.raw_os_error() == Some(rustix::io::Errno::NOMEM.raw_os_error()))
}

fn as_io(error: SysctlError) -> io::Error {
    match error {
        SysctlError::IoError(error) => error,
        other => io::Error::other(other.to_string()),
    }
}

/// What the table says of `socket`, for the tests and the start-up check.
pub fn find(socket: PeerSocket) -> io::Result<Found> {
    Ok(tcp_table::caller(socket.peer, socket.server, &table()?))
}

/// The account of the caller's socket, when the table shows exactly one.
fn uid_of_peer_socket(control: &Ctl, socket: PeerSocket) -> io::Result<Option<u32>> {
    match tcp_table::caller(socket.peer, socket.server, &read_table(control)?) {
        Found::Caller { uid } => Ok(Some(uid)),
        Found::Nothing | Found::NotEstablished(_) | Found::Several | Found::AcceptedSideGone => {
            Ok(None)
        }
    }
}

fn lookup() -> io::Result<Lookup> {
    let control = Ctl::new(TABLE).map_err(as_io)?;
    Ok(Arc::new(move |socket| uid_of_peer_socket(&control, socket)))
}

/// One loopback connection of this process, looked up as a caller's would be.
pub fn check_own_connection(own_uid: u32) -> Result<(), CallersUnknown> {
    check_own_connection_with(lookup().map_err(CallersUnknown::LookupFailed)?, own_uid)
}

fn check_own_connection_with(lookup: Lookup, own_uid: u32) -> Result<(), CallersUnknown> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(CallersUnknown::LookupFailed)?;
    let server = listener
        .local_addr()
        .map_err(CallersUnknown::LookupFailed)?;
    let test_connection =
        std::net::TcpStream::connect(server).map_err(CallersUnknown::LookupFailed)?;
    let _accepted = listener.accept().map_err(CallersUnknown::LookupFailed)?;
    let peer = test_connection
        .local_addr()
        .map_err(CallersUnknown::LookupFailed)?;
    let (SocketAddr::V4(server), SocketAddr::V4(peer)) = (server, peer) else {
        return Err(CallersUnknown::NoSocket);
    };
    match lookup(PeerSocket { peer, server }) {
        Ok(Some(uid)) if uid == own_uid => Ok(()),
        Ok(Some(found)) => Err(CallersUnknown::OtherUid {
            found,
            own: own_uid,
        }),
        Ok(None) => Err(CallersUnknown::NoSocket),
        Err(error) => Err(CallersUnknown::LookupFailed(error)),
    }
}

/// The admission of the server. `check_own_connection` has read the table once before, so the sysctl exists; if it
/// cannot be named now, every connection is refused.
pub fn owner_only(own_uid: u32) -> Admit {
    let report = Arc::new(|error: &io::Error| {
        eprintln!(
            "quadratique serve: the account of a caller cannot be read from macOS's table of TCP connections ({error}); this server refuses every connection until it can"
        );
    });
    match lookup() {
        Ok(lookup) => admit_with(lookup, own_uid, report),
        Err(error) => {
            report(&error);
            let refuse_all: Admit = Arc::new(|_| false);
            refuse_all
        }
    }
}

pub fn admit_with(
    lookup: Lookup,
    own_uid: u32,
    report_first_failure: Arc<dyn Fn(&io::Error) + Send + Sync>,
) -> Admit {
    let reported = AtomicBool::new(false);
    let running = Arc::new(AtomicUsize::new(0));
    Arc::new(move |connection: Connection| {
        let (SocketAddr::V4(server), SocketAddr::V4(peer)) = (connection.local, connection.peer)
        else {
            return false;
        };
        let Some(_slot) = Slot::take(&running) else {
            return false;
        };
        match off_the_runtime(|| lookup(PeerSocket { peer, server })) {
            Ok(uid) => uid == Some(own_uid),
            Err(error) => {
                if !reported.swap(true, Ordering::Relaxed) {
                    report_first_failure(&error);
                }
                false
            }
        }
    })
}

/// One of the `IN_FLIGHT` lookups that may run at once, given back when dropped.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(running: &Arc<AtomicUsize>) -> Option<Slot> {
        running
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < IN_FLIGHT).then_some(count + 1)
            })
            .ok()
            .map(|_| Slot(Arc::clone(running)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// On a multi-threaded runtime, the worker that runs the lookup hands its other tasks to the other workers first, so
/// a slow lookup does not stall them. A single-threaded runtime (some tests) has no other worker to hand them to.
fn off_the_runtime<T>(work: impl FnOnce() -> T) -> T {
    let multi_threaded = tokio::runtime::Handle::try_current().is_ok_and(|runtime| {
        runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
    });
    if multi_threaded {
        tokio::task::block_in_place(work)
    } else {
        work()
    }
}

/// How long `count` reads of the table take and how many connections it holds, for the measure of the check's cost.
pub fn time_lookups(socket: PeerSocket, count: usize) -> io::Result<(Vec<Duration>, usize)> {
    let control = Ctl::new(TABLE).map_err(as_io)?;
    let size = read_table(&control)?.len();
    let times = (0..count)
        .map(|_| {
            let started = Instant::now();
            let _ = uid_of_peer_socket(&control, socket);
            started.elapsed()
        })
        .collect();
    Ok((times, size))
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    use super::{CallersUnknown, IN_FLIGHT, admit_with, own_account, owner_only};
    use crate::serve::Connection;

    fn accepted() -> (TcpStream, TcpStream, Connection) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let client =
            TcpStream::connect(listener.local_addr().expect("an address")).expect("a connection");
        let (server_side, peer) = listener.accept().expect("the connection is accepted");
        let local = server_side.local_addr().expect("the server's address");
        (client, server_side, Connection { local, peer })
    }

    #[test]
    fn a_connection_from_this_account_is_admitted() {
        let (_client, _server_side, connection) = accepted();

        assert!(owner_only(own_account())(connection));
    }

    #[test]
    fn a_connection_closed_before_the_lookup_is_refused() {
        let (client, _server_side, connection) = accepted();
        drop(client);

        assert!(!owner_only(own_account())(connection));
    }

    #[test]
    fn a_connection_whose_accepted_side_is_gone_is_refused() {
        let (_client, server_side, connection) = accepted();
        drop(server_side);

        assert!(!owner_only(own_account())(connection));
    }

    #[test]
    fn a_peer_unknown_to_the_kernel_or_naming_a_listener_is_refused() {
        let (_client, _server_side, connection) = accepted();
        let gone = Connection {
            peer: SocketAddr::from(([127, 0, 0, 1], 1)),
            ..connection
        };
        let listener_of_this_account = TcpListener::bind("127.0.0.1:0").expect("another port");
        let no_connection = Connection {
            peer: listener_of_this_account
                .local_addr()
                .expect("the listener's address"),
            ..connection
        };

        assert!(!owner_only(own_account())(gone));
        assert!(!owner_only(own_account())(no_connection));
    }

    #[test]
    fn a_connection_of_this_account_is_refused_by_a_server_of_another() {
        let (_client, _server_side, connection) = accepted();

        assert!(!owner_only(own_account() + 1)(connection));
    }

    #[test]
    fn a_start_whose_own_connection_macos_cannot_name_fails_with_the_cause() {
        let own_uid = own_account();
        let check = |lookup: super::Lookup| super::check_own_connection_with(lookup, own_uid);

        assert!(super::check_own_connection(own_uid).is_ok());
        assert!(matches!(
            check(Arc::new(|_| Ok(None))),
            Err(CallersUnknown::NoSocket)
        ));
        assert!(matches!(
            check(Arc::new(|_| Err(io::Error::from(
                io::ErrorKind::PermissionDenied
            )))),
            Err(CallersUnknown::LookupFailed(_))
        ));
        assert!(matches!(
            check(Arc::new(move |_| Ok(Some(own_uid + 1)))),
            Err(CallersUnknown::OtherUid { .. })
        ));
    }

    #[test]
    fn a_lookup_that_cannot_be_made_refuses_and_is_reported_once() {
        let (_client, _server_side, connection) = accepted();
        let reports = Arc::new(AtomicUsize::new(0));
        let heard = reports.clone();
        let admit = admit_with(
            Arc::new(|_| Err(io::Error::from(io::ErrorKind::PermissionDenied))),
            own_account(),
            Arc::new(move |_: &io::Error| {
                heard.fetch_add(1, Ordering::Relaxed);
            }),
        );

        assert!(!admit(connection));
        assert!(!admit(connection));
        assert_eq!(reports.load(Ordering::Relaxed), 1);
    }

    /// The first `IN_FLIGHT` lookups wait on a barrier until the test has seen one more connection refused without a
    /// lookup.
    #[test]
    fn a_connection_that_comes_while_every_lookup_slot_is_taken_is_refused() {
        let (_client, _server_side, connection) = accepted();
        let own_uid = own_account();
        let started = Arc::new(Barrier::new(IN_FLIGHT + 1));
        let release = Arc::new(Barrier::new(IN_FLIGHT + 1));
        let looked_up = Arc::new(AtomicUsize::new(0));
        let lookup: super::Lookup = {
            let (started, release, looked_up) =
                (started.clone(), release.clone(), looked_up.clone());
            Arc::new(move |_| {
                if looked_up.fetch_add(1, Ordering::SeqCst) < IN_FLIGHT {
                    started.wait();
                    release.wait();
                }
                Ok(Some(own_uid))
            })
        };
        let admit = admit_with(lookup, own_uid, Arc::new(|_: &io::Error| {}));
        let waiting: Vec<_> = (0..IN_FLIGHT)
            .map(|_| {
                let admit = admit.clone();
                std::thread::spawn(move || admit(connection))
            })
            .collect();
        started.wait();

        let overloaded = admit(connection);
        release.wait();
        let admitted: Vec<bool> = waiting
            .into_iter()
            .map(|thread| thread.join().expect("a lookup thread"))
            .collect();

        assert!(!overloaded);
        assert_eq!(looked_up.load(Ordering::SeqCst), IN_FLIGHT);
        assert!(admitted.into_iter().all(|admitted| admitted));
        assert!(admit(connection), "a slot is given back after its lookup");
    }
}
