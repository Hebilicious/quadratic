//! What the macOS caller check reads from `struct socket_fdinfo` (`<sys/proc_info.h>`), which
//! `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)` fills, and how it picks the one socket that is the other end of a
//! connection. Plain bytes and plain Rust, so the tests run on every platform; `peer_macos` asks the kernel.
//!
//! The layout is the same on arm64 and x86_64 macOS. On macOS the build checks these offsets against the `libproc`
//! crate's own declaration of the structure (`peer_macos`), and the server's start checks them against the kernel by
//! looking up a connection of its own.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddrV4};

/// `sizeof(struct socket_fdinfo)`. The kernel refuses a smaller buffer, so a macOS that grew the structure makes
/// every lookup fail, and the server refuses to start.
pub const BYTES: usize = 792;
/// `psi.soi_so`: the kernel's (permuted) address of the socket, the same in every process that shares it.
pub const SOCKET_AT: usize = 160;
/// `psi.soi_kind`.
pub const KIND_AT: usize = 256;
/// `psi.soi_proto`, here `pri_tcp`.
pub const PROTO_AT: usize = 264;
/// In `struct in_sockinfo`, at the start of `struct tcp_sockinfo`: the remote and local ports, the IP version flags,
/// and the IPv4 address of each end, the last 4 bytes of a 16-byte `in4in6_addr`.
pub const REMOTE_PORT_AT: usize = PROTO_AT;
pub const LOCAL_PORT_AT: usize = PROTO_AT + 4;
pub const VERSION_AT: usize = PROTO_AT + 24;
pub const REMOTE_ADDRESS_AT: usize = PROTO_AT + 32 + 12;
pub const LOCAL_ADDRESS_AT: usize = PROTO_AT + 48 + 12;
/// `tcpsi_state`, after the 80 bytes of `struct in_sockinfo`.
pub const STATE_AT: usize = PROTO_AT + 80;

const SOCKINFO_TCP: i32 = 2;
const INI_IPV4: u8 = 0x1;
pub const ESTABLISHED: i32 = 4;

/// One TCP socket over IPv4, as a process's descriptor shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TcpSocket {
    pub socket: u64,
    pub state: i32,
    pub local: SocketAddrV4,
    pub remote: SocketAddrV4,
}

/// The TCP socket over IPv4 that `bytes` describe; `None` for any other socket.
pub fn tcp_over_ipv4(bytes: &[u8]) -> Option<TcpSocket> {
    let array = |at: usize| -> Option<[u8; 4]> { bytes.get(at..at + 4)?.try_into().ok() };
    let int = |at: usize| array(at).map(i32::from_ne_bytes);
    // The kernel copies `inp_lport`, a port in network byte order, into an `int`.
    let port = |at: usize| int(at).map(|value| u16::from_be(value as u16));
    let address = |at: usize| array(at).map(Ipv4Addr::from);
    if int(KIND_AT)? != SOCKINFO_TCP || bytes.get(VERSION_AT)? & INI_IPV4 == 0 {
        return None;
    }
    Some(TcpSocket {
        socket: u64::from_ne_bytes(bytes.get(SOCKET_AT..SOCKET_AT + 8)?.try_into().ok()?),
        state: int(STATE_AT)?,
        local: SocketAddrV4::new(address(LOCAL_ADDRESS_AT)?, port(LOCAL_PORT_AT)?),
        remote: SocketAddrV4::new(address(REMOTE_ADDRESS_AT)?, port(REMOTE_PORT_AT)?),
    })
}

/// What a scan of the owner's descriptors found for one accepted connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// Exactly one connected socket has the peer's address as its local end and the server's as its remote end,
    /// and it is established. These processes hold it: several when `fork` shares it, in the order of the scan.
    Holders(Vec<u32>),
    /// No socket has both endpoints.
    Nothing,
    /// The one socket with both endpoints is not established.
    NotEstablished(i32),
    /// Two different sockets have both endpoints.
    Several,
}

/// The rule of docs/decisions/0003: the client's socket is the accepted connection seen from the other side.
pub fn holder_of(
    peer: SocketAddrV4,
    server: SocketAddrV4,
    sockets: impl IntoIterator<Item = (u32, TcpSocket)>,
) -> Found {
    let matching: Vec<(u32, TcpSocket)> = sockets
        .into_iter()
        .filter(|(_, socket)| socket.local == peer && socket.remote == server)
        .collect();
    let distinct: BTreeSet<u64> = matching.iter().map(|(_, socket)| socket.socket).collect();
    match (distinct.len(), matching.first()) {
        (0, _) | (_, None) => Found::Nothing,
        (1, Some((_, socket))) if socket.state == ESTABLISHED => {
            Found::Holders(matching.iter().map(|(pid, _)| *pid).collect())
        }
        (1, Some((_, socket))) => Found::NotEstablished(socket.state),
        _ => Found::Several,
    }
}

