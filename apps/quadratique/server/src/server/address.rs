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

use tokio::net::TcpSocket;

use crate::home::Prepared;

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
}

pub(super) enum Record {
    Nothing,
    Kept,
    Served,
}

pub(super) fn bind(home: &Prepared, listen: Listen) -> std::io::Result<Bound> {
    match listen {
        Listen::On(address) => Ok(Bound {
            socket: bound(address)?,
            record: Record::Nothing,
        }),
        Listen::Home if strict(home) => {
            let served = served_ports(home);
            let socket = choose_fresh(&served, FRESH_TRIES, || {
                let socket = bound(loopback())?;
                let port = socket.local_addr()?.port();
                Ok((socket, port))
            })?;
            Ok(Bound {
                socket,
                record: Record::Served,
            })
        }
        Listen::Home => {
            if let Some(port) = kept_port(home)
                && let Ok(socket) = bound(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
            {
                return Ok(Bound {
                    socket,
                    record: Record::Kept,
                });
            }
            Ok(Bound {
                socket: bound(loopback())?,
                record: Record::Kept,
            })
        }
    }
}

/// Records the port the server now listens on. A failure is only logged: the address is a convenience, and the server
/// serves anyway.
pub(super) fn record(home: &Prepared, record: &Record, port: u16) {
    let written = match record {
        Record::Nothing => return,
        Record::Kept if kept_port(home) == Some(port) => return,
        Record::Kept => keep_port(home, port),
        Record::Served => add_served(home, &[port]),
    };
    if let Err(error) = written {
        eprintln!(
            "quadratique server: could not record port {port} in {}: {error}; the next start may take another port",
            home.folder().display()
        );
    }
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

/// Whether this home is strict: a regular file, not a link, at `strict`.
pub fn strict(home: &Prepared) -> bool {
    std::fs::symlink_metadata(home.strict()).is_ok_and(|metadata| metadata.is_file())
}

/// Turns the strict setting on or off. On, the kept port joins the served ports and is forgotten, so that no later
/// start asks for it. Off, `port`, when given, is kept at once: the port of the server that runs now.
pub fn set_strict(home: &Prepared, on: bool, port: Option<u16>) -> std::io::Result<()> {
    if on {
        let mut ports: Vec<u16> = kept_port(home).into_iter().collect();
        ports.extend(port);
        add_served(home, &ports)?;
        write_atomically(&home.strict(), b"on\n")?;
        return remove(&home.kept_port());
    }
    remove(&home.strict())?;
    match port {
        Some(port) => keep_port(home, port),
        None => Ok(()),
    }
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
    write_atomically(&home.kept_port(), format!("{port}\n").as_bytes())
}

fn served_ports(home: &Prepared) -> HashSet<u16> {
    served_list(home).into_iter().collect()
}

fn served_list(home: &Prepared) -> Vec<u16> {
    read_small(&home.served_ports(), SERVED_PORTS_LIMIT)
        .map(|text| {
            text.lines()
                .filter_map(|line| line.trim().parse::<u16>().ok())
                .collect()
        })
        .unwrap_or_default()
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
    write_atomically(&home.served_ports(), text.as_bytes())
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

/// Writes a sibling file, then renames it over `path`. The sibling is removed when either step fails.
fn write_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let partial = path.with_file_name(format!("{name}.{}", std::process::id()));
    let named = |what: &str, error: std::io::Error| {
        std::io::Error::new(
            error.kind(),
            format!("{what} {}: {error}", partial.display()),
        )
    };
    let written = std::fs::write(&partial, contents)
        .map_err(|error| named("writing", error))
        .and_then(|()| std::fs::rename(&partial, path).map_err(|error| named("renaming", error)));
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    written
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
