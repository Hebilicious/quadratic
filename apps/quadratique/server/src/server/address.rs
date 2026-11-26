//! Where the server's TCP listener binds. By default it asks for the port it had last, kept in `server.port`, so that
//! open tabs and bookmarks of the web app keep working after a restart; when another program holds that port, it takes
//! one the system chooses and keeps that one. The strict setting (`quadratique server strict on`) turns keeping off:
//! each start then takes a port that this home never served, listed in `served-ports`. A port that is given, 0
//! included, reads and writes neither file. docs/decisions/0004-a-socket-for-commands-and-a-token-for-the-browser.md,
//! §4 "Port policy".

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;

use tokio::net::{TcpListener, TcpSocket};

use crate::home::{self, Prepared};

/// `server.port` holds one port number: anything longer is not one.
const PORT_FILE_LIMIT: u64 = 16;
/// How many ports `served-ports` remembers, the most recent last. Older ones may be served again.
const SERVED_PORTS: usize = 4096;
/// One port and its line end per entry.
const SERVED_PORTS_LIMIT: u64 = SERVED_PORTS as u64 * 6;
/// How many times a strict start binds again when the system gives a port this home served before.
const FRESH_TRIES: usize = 8;

/// Where a server listens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listen {
    /// On this address; a port of 0 lets the system choose. Neither `server.port` nor `served-ports` is read or
    /// written.
    On(SocketAddr),
    /// Where the home's setting says: the kept port by default, a fresh one in strict mode.
    Home,
}

/// A socket bound where `listen` says, not yet listening, and what the server records once it listens on it.
pub(super) struct Bound {
    pub socket: TcpSocket,
    pub record: Record,
    /// The kept port was bound: another start may bind it too and listen first, and the server then takes another.
    pub kept: bool,
}

pub(super) enum Record {
    Nothing,
    Kept,
    Served,
}