/// The account to admit for a socket that `holders` hold, read again after the scan: `own_uid` when any holder still
/// runs as `own_uid`, else the first account that can be read. A child that shared the socket between `fork` and
/// `exec`, and has ended since, must not hide the parent that still holds it; a process id reused by another
/// account names that account.
pub fn account_of_holders(
    holders: &[u32],
    own_uid: u32,
    account_of: impl Fn(u32) -> Option<u32>,
) -> Option<u32> {
    let accounts: Vec<u32> = holders.iter().filter_map(|pid| account_of(*pid)).collect();
    if accounts.contains(&own_uid) {
        return Some(own_uid);
    }
    accounts.first().copied()
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddrV4;

    use super::*;

    const LISTEN: i32 = 1;
    const FIN_WAIT_1: i32 = 6;

    fn address(text: &str) -> SocketAddrV4 {
        text.parse().expect("an IPv4 address and port")
    }

    /// The bytes the kernel writes for a TCP socket, field by field as `<sys/proc_info.h>` lays them out.
    fn described(socket: u64, state: i32, local: &str, remote: &str) -> Vec<u8> {
        let (local, remote) = (address(local), address(remote));
        let mut bytes = vec![0xAAu8; BYTES];
        let mut put = |at: usize, value: &[u8]| bytes[at..at + value.len()].copy_from_slice(value);
        put(SOCKET_AT, &socket.to_ne_bytes());
        put(KIND_AT, &SOCKINFO_TCP.to_ne_bytes());
        put(VERSION_AT, &[INI_IPV4]);
        put(STATE_AT, &state.to_ne_bytes());
        let port = |port: u16| i32::from(port.to_be()).to_ne_bytes();
        put(LOCAL_PORT_AT, &port(local.port()));
        put(REMOTE_PORT_AT, &port(remote.port()));
        put(LOCAL_ADDRESS_AT, &local.ip().octets());
        put(REMOTE_ADDRESS_AT, &remote.ip().octets());
        bytes
    }

    #[test]
    fn a_tcp_socket_over_ipv4_is_read_with_both_endpoints_and_its_state() {
        let bytes = described(7, ESTABLISHED, "127.0.0.1:50123", "127.0.0.1:8080");

        assert_eq!(
            tcp_over_ipv4(&bytes),
            Some(TcpSocket {
                socket: 7,
                state: ESTABLISHED,
                local: address("127.0.0.1:50123"),
                remote: address("127.0.0.1:8080"),
            })
        );
    }

    #[test]
    fn other_sockets_and_short_answers_are_not_read() {
        let tcp = described(7, ESTABLISHED, "127.0.0.1:50123", "127.0.0.1:8080");
        let mut unix = tcp.clone();
        unix[KIND_AT..KIND_AT + 4].copy_from_slice(&3i32.to_ne_bytes());
        let mut ipv6_only = tcp.clone();
        ipv6_only[VERSION_AT] = 0x2;

        assert_eq!(tcp_over_ipv4(&unix), None);
        assert_eq!(tcp_over_ipv4(&ipv6_only), None);
        assert_eq!(tcp_over_ipv4(&tcp[..STATE_AT]), None);
    }

    fn socket(socket: u64, state: i32, local: &str, remote: &str) -> TcpSocket {
        tcp_over_ipv4(&described(socket, state, local, remote)).expect("a TCP socket")
    }

    const PEER: &str = "127.0.0.1:50123";
    const SERVER: &str = "127.0.0.1:8080";

    fn found(sockets: Vec<(u32, TcpSocket)>) -> Found {
        holder_of(address(PEER), address(SERVER), sockets)
    }

    #[test]
    fn the_one_established_socket_from_the_peer_to_the_server_names_its_process() {
        let accepted_side = socket(1, ESTABLISHED, SERVER, PEER);
        let client = socket(2, ESTABLISHED, PEER, SERVER);
        let unrelated = socket(3, ESTABLISHED, PEER, "127.0.0.1:9090");

        assert_eq!(
            found(vec![(10, accepted_side), (20, client), (30, unrelated)]),
            Found::Holders(vec![20])
        );
    }

    #[test]
    fn one_socket_shared_by_fork_counts_once() {
        let client = socket(2, ESTABLISHED, PEER, SERVER);

        assert_eq!(
            found(vec![(21, client), (20, client)]),
            Found::Holders(vec![21, 20])
        );
    }

    #[test]
    fn a_holder_that_ended_after_the_scan_does_not_hide_one_that_still_holds_the_socket() {
        const OWN: u32 = 501;
        let ended_child = 21;
        let parent = 20;
        let reused_by_another = 22;
        let accounts = |pid: u32| match pid {
            20 => Some(OWN),
            22 => Some(OWN + 1),
            _ => None,
        };

        assert_eq!(
            account_of_holders(&[ended_child, parent], OWN, accounts),
            Some(OWN)
        );
        assert_eq!(account_of_holders(&[ended_child], OWN, accounts), None);
        assert_eq!(
            account_of_holders(&[reused_by_another], OWN, accounts),
            Some(OWN + 1)
        );
        assert_eq!(
            account_of_holders(&[reused_by_another, parent], OWN, accounts),
            Some(OWN)
        );
    }

    #[test]
    fn no_socket_a_listener_another_state_or_two_sockets_refuse() {
        let the_server_side_only = socket(1, ESTABLISHED, SERVER, PEER);
        let listener_on_the_peer_port = socket(4, LISTEN, PEER, "0.0.0.0:0");
        let a_listener_with_both_endpoints = socket(5, LISTEN, PEER, SERVER);
        let closing = socket(6, FIN_WAIT_1, PEER, SERVER);
        let other_address = socket(8, ESTABLISHED, "127.0.0.2:50123", SERVER);
        let first = socket(2, ESTABLISHED, PEER, SERVER);
        let second = socket(9, ESTABLISHED, PEER, SERVER);

        assert_eq!(found(vec![]), Found::Nothing);
        assert_eq!(
            found(vec![
                (10, the_server_side_only),
                (11, listener_on_the_peer_port),
                (12, other_address)
            ]),
            Found::Nothing
        );
        assert_eq!(
            found(vec![(13, a_listener_with_both_endpoints)]),
            Found::NotEstablished(LISTEN)
        );
        assert_eq!(
            found(vec![(14, closing)]),
            Found::NotEstablished(FIN_WAIT_1)
        );
        assert_eq!(found(vec![(20, first), (21, second)]), Found::Several);
    }
}
