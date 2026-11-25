//! The caller check of the local listener on macOS (docs/decisions/0003-the-caller-check-on-macos-and-windows.md).
//!
//! macOS has no call that maps a TCP connection to an account. The server lists the processes of its own account
//! (`proc_listpids` with `PROC_UID_ONLY`), their socket descriptors (`proc_pidinfo(PROC_PIDLISTFDS)`) and each
//! socket's state and endpoints (`proc_pidfdinfo(PROC_PIDFDSOCKETINFO)`), through the `libproc` crate. It admits a
//! connection when exactly one established socket of those processes is the connection seen from the other side.
//! A process of another account is not listed, and the kernel refuses to show its descriptors to any account but its
//! own and root, so its connection is refused.

use std::io;
use std::net::{SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use libproc::bsd_info::BSDInfo;
use libproc::file_info::{ListFDs, PIDFDInfo, PIDFDInfoFlavor, ProcFDInfo, ProcFDType, pidfdinfo};
use libproc::proc_pid::{listpidinfo, pidinfo};
use libproc::processes::{ProcFilter, pids_by_type};

use crate::serve::{Admit, Connection};
use crate::socket_fdinfo::{self, Found, TcpSocket};

/// The build stops when the `libproc` crate's declaration of `struct socket_fdinfo` places a field this check reads
/// elsewhere than `socket_fdinfo` does.
const _: () = {
    use std::mem::offset_of;

    use libproc::net_info::{InSockInfo, SocketFDInfo, SocketInfo, TcpSockInfo};

    let socket = offset_of!(SocketFDInfo, psi);
    let proto = socket + offset_of!(SocketInfo, soi_proto);
    let in_sockinfo = proto + offset_of!(TcpSockInfo, tcpsi_ini);
    assert!(socket + offset_of!(SocketInfo, soi_so) == socket_fdinfo::SOCKET_AT);
    assert!(socket + offset_of!(SocketInfo, soi_kind) == socket_fdinfo::KIND_AT);
    assert!(proto == socket_fdinfo::PROTO_AT);
    assert!(in_sockinfo + offset_of!(InSockInfo, insi_fport) == socket_fdinfo::REMOTE_PORT_AT);
    assert!(in_sockinfo + offset_of!(InSockInfo, insi_lport) == socket_fdinfo::LOCAL_PORT_AT);
    assert!(in_sockinfo + offset_of!(InSockInfo, insi_vflag) == socket_fdinfo::VERSION_AT);
    assert!(
        in_sockinfo + offset_of!(InSockInfo, insi_faddr) + 12 == socket_fdinfo::REMOTE_ADDRESS_AT
    );
    assert!(
        in_sockinfo + offset_of!(InSockInfo, insi_laddr) + 12 == socket_fdinfo::LOCAL_ADDRESS_AT
    );
    assert!(proto + offset_of!(TcpSockInfo, tcpsi_state) == socket_fdinfo::STATE_AT);
};

/// The kernel writes `struct socket_fdinfo` into these bytes. Any bytes are a valid value, so a structure that does
/// not match makes the lookup find nothing, never undefined behaviour.
struct SocketBytes([u8; socket_fdinfo::BYTES]);

impl Default for SocketBytes {
    fn default() -> SocketBytes {
        SocketBytes([0; socket_fdinfo::BYTES])
    }
}

impl PIDFDInfo for SocketBytes {
    fn flavor() -> PIDFDInfoFlavor {
        PIDFDInfoFlavor::SocketInfo
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PeerSocket {
    pub peer: SocketAddrV4,
    pub server: SocketAddrV4,
}

pub type Lookup = Arc<dyn Fn(PeerSocket) -> io::Result<Option<u32>> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum CallersUnknown {
    #[error(
        "macOS cannot tell this server who is connecting: listing the processes of this account failed ({0}). Quadratique does not serve without knowing who connects; start it from a normal session of your account, outside any sandbox"
    )]
    LookupFailed(io::Error),
    #[error(
        "macOS did not show this server the socket of a test connection it made to itself (proc_pidfdinfo), so it cannot tell who connects. Quadratique does not serve without knowing who connects. Start it from a normal session of your account, outside any sandbox; if it still refuses, report your macOS version at https://github.com/Hebilicious/quadratique/issues"
    )]
    NoSocket,
    #[error(
        "macOS names uid {found} for a test connection of this server, not uid {own}; Quadratique does not serve without knowing who connects"
    )]
    OtherUid { found: u32, own: u32 },
}

/// The effective uid: the one the kernel compares when it lists processes by account and shows their descriptors.
pub fn own_account() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// One loopback connection of this process, looked up as a caller's would be.
pub fn check_own_connection(own_uid: u32) -> Result<(), CallersUnknown> {
    check_own_connection_with(lookup_for(own_uid), own_uid)
}

fn check_own_connection_with(lookup: Lookup, own_uid: u32) -> Result<(), CallersUnknown> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(CallersUnknown::LookupFailed)?;
    let server = listener
        .local_addr()
        .map_err(CallersUnknown::LookupFailed)?;
    let test_connection =
        std::net::TcpStream::connect(server).map_err(CallersUnknown::LookupFailed)?;
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

pub fn owner_only(own_uid: u32) -> Admit {
    admit_with(
        lookup_for(own_uid),
        own_uid,
        Arc::new(|error: &io::Error| {
            eprintln!(
                "quadratique serve: macOS cannot list the processes of this account ({error}); this server refuses every connection until it can"
            );
        }),
    )
}