/// `strict` is the home's setting as this start read it, once, so that the port and the token of one start agree.
pub(super) fn bind(home: &Prepared, listen: Listen, strict: bool) -> std::io::Result<Bound> {
    match listen {
        Listen::On(address) => Ok(Bound {
            socket: bound(address)?,
            record: Record::Nothing,
            kept: false,
        }),
        Listen::Home if strict => {
            forget_kept_port(home)?;
            let served = served_ports(home);
            let socket = choose_fresh(&served, FRESH_TRIES, || {
                let socket = bound(loopback())?;
                let port = socket.local_addr()?.port();
                Ok((socket, port))
            })?;
            Ok(Bound {
                socket,
                record: Record::Served,
                kept: false,
            })
        }
        Listen::Home => {
            if let Some(port) = kept_port(home)
                && let Ok(socket) = bound(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
            {
                return Ok(Bound {
                    socket,
                    record: Record::Kept,
                    kept: true,
                });
            }
            Ok(Bound {
                socket: bound(loopback())?,
                record: Record::Kept,
                kept: false,
            })
        }
    }
}

/// Listens on the bound socket. Linux lets two sockets with `SO_REUSEADDR` bind one port while neither listens, so two
/// starts may both bind the kept port; the one that listens second gets `EADDRINUSE` and takes a port the system
/// chooses, which it keeps.
pub(super) fn listen(bound: Bound, backlog: u32) -> std::io::Result<(TcpListener, Record)> {
    match bound.socket.listen(backlog) {
        Ok(listener) => Ok((listener, bound.record)),
        Err(error) if bound.kept && error.kind() == std::io::ErrorKind::AddrInUse => {
            Ok((bound_any()?.listen(backlog)?, bound.record))
        }
        Err(error) => Err(error),
    }
}

fn bound_any() -> std::io::Result<TcpSocket> {
    bound(loopback())
}

/// Records the port the server now listens on, and says whether it is the kept one: the home keeps its address and
/// `server.port` names this port. A failure is only logged: the address is a convenience, and the server serves anyway.
pub(super) fn record(home: &Prepared, record: &Record, port: u16) -> bool {
    let written = match record {
        Record::Nothing => return false,
        Record::Kept if kept_port(home) == Some(port) => return true,
        Record::Kept => keep_port(home, port).map(|()| true),
        Record::Served => add_served(home, &[port]).map(|()| false),
    };
    written.unwrap_or_else(|error| {
        eprintln!(
            "quadratique server: could not record port {port} in {}: {error}; the next start may take another port",
            home.folder().display()
        );
        false
    })
}

fn loopback() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

fn bound(address: SocketAddr) -> std::io::Result<TcpSocket> {
    let socket = TcpSocket::new_v4()?;
    // As `TcpListener::bind` does on Unix: a restart may bind the port while connections of the last server wait out
    // TIME_WAIT. On Windows the option lets another socket take a bound port, so it stays off there.
    #[cfg(unix)]
    socket.set_reuseaddr(true)?;
    socket.bind(address)?;
    Ok(socket)
}

/// Binds until the port is not one of `served`, at most `tries` times. Every socket it rejects stays bound until it
/// returns, so the system cannot give the same port twice; after the last try it takes what it got.
fn choose_fresh<S>(
    served: &HashSet<u16>,
    tries: usize,
    mut bind: impl FnMut() -> std::io::Result<(S, u16)>,
) -> std::io::Result<S> {
    let mut rejected = Vec::new();
    loop {
        let (socket, port) = bind()?;
        if !served.contains(&port) || rejected.len() + 1 >= tries {
            return Ok(socket);
        }
        rejected.push(socket);
    }
}

/// The one definition of a kept address: the server on `port` says it kept that port at its start (`said`), the home is
/// not strict now, and `server.port` names that port now, so the next start asks for the same address. A change made
/// while the server runs (`strict on`, `server.port` edited or removed) shows at once.
pub fn address_kept(home: &Prepared, said: bool, port: u16) -> bool {
    said && !strict(home) && kept_port(home) == Some(port)
}

/// Whether this home is strict: a regular file, not a link, at `strict`.
pub fn strict(home: &Prepared) -> bool {
    std::fs::symlink_metadata(home.strict()).is_ok_and(|metadata| metadata.is_file())
}

/// Turns the strict setting on or off, for the next start of the server. On, the kept port joins the served ports and
/// is forgotten, so that no later start asks for it. Off, the next start takes a port the system chooses and keeps it.
pub fn set_strict(home: &Prepared, on: bool) -> std::io::Result<()> {
    if on {
        home::write_atomically(&home.strict(), b"on\n")?;
        return forget_kept_port(home);
    }
    remove(&home.strict())
}

/// The kept port joins the served ports, then `server.port` goes. `strict on` does it, and so does every strict start:
/// a normal start that read the setting before `strict on` may write `server.port` again after it, and the next start,
/// which is strict, forgets that port too.
fn forget_kept_port(home: &Prepared) -> std::io::Result<()> {
    if let Some(port) = kept_port(home) {
        add_served(home, &[port])?;
    }
    remove(&home.kept_port())
}

fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The kept port, or nothing when `server.port` is missing or is not a small regular file holding a port.
pub fn kept_port(home: &Prepared) -> Option<u16> {
    read_small(&home.kept_port(), PORT_FILE_LIMIT)?
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
}

fn keep_port(home: &Prepared, port: u16) -> std::io::Result<()> {
    home::write_atomically(&home.kept_port(), format!("{port}\n").as_bytes())
}

fn served_ports(home: &Prepared) -> HashSet<u16> {
    served_list(home).into_iter().collect()
}

/// The ports in the last [`SERVED_PORTS_LIMIT`] bytes of `served-ports`: a longer file, or a line that is not a port,
/// costs only what cannot be read, never the rest of the list.
fn served_list(home: &Prepared) -> Vec<u16> {
    let Some(bytes) = read_tail(&home.served_ports(), SERVED_PORTS_LIMIT) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|line| line.trim().parse::<u16>().ok())
        .collect()
}

