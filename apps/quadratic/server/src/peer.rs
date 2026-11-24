use std::io;
use std::net::{SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rustix::net::netlink::{self, SocketAddrNetlink};
use rustix::net::{AddressFamily, RecvFlags, SendFlags, SocketFlags, SocketType};

use crate::serve::{Admit, Connection};

const SOCK_DIAG_BY_FAMILY: u16 = 20;
const NLMSG_ERROR: u16 = 2;
const NLM_F_REQUEST: u16 = 1;
const AF_INET: u8 = 2;
const IPPROTO_TCP: u8 = 6;
const ALL_STATES: u32 = u32::MAX;
const NO_COOKIE: u32 = u32::MAX;
const TCP_LISTEN: u8 = 10;
const HEADER_BYTES: usize = 16;
const STATE_AT: usize = HEADER_BYTES + 1;
/// `inet_diag_sockid`, after the message's family, state, timer and retransmits: source port, destination port, then
/// 16 bytes for each address, of which an IPv4 address uses the first 4.
const SOCKID_AT: usize = HEADER_BYTES + 4;
/// `inet_diag_msg`: family, state, timer and retransmits (4 bytes), `inet_diag_sockid` (48), expires, rqueue and
/// wqueue (12), then the uid.
const UID_AT: usize = HEADER_BYTES + 4 + 48 + 12;

#[derive(Clone, Copy, Debug)]
pub struct PeerSocket {
    pub peer: SocketAddrV4,
    pub server: SocketAddrV4,
}

pub type Lookup = Arc<dyn Fn(PeerSocket) -> io::Result<Option<u32>> + Send + Sync>;

const OVERFLOW_UID: &str = "/proc/sys/kernel/overflowuid";
const DEFAULT_OVERFLOW_UID: u32 = 65534;

#[derive(Debug, thiserror::Error)]
pub enum CallersUnknown {
    #[error(
        "this server's account is uid {uid}, the kernel's overflow uid: it runs in a user namespace where its account is not mapped, and the kernel would show every unmapped caller, root included, as that same uid. Start it outside that user namespace"
    )]
    UnmappedAccount { uid: u32 },
    #[error(
        "the kernel cannot tell who is connecting: looking up a test connection of this server failed ({0})"
    )]
    LookupFailed(io::Error),
    #[error(
        "the kernel cannot tell who is connecting: it knows no socket for a test connection of this server (the inet_diag and tcp_diag kernel modules may be missing)"
    )]
    NoSocket,
    #[error(
        "the kernel cannot tell who is connecting: it names uid {found} for a test connection of this server, not uid {own}"
    )]
    OtherUid { found: u32, own: u32 },
}

/// The uid this server admits, unless the kernel would show unmapped callers as that same uid.
pub fn own_account() -> Result<u32, CallersUnknown> {
    let overflow = std::fs::read_to_string(OVERFLOW_UID)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(DEFAULT_OVERFLOW_UID);
    let uid = rustix::process::getuid().as_raw();
    if uid == overflow {
        return Err(CallersUnknown::UnmappedAccount { uid });
    }
    Ok(uid)
}

