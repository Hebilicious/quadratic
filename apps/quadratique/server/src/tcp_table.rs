//! The macOS kernel's table of TCP connections, as the sysctl `net.inet.tcp.pcblist_n` gives it (what `netstat`
//! reads), and the rule that picks the caller's connection in it. Plain bytes and plain Rust, so the tests run on
//! every platform; `peer_macos` reads the sysctl.
//!
//! The answer is a `struct xinpgen`, then for each connection a group of items, each starting with its length and
//! its kind and padded to 8 bytes: `struct xinpcb_n` (the endpoints), `struct xsocket_n` (with `so_uid`, the account
//! of the socket's own credential, which `fork`, inheritance and descriptor passing do not change), two
//! `struct xsockbuf_n`, `struct xsockstat_n` and `struct xtcpcb_n` (the TCP state). A last `struct xinpgen` closes it.
//! Sources: xnu-12377.121.6 `bsd/netinet/in_pcblist.c` (`get_pcblist_n`, `sotoxsocket_n`, `inpcb_to_xinpcb_n`,
//! `tcpcb_to_xtcpcb_n`), `bsd/netinet/in_pcb.h`, `bsd/sys/socketvar.h` and `bsd/netinet/tcp_var.h`. Those structures
//! are declared under `#pragma pack(4)` with fixed-width fields, so their sizes and offsets are the same on arm64 and
//! x86-64.

use std::net::{Ipv4Addr, SocketAddrV4};

/// `sizeof(struct xinpgen)`.
pub const XINPGEN_BYTES: usize = 24;
/// `sizeof(struct xinpcb_n)`.
pub const XINPCB_N_BYTES: usize = 104;
/// `sizeof(struct xsocket_n)`.
pub const XSOCKET_N_BYTES: usize = 104;
/// `sizeof(struct xtcpcb_n)`.
pub const XTCPCB_N_BYTES: usize = 204;

pub const XSO_SOCKET: u32 = 0x001;
pub const XSO_INPCB: u32 = 0x010;
pub const XSO_TCPCB: u32 = 0x020;

/// In `struct xinpcb_n`: `inp_fport` and `inp_lport` (network byte order), `inp_vflag`, and the IPv4 address of each
/// end, the last 4 bytes of a 16-byte `in_addr_4in6`.
pub const REMOTE_PORT_AT: usize = 16;
pub const LOCAL_PORT_AT: usize = 18;
pub const VERSION_AT: usize = 44;
pub const REMOTE_ADDRESS_AT: usize = 48 + 12;
pub const LOCAL_ADDRESS_AT: usize = 64 + 12;
/// In `struct xsocket_n`: `so_uid`.
pub const UID_AT: usize = 64;
/// In `struct xtcpcb_n`: `t_state`.
pub const STATE_AT: usize = 36;

const INP_IPV4: u8 = 0x1;
pub const ESTABLISHED: i32 = 4;

/// One TCP connection over IPv4 of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub local: SocketAddrV4,
    pub remote: SocketAddrV4,
    pub state: i32,
    pub uid: u32,
}

/// Why an answer of the sysctl is not read: the caller check refuses on any of them.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Malformed {
    #[error("the TCP table is shorter than its header")]
    NoHeader,
    #[error("an item of the TCP table at byte {0} is cut short or states a length it cannot have")]
    BadItem(usize),
    #[error("an item of kind {kind:#x} of the TCP table states {length} bytes, not {expected}")]
    WrongSize {
        kind: u32,
        length: usize,
        expected: usize,
    },
    #[error("a connection of the TCP table lacks its socket or its TCP block")]
    Incomplete,
    #[error("the TCP table does not end with its closing header")]
    NoEnd,
}

#[derive(Default)]
struct Group {
    endpoints: Option<(SocketAddrV4, SocketAddrV4, bool)>,
    uid: Option<u32>,
    state: Option<i32>,
}