/// At most the last `limit` bytes of a regular file, starting at a line, or nothing when it is not a regular file.
fn read_tail(path: &Path, limit: u64) -> Option<Vec<u8>> {
    use std::io::{Seek, SeekFrom};

    let looked = std::fs::symlink_metadata(path).ok()?;
    if !looked.is_file() {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(limit);
    // From the byte before the cut, which tells whether the cut lands at the start of a line.
    let before = start.saturating_sub(1);
    file.seek(SeekFrom::Start(before)).ok()?;
    let mut bytes = Vec::new();
    file.take(limit + (start - before))
        .read_to_end(&mut bytes)
        .ok()?;
    if start > 0 {
        let line = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |end| end + 1);
        bytes.drain(..line);
    }
    Some(bytes)
}

fn add_served(home: &Prepared, ports: &[u16]) -> std::io::Result<()> {
    let mut list = served_list(home);
    list.retain(|served| !ports.contains(served));
    list.extend_from_slice(ports);
    let start = list.len().saturating_sub(SERVED_PORTS);
    let text: String = list[start..]
        .iter()
        .map(|port| format!("{port}\n"))
        .collect();
    home::write_atomically(&home.served_ports(), text.as_bytes())
}

/// The text of a regular file of at most `limit` bytes: a link, a FIFO, a folder or a longer file gives nothing. The
/// data folder is private to its account, so nothing else can swap the file between the check and the read.
fn read_small(path: &Path, limit: u64) -> Option<String> {
    let looked = std::fs::symlink_metadata(path).ok()?;
    if !looked.is_file() || looked.len() > limit {
        return None;
    }
    let file = File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(limit).read_to_string(&mut text).ok()?;
    Some(text)
}

#[cfg(all(test, unix))]
mod home_tests {
    use std::os::unix::fs::PermissionsExt;

    use super::{Record, add_served, record, served_list};
    use crate::home::{self, Prepared};
    use crate::server::Paths;

    fn home() -> (tempfile::TempDir, Prepared) {
        let folder = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("a private folder");
        let prepared =
            home::prepare(&Paths::in_home(folder.path()).expect("a path")).expect("a home");
        (folder, prepared)
    }

    #[cfg(target_os = "linux")]
    fn held_port() -> (tokio::net::TcpSocket, u16) {
        use std::net::{Ipv4Addr, SocketAddr};
        use tokio::net::TcpSocket;

        let socket = TcpSocket::new_v4().expect("a socket");
        socket.set_reuseaddr(true).expect("SO_REUSEADDR");
        socket
            .bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("a loopback port");
        let port = socket.local_addr().expect("its address").port();
        (socket, port)
    }

    /// Two starts may race for the kept port: both bind it, as `SO_REUSEADDR` allows on Linux while neither listens,
    /// and the one that listens second must not fail. macOS refuses the second bind already, and the server then takes
    /// another port at its bind.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn when_another_socket_listens_on_the_kept_port_between_the_bind_and_the_listen_the_server_takes_another()
     {
        use super::{Listen, bind, listen};

        let (_folder, home) = home();
        let (other, port) = held_port();
        std::fs::write(home.kept_port(), format!("{port}\n")).expect("a kept port");
        let bound = bind(&home, Listen::Home, false).expect("the kept port is bound");
        assert_eq!(bound.socket.local_addr().expect("its address").port(), port);
        let _other = other.listen(16).expect("the other socket listens first");

        let listened = listen(bound, 16);

        let (listener, _) = listened.expect("the server listens on another port");
        assert_ne!(listener.local_addr().expect("its address").port(), port);
    }

    #[test]
    fn a_served_ports_file_cut_exactly_at_a_line_start_keeps_that_line() {
        let (_folder, home) = home();
        // Lines of six bytes: the last SERVED_PORTS_LIMIT bytes start exactly at a line.
        let text: String = (10000..15000u16).map(|port| format!("{port}\n")).collect();
        std::fs::write(home.served_ports(), text).expect("a long file");

        let kept = served_list(&home);

        assert_eq!(kept.len(), super::SERVED_PORTS);
        assert_eq!(kept.first(), Some(&(15000 - super::SERVED_PORTS as u16)));
    }