/// One loopback connection of this process, looked up as a caller's would be.
pub fn check_own_connection(own_uid: u32) -> Result<(), CallersUnknown> {
    check_own_connection_with(Arc::new(uid_of_peer_socket), own_uid)
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
        Arc::new(uid_of_peer_socket),
        own_uid,
        Arc::new(|error: &io::Error| {
            eprintln!(
                "quadratic serve: the account of a caller cannot be read from the kernel ({error}); this server refuses every connection until it can"
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

/// The kernel answers a `NETLINK_SOCK_DIAG` request inside `sendto`, so the answer is already queued for `recv`.
fn uid_of_peer_socket(socket: PeerSocket) -> io::Result<Option<u32>> {
    let netlink_socket = rustix::net::socket_with(
        AddressFamily::NETLINK,
        SocketType::DGRAM,
        SocketFlags::CLOEXEC,
        Some(netlink::SOCK_DIAG),
    )?;
    rustix::net::sendto(
        &netlink_socket,
        &request(socket),
        SendFlags::empty(),
        &SocketAddrNetlink::new(0, 0),
    )?;
    let mut buffer = [0u8; 1024];
    let (read, _) = rustix::net::recv(&netlink_socket, &mut buffer[..], RecvFlags::DONTWAIT)?;
    answer(&buffer[..read], socket)
}

fn request(PeerSocket { peer, server }: PeerSocket) -> Vec<u8> {
    let mut body = vec![AF_INET, IPPROTO_TCP, 0, 0];
    body.extend(ALL_STATES.to_ne_bytes());
    body.extend(peer.port().to_be_bytes());
    body.extend(server.port().to_be_bytes());
    for address in [peer.ip(), server.ip()] {
        body.extend(address.octets());
        body.extend([0u8; 12]);
    }
    body.extend(0u32.to_ne_bytes());
    body.extend(NO_COOKIE.to_ne_bytes());
    body.extend(NO_COOKIE.to_ne_bytes());

    let length = u32::try_from(HEADER_BYTES + body.len()).unwrap_or(u32::MAX);
    let mut message = Vec::with_capacity(HEADER_BYTES + body.len());
    message.extend(length.to_ne_bytes());
    message.extend(SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    message.extend(NLM_F_REQUEST.to_ne_bytes());
    message.extend(1u32.to_ne_bytes());
    message.extend(0u32.to_ne_bytes());
    message.extend(body);
    message
}

/// Without a connected socket of those addresses and ports, the kernel answers with one that listens on the source
/// address and port: only a reply that describes the connection itself names the peer.
fn answer(bytes: &[u8], asked: PeerSocket) -> io::Result<Option<u32>> {
    let short = || io::Error::new(io::ErrorKind::InvalidData, "a short answer from sock_diag");
    let word = |at: usize| -> io::Result<[u8; 4]> {
        bytes
            .get(at..at + 4)
            .and_then(|word| word.try_into().ok())
            .ok_or_else(short)
    };
    let kind = bytes
        .get(4..6)
        .and_then(|kind| kind.try_into().ok())
        .map(u16::from_ne_bytes)
        .ok_or_else(short)?;
    match kind {
        SOCK_DIAG_BY_FAMILY => {
            let state = *bytes.get(STATE_AT).ok_or_else(short)?;
            let field =
                |at: usize, length: usize| bytes.get(SOCKID_AT + at..SOCKID_AT + at + length);
            let described = field(0, 2) == Some(&asked.peer.port().to_be_bytes()[..])
                && field(2, 2) == Some(&asked.server.port().to_be_bytes()[..])
                && field(4, 4) == Some(&asked.peer.ip().octets()[..])
                && field(20, 4) == Some(&asked.server.ip().octets()[..]);
            if state == TCP_LISTEN || !described {
                return Ok(None);
            }
            Ok(Some(u32::from_ne_bytes(word(UID_AT)?)))
        }
        NLMSG_ERROR => match -i32::from_ne_bytes(word(HEADER_BYTES)?) {
            code if code == rustix::io::Errno::NOENT.raw_os_error() => Ok(None),
            code => Err(io::Error::from_raw_os_error(code)),
        },
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("an answer of type {other} from sock_diag"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{CallersUnknown, admit_with, owner_only};
    use crate::serve::Connection;

    fn accepted() -> (TcpStream, Connection) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let client =
            TcpStream::connect(listener.local_addr().expect("an address")).expect("a connection");
        let (server_side, peer) = listener.accept().expect("the connection is accepted");
        let local = server_side.local_addr().expect("the server's address");
        (client, Connection { local, peer })
    }

    #[test]
    fn a_connection_from_this_account_is_admitted() {
        let (_client, connection) = accepted();

        assert!(owner_only(rustix::process::getuid().as_raw())(connection));
    }

    #[test]
    fn a_peer_of_another_uid_or_unknown_to_the_kernel_is_refused() {
        let (_client, connection) = accepted();
        let quiet = Arc::new(|_: &io::Error| {});
        let own_uid = rustix::process::getuid().as_raw();

        let as_another = admit_with(
            Arc::new(super::uid_of_peer_socket),
            own_uid + 1,
            quiet.clone(),
        );
        assert!(!as_another(connection));

        let gone = Connection {
            peer: SocketAddr::from(([127, 0, 0, 1], 1)),
            ..connection
        };
        assert!(!owner_only(rustix::process::getuid().as_raw())(gone));
    }

    #[test]
    fn a_peer_address_that_names_a_listener_and_no_connection_is_refused() {
        let server = TcpListener::bind("127.0.0.1:0").expect("the server's port");
        let listener_of_this_account = TcpListener::bind("127.0.0.1:0").expect("another port");
        let no_connection = Connection {
            local: server.local_addr().expect("the server's address"),
            peer: listener_of_this_account
                .local_addr()
                .expect("the listener's address"),
        };

        assert!(!owner_only(rustix::process::getuid().as_raw())(
            no_connection
        ));
    }

    #[test]
    fn a_start_whose_own_connection_the_kernel_cannot_name_fails_with_the_cause() {
        let own_uid = rustix::process::getuid().as_raw();
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
        let (_client, connection) = accepted();
        let reports = Arc::new(AtomicUsize::new(0));
        let heard = reports.clone();
        let admit = admit_with(
            Arc::new(|_| Err(io::Error::from(io::ErrorKind::PermissionDenied))),
            rustix::process::getuid().as_raw(),
            Arc::new(move |_: &io::Error| {
                heard.fetch_add(1, Ordering::Relaxed);
            }),
        );

        assert!(!admit(connection));
        assert!(!admit(connection));
        assert_eq!(reports.load(Ordering::Relaxed), 1);
    }
}
