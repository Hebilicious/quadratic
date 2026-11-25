//! Command-line tests that start no server, on every platform.

#[allow(dead_code)]
#[path = "../src/shipped.rs"]
mod shipped;

#[path = "portable/no_server.rs"]
mod no_server;
#[path = "cli/shipped_lists.rs"]
mod shipped_lists;