impl Group {
    fn finish(self, records: &mut Vec<Record>) -> Result<(), Malformed> {
        let Some((local, remote, ipv4)) = self.endpoints else {
            return Ok(());
        };
        let (Some(uid), Some(state)) = (self.uid, self.state) else {
            return Err(Malformed::Incomplete);
        };
        if ipv4 {
            records.push(Record {
                local,
                remote,
                state,
                uid,
            });
        }
        Ok(())
    }
}

fn round_up_8(length: usize) -> usize {
    length.div_ceil(8) * 8
}

fn word(bytes: &[u8], at: usize) -> Option<[u8; 4]> {
    bytes.get(at..at + 4)?.try_into().ok()
}

/// The TCP connections over IPv4 that an answer of `net.inet.tcp.pcblist_n` lists.
pub fn records(bytes: &[u8]) -> Result<Vec<Record>, Malformed> {
    let header = word(bytes, 0)
        .map(u32::from_ne_bytes)
        .ok_or(Malformed::NoHeader)?;
    if header as usize != XINPGEN_BYTES || bytes.len() < XINPGEN_BYTES {
        return Err(Malformed::NoHeader);
    }
    let mut records = Vec::new();
    let mut group = Group::default();
    let mut at = round_up_8(XINPGEN_BYTES);
    loop {
        let length = word(bytes, at)
            .map(|length| u32::from_ne_bytes(length) as usize)
            .ok_or(Malformed::NoEnd)?;
        if length == XINPGEN_BYTES {
            group.finish(&mut records)?;
            return if at + XINPGEN_BYTES == bytes.len() {
                Ok(records)
            } else {
                Err(Malformed::NoEnd)
            };
        }
        let item = bytes
            .get(at..at + length)
            .filter(|_| length > 8)
            .ok_or(Malformed::BadItem(at))?;
        let kind = word(item, 4)
            .map(u32::from_ne_bytes)
            .ok_or(Malformed::BadItem(at))?;
        let sized = |expected: usize| {
            if length == expected {
                Ok(())
            } else {
                Err(Malformed::WrongSize {
                    kind,
                    length,
                    expected,
                })
            }
        };
        match kind {
            XSO_INPCB => {
                sized(XINPCB_N_BYTES)?;
                std::mem::take(&mut group).finish(&mut records)?;
                group.endpoints = Some(endpoints(item));
            }
            XSO_SOCKET => {
                sized(XSOCKET_N_BYTES)?;
                group.uid = word(item, UID_AT).map(u32::from_ne_bytes);
            }
            XSO_TCPCB => {
                sized(XTCPCB_N_BYTES)?;
                group.state = word(item, STATE_AT).map(i32::from_ne_bytes);
            }
            _ => {}
        }
        at += round_up_8(length);
    }
}

fn endpoints(item: &[u8]) -> (SocketAddrV4, SocketAddrV4, bool) {
    let port = |at: usize| u16::from_be_bytes([item[at], item[at + 1]]);
    let address = |at: usize| Ipv4Addr::new(item[at], item[at + 1], item[at + 2], item[at + 3]);
    (
        SocketAddrV4::new(address(LOCAL_ADDRESS_AT), port(LOCAL_PORT_AT)),
        SocketAddrV4::new(address(REMOTE_ADDRESS_AT), port(REMOTE_PORT_AT)),
        item[VERSION_AT] & INP_IPV4 != 0,
    )
}

/// What the table says of one accepted connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// Exactly one connection goes from the peer to the server and exactly one from the server to the peer, both
    /// established: the caller's socket belongs to this account.
    Caller { uid: u32 },
    /// No connection goes from the peer to the server.
    Nothing,
    /// The one connection from the peer to the server is not established.
    NotEstablished(i32),
    /// Two connections go from the peer to the server.
    Several,
    /// The accepted side, from the server to the peer, is not exactly one established connection.
    AcceptedSideGone,
}

