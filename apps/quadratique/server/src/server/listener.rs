//! The web app's TCP listener, which a server opens only when a browser may use it: at its start when an earlier
//! pairing made the token that a browser keeps, otherwise at the first pairing (`quadratique ui`). A server that only
//! agents use listens on no TCP port. docs/decisions/0004-a-socket-for-commands-and-a-token-for-the-browser.md, §4.
//!
//! | State       | `open`                                             | `stop`                    |
//! | ----------- | -------------------------------------------------- | ------------------------- |
//! | `Closed`    | binds where the port policy says, serves: `Open`   | `Stopped`, nothing to end |
//! | `Open`      | the same address                                   | `Stopped`, its listener   |
//! | `Stopped`   | refused: the server is stopping                    | `Stopped`                 |

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use acid_cat_core::remote::Routes;

use super::LISTEN_BACKLOG;
use super::address::{self, Listen};
use crate::home::Prepared;
use crate::serve::{self, Gate, Served};
use crate::web::Origin;

/// Where the web app listens once it is open, and whether that is the port its home keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listening {
    pub address: SocketAddr,
    pub kept: bool,
}

/// What opening needs: the port policy as this start read it, the routes, and what stops its connections.
pub(super) struct Opening {
    pub home: Prepared,
    pub listen: Listen,
    pub strict: bool,
    pub routes: Routes,
    pub request_limit: std::time::Duration,
    pub gate: Gate,
    pub origin: Origin,
}

enum State {
    Closed(Opening),
    Open(Served),
    Stopped,
}

pub(super) struct WebListener {
    state: tokio::sync::Mutex<State>,
    listening: OnceLock<Listening>,
}

impl WebListener {
    pub(super) fn new(opening: Opening) -> Arc<WebListener> {
        Arc::new(WebListener {
            state: tokio::sync::Mutex::new(State::Closed(opening)),
            listening: OnceLock::new(),
        })
    }

    /// Where it listens, once it is open.
    pub(super) fn listening(&self) -> Option<Listening> {
        self.listening.get().copied()
    }

    /// Listens, if it does not yet, and gives the address. Two callers at once get the same listener.
    pub(super) async fn open(&self) -> std::io::Result<SocketAddr> {
        let mut state = self.state.lock().await;
        let opening = match std::mem::replace(&mut *state, State::Stopped) {
            State::Open(served) => {
                let address = served.address;
                *state = State::Open(served);
                return Ok(address);
            }
            State::Stopped => {
                return Err(std::io::Error::other(
                    "the server is stopping, and opens no listener for the web app",
                ));
            }
            State::Closed(opening) => opening,
        };
        match listen(&opening) {
            Ok((served, record)) => {
                let address = served.address;
                let kept = address::record(&opening.home, &record, address.port());
                opening.origin.set(address);
                served.close_with(&opening.gate);
                let _ = self.listening.set(Listening { address, kept });
                *state = State::Open(served);
                Ok(address)
            }
            Err(error) => {
                *state = State::Closed(opening);
                Err(error)
            }
        }
    }

    /// The TCP connections open now, none while the listener is not open.
    pub(super) async fn open_connections(&self) -> usize {
        match &*self.state.lock().await {
            State::Open(served) => served.open_connections(),
            State::Closed(_) | State::Stopped => 0,
        }
    }

    /// Opens no listener from now on, and gives the one that is open, for the server to end.
    pub(super) async fn stop(&self) -> Option<Served> {
        match std::mem::replace(&mut *self.state.lock().await, State::Stopped) {
            State::Open(served) => Some(served),
            State::Closed(_) | State::Stopped => None,
        }
    }
}

fn listen(opening: &Opening) -> std::io::Result<(Served, address::Record)> {
    let bound = address::bind(&opening.home, opening.listen, opening.strict)?;
    let (listener, record) = address::listen(bound, LISTEN_BACKLOG)?;
    let served = serve::serve_on(
        listener,
        opening.routes.clone(),
        serve::TCP_CONNECTIONS,
        opening.request_limit,
    )?;
    Ok((served, record))
}