pub fn admit_with(
    lookup: Lookup,
    own_uid: u32,
    report_first_failure: Arc<dyn Fn(&io::Error) + Send + Sync>,
) -> Admit {
    let reported = AtomicBool::new(false);
    Arc::new(move |connection: Connection| {
        let (SocketAddr::V4(server), SocketAddr::V4(peer)) = (connection.local, connection.peer)
        else {
            return false;
        };
        match lookup(PeerSocket { peer, server }) {
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

fn lookup_for(own_uid: u32) -> Lookup {
    Arc::new(move |socket| uid_of_peer_socket(own_uid, socket))
}

/// The account of the process that holds the other end of the connection, among this account's processes. The
/// holder's account is read again after the scan: a process id that the scan found, then reused by another account's
/// process, is not taken for the owner's.
fn uid_of_peer_socket(own_uid: u32, socket: PeerSocket) -> io::Result<Option<u32>> {
    match holder_among(processes_of(own_uid)?, socket) {
        Found::Holder(pid) => Ok(account_of(pid)),
        Found::Nothing | Found::NotEstablished(_) | Found::Several => Ok(None),
    }
}

/// What a scan of `processes` finds for `socket`. A process that ends during the scan, or whose descriptors the
/// kernel does not show, holds no socket for the scan.
pub fn holder_among(processes: Vec<u32>, socket: PeerSocket) -> Found {
    let sockets = processes.into_iter().flat_map(|pid| {
        tcp_sockets_of(pid)
            .into_iter()
            .map(move |found| (pid, found))
    });
    socket_fdinfo::holder_of(socket.peer, socket.server, sockets)
}

/// The processes whose effective uid is `uid`.
pub fn processes_of(uid: u32) -> io::Result<Vec<u32>> {
    pids_by_type(ProcFilter::ByUID { uid })
}

/// Every process of the machine, for the test that shows another account's sockets are hidden from this one.
pub fn holder_among_every_process(socket: PeerSocket) -> io::Result<Found> {
    Ok(holder_among(pids_by_type(ProcFilter::All)?, socket))
}

fn account_of(pid: u32) -> Option<u32> {
    let pid = i32::try_from(pid).ok()?;
    pidinfo::<BSDInfo>(pid, 0).ok().map(|info| info.pbi_uid)
}

/// The TCP sockets over IPv4 that `pid`'s descriptors show; none when the kernel refuses to show them.
pub fn tcp_sockets_of(pid: u32) -> Vec<TcpSocket> {
    let Ok(pid) = i32::try_from(pid) else {
        return Vec::new();
    };
    let Ok(info) = pidinfo::<BSDInfo>(pid, 0) else {
        return Vec::new();
    };
    let Some(descriptors) = descriptors_of(pid, info.pbi_nfiles as usize) else {
        return Vec::new();
    };
    descriptors
        .into_iter()
        .filter(|descriptor| descriptor.proc_fdtype == ProcFDType::Socket as u32)
        .filter_map(|descriptor| pidfdinfo::<SocketBytes>(pid, descriptor.proc_fd).ok())
        .filter_map(|bytes| socket_fdinfo::tcp_over_ipv4(&bytes.0))
        .collect()
}

const MOST_DESCRIPTORS: usize = 1 << 20;

/// Every descriptor of `pid`. The kernel lists them in order and stops when the buffer is full, so a process that
/// opens descriptors after `pbi_nfiles` was read would push its highest ones, the caller's socket among them, out of
/// a buffer of exactly that size. A full answer is asked again with twice the room.
fn descriptors_of(pid: i32, files: usize) -> Option<Vec<ProcFDInfo>> {
    let mut room = files.saturating_mul(2).max(64);
    while room <= MOST_DESCRIPTORS {
        let descriptors = listpidinfo::<ListFDs>(pid, room).ok()?;
        if descriptors.len() < room {
            return Some(descriptors);
        }
        room *= 2;
    }
    None
}

/// `pbi_nfiles` of `pid`, the number of its open descriptors and its highest descriptor, for the measure: they show
/// what `pbi_nfiles` counts on this macOS.
pub fn descriptor_counts(pid: u32) -> Option<(u32, usize, i32)> {
    let pid = i32::try_from(pid).ok()?;
    let files = pidinfo::<BSDInfo>(pid, 0).ok()?.pbi_nfiles;
    let descriptors = descriptors_of(pid, files as usize)?;
    let highest = descriptors
        .iter()
        .map(|descriptor| descriptor.proc_fd)
        .max()?;
    Some((files, descriptors.len(), highest))
}

/// How long `count` lookups of `socket` take, for the measure of the check's cost.
pub fn time_lookups(own_uid: u32, socket: PeerSocket, count: usize) -> Vec<Duration> {
    (0..count)
        .map(|_| {
            let started = std::time::Instant::now();
            let _ = uid_of_peer_socket(own_uid, socket);
            started.elapsed()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{CallersUnknown, admit_with, own_account, owner_only};
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

    #[test]
    fn the_lookup_of_another_uid_does_not_find_this_accounts_connection() {
        let (_client, _server_side, connection) = accepted();
        let another = own_account() + 1;

        assert!(!owner_only(another)(connection));
    }
}
