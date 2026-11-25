//! Server tests that start no server, so no caller check: they run on every platform.

#[path = "portable/parity.rs"]
mod parity;
#[path = "portable/transport.rs"]
mod transport;