/// The rule of docs/decisions/0003: the caller's socket is the accepted connection seen from the other side, and the
/// accepted side must still be connected when the decision is made.
pub fn caller(peer: SocketAddrV4, server: SocketAddrV4, records: &[Record]) -> Found {
    let between = |local: SocketAddrV4, remote: SocketAddrV4| -> Vec<&Record> {
        records
            .iter()
            .filter(|record| record.local == local && record.remote == remote)
            .collect()
    };
    let client = between(peer, server);
    let accepted = between(server, peer);
    match client.as_slice() {
        [] => Found::Nothing,
        [one] if one.state != ESTABLISHED => Found::NotEstablished(one.state),
        [one] => match accepted.as_slice() {
            [side] if side.state == ESTABLISHED => Found::Caller { uid: one.uid },
            _ => Found::AcceptedSideGone,
        },
        _ => Found::Several,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: u32 = 501;
    const NOBODY: u32 = 4_294_967_294;
    const LISTEN: i32 = 1;
    const FIN_WAIT_1: i32 = 6;
    const PEER: &str = "127.0.0.1:50123";
    const SERVER: &str = "127.0.0.1:8080";

    fn address(text: &str) -> SocketAddrV4 {
        text.parse().expect("an IPv4 address and port")
    }

    fn item(kind: u32, length: usize) -> Vec<u8> {
        let mut bytes = vec![0xAAu8; round_up_8(length)];
        bytes[0..4].copy_from_slice(&(length as u32).to_ne_bytes());
        bytes[4..8].copy_from_slice(&kind.to_ne_bytes());
        bytes
    }

    fn header() -> Vec<u8> {
        let mut bytes = vec![0u8; XINPGEN_BYTES];
        bytes[0..4].copy_from_slice(&(XINPGEN_BYTES as u32).to_ne_bytes());
        bytes
    }

    /// One connection's group of items, as `get_pcblist_n` writes it.
    fn connection(local: &str, remote: &str, state: i32, uid: u32) -> Vec<u8> {
        let (local, remote) = (address(local), address(remote));
        let mut inpcb = item(XSO_INPCB, XINPCB_N_BYTES);
        inpcb[LOCAL_PORT_AT..LOCAL_PORT_AT + 2].copy_from_slice(&local.port().to_be_bytes());
        inpcb[REMOTE_PORT_AT..REMOTE_PORT_AT + 2].copy_from_slice(&remote.port().to_be_bytes());
        inpcb[VERSION_AT] = INP_IPV4;
        inpcb[LOCAL_ADDRESS_AT..LOCAL_ADDRESS_AT + 4].copy_from_slice(&local.ip().octets());
        inpcb[REMOTE_ADDRESS_AT..REMOTE_ADDRESS_AT + 4].copy_from_slice(&remote.ip().octets());
        let mut socket = item(XSO_SOCKET, XSOCKET_N_BYTES);
        socket[UID_AT..UID_AT + 4].copy_from_slice(&uid.to_ne_bytes());
        let mut tcpcb = item(XSO_TCPCB, XTCPCB_N_BYTES);
        tcpcb[STATE_AT..STATE_AT + 4].copy_from_slice(&state.to_ne_bytes());
        [
            inpcb,
            socket,
            item(0x002, 28),
            item(0x004, 28),
            item(0x008, 408),
            tcpcb,
        ]
        .concat()
    }

    fn table(connections: &[Vec<u8>]) -> Vec<u8> {
        [vec![header()], connections.to_vec(), vec![header()]]
            .concat()
            .concat()
    }

    #[test]
    fn each_connection_is_read_with_its_endpoints_state_and_the_account_of_its_socket() {
        let bytes = table(&[
            connection(PEER, SERVER, ESTABLISHED, OWNER),
            connection(SERVER, PEER, ESTABLISHED, OWNER),
        ]);

        assert_eq!(
            records(&bytes),
            Ok(vec![
                Record {
                    local: address(PEER),
                    remote: address(SERVER),
                    state: ESTABLISHED,
                    uid: OWNER
                },
                Record {
                    local: address(SERVER),
                    remote: address(PEER),
                    state: ESTABLISHED,
                    uid: OWNER
                },
            ])
        );
        assert_eq!(records(&table(&[])), Ok(vec![]));
    }

    #[test]
    fn a_connection_over_ipv6_only_is_left_out() {
        let mut ipv6 = connection(PEER, SERVER, ESTABLISHED, OWNER);
        ipv6[VERSION_AT] = 0x2;

        assert_eq!(records(&table(&[ipv6])), Ok(vec![]));
    }

    #[test]
    fn a_short_cut_or_wrongly_sized_answer_is_refused() {
        let whole = table(&[connection(PEER, SERVER, ESTABLISHED, OWNER)]);
        let mut wrong_socket = connection(PEER, SERVER, ESTABLISHED, OWNER);
        wrong_socket[XINPCB_N_BYTES..XINPCB_N_BYTES + 4].copy_from_slice(&100u32.to_ne_bytes());
        let mut too_long = connection(PEER, SERVER, ESTABLISHED, OWNER);
        too_long[0..4].copy_from_slice(&100_000u32.to_ne_bytes());
        let without_socket: Vec<u8> = {
            let group = connection(PEER, SERVER, ESTABLISHED, OWNER);
            [
                &group[..XINPCB_N_BYTES],
                &group[XINPCB_N_BYTES + XSOCKET_N_BYTES..],
            ]
            .concat()
        };

        assert_eq!(records(&[]), Err(Malformed::NoHeader));
        assert_eq!(records(&whole[..whole.len() - 1]), Err(Malformed::NoEnd));
        assert_eq!(
            records(&whole[..whole.len() - XINPGEN_BYTES]),
            Err(Malformed::NoEnd)
        );
        assert_eq!(
            records(&[whole.clone(), vec![0; 8]].concat()),
            Err(Malformed::NoEnd)
        );
        assert!(matches!(
            records(&table(&[wrong_socket])),
            Err(Malformed::WrongSize {
                kind: XSO_SOCKET,
                length: 100,
                ..
            })
        ));
        assert!(matches!(
            records(&table(&[too_long])),
            Err(Malformed::BadItem(_))
        ));
        assert_eq!(
            records(&table(&[without_socket])),
            Err(Malformed::Incomplete)
        );
    }

    fn record(local: &str, remote: &str, state: i32, uid: u32) -> Record {
        Record {
            local: address(local),
            remote: address(remote),
            state,
            uid,
        }
    }

    fn found(records: &[Record]) -> Found {
        caller(address(PEER), address(SERVER), records)
    }

    #[test]
    fn the_caller_is_the_account_of_the_socket_from_the_peer_to_the_server() {
        let accepted = record(SERVER, PEER, ESTABLISHED, OWNER);

        assert_eq!(
            found(&[record(PEER, SERVER, ESTABLISHED, OWNER), accepted]),
            Found::Caller { uid: OWNER }
        );
        assert_eq!(
            found(&[record(PEER, SERVER, ESTABLISHED, NOBODY), accepted]),
            Found::Caller { uid: NOBODY }
        );
    }

    #[test]
    fn no_socket_another_state_two_sockets_or_a_gone_accepted_side_refuse() {
        let accepted = record(SERVER, PEER, ESTABLISHED, OWNER);
        let client = record(PEER, SERVER, ESTABLISHED, OWNER);
        let listener_on_the_peer_port = record(PEER, "0.0.0.0:0", LISTEN, OWNER);
        let other_address = record("127.0.0.2:50123", SERVER, ESTABLISHED, OWNER);

        assert_eq!(found(&[]), Found::Nothing);
        assert_eq!(
            found(&[accepted, listener_on_the_peer_port, other_address]),
            Found::Nothing
        );
        assert_eq!(
            found(&[record(PEER, SERVER, FIN_WAIT_1, OWNER), accepted]),
            Found::NotEstablished(FIN_WAIT_1)
        );
        assert_eq!(found(&[client, client, accepted]), Found::Several);
        assert_eq!(found(&[client]), Found::AcceptedSideGone);
        assert_eq!(
            found(&[client, record(SERVER, PEER, 5, OWNER)]),
            Found::AcceptedSideGone
        );
        assert_eq!(
            found(&[client, accepted, accepted]),
            Found::AcceptedSideGone
        );
    }
}