    #[test]
    fn a_write_of_the_home_never_touches_a_file_at_another_writers_partial_name() {
        let (_folder, home) = home();
        let other = home
            .kept_port()
            .with_file_name(format!("server.port.{}", std::process::id()));
        std::fs::write(&other, "another write in progress").expect("another partial file");

        assert!(record(&home, &Record::Kept, 4100), "the port is kept");

        assert_eq!(
            std::fs::read_to_string(&other).expect("the other partial file"),
            "another write in progress"
        );
    }

    #[test]
    fn a_strict_start_forgets_a_port_that_a_racing_normal_start_kept_after_strict_on() {
        let (_folder, home) = home();
        super::set_strict(&home, true).expect("strict on");
        std::fs::write(home.kept_port(), "4100\n").expect("a port kept after strict on");

        let bound = super::bind(&home, super::Listen::Home, true).expect("a strict start binds");
        drop(bound);

        assert!(!home.kept_port().exists(), "server.port stays under strict");
        assert!(
            served_list(&home).contains(&4100),
            "the kept port can be served again"
        );
    }

    #[test]
    fn a_long_or_partly_unreadable_served_ports_file_keeps_its_last_ports() {
        let (_folder, home) = home();
        let mut text: Vec<u8> = (1000..6000u16)
            .flat_map(|port| format!("{port}\n").into_bytes())
            .collect();
        text.extend_from_slice(b"\xff\xfe not a port\n6001\n");
        std::fs::write(home.served_ports(), &text).expect("a long file");

        add_served(&home, &[7000]).expect("a port is added");

        let kept = served_list(&home);
        assert!(
            kept.contains(&5999) && kept.contains(&6001),
            "{} ports",
            kept.len()
        );
        assert_eq!(kept.last(), Some(&7000));
        assert!(kept.len() > 3000, "only {} ports kept", kept.len());
    }

    #[test]
    fn a_file_of_the_home_is_never_written_through_a_link_at_its_partial_name() {
        let (_folder, home) = home();
        let elsewhere = tempfile::tempdir().expect("a folder outside the home");
        let target = elsewhere.path().join("target");
        std::fs::write(&target, "untouched").expect("a file outside the home");
        let partial = home
            .kept_port()
            .with_file_name(format!("server.port.{}", std::process::id()));
        std::os::unix::fs::symlink(&target, &partial).expect("a link at the partial name");

        assert!(record(&home, &Record::Kept, 4100), "the port is kept");

        assert_eq!(
            std::fs::read_to_string(&target).expect("the target"),
            "untouched"
        );
        let mode = std::fs::metadata(home.kept_port())
            .expect("server.port")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(
            std::fs::read_to_string(home.kept_port()).expect("server.port"),
            "4100\n"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::choose_fresh;

    fn ports(sequence: &[u16]) -> impl FnMut() -> std::io::Result<(u16, u16)> + '_ {
        let mut next = sequence.iter();
        move || {
            let port = *next.next().expect("a port left in the sequence");
            Ok((port, port))
        }
    }

    #[test]
    fn a_strict_start_binds_again_while_the_system_gives_a_served_port() {
        let served = HashSet::from([4100, 4101]);

        let chosen = choose_fresh(&served, 8, ports(&[4100, 4101, 4102])).expect("a port");

        assert_eq!(chosen, 4102);
    }

    #[test]
    fn a_strict_start_takes_the_last_port_it_got_after_its_tries() {
        let served = HashSet::from([4100, 4101, 4102]);

        let chosen = choose_fresh(&served, 3, ports(&[4100, 4101, 4102, 4103])).expect("a port");

        assert_eq!(chosen, 4102);
    }
}
