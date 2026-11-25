//! Every test here starts a server, and the server serves only on Linux until it can tell which account connects to
//! its local listener elsewhere (docs/decisions/0003-the-caller-check-on-macos-and-windows.md). The issues that bring
//! the check to macOS (QUA-33) and Windows (QUA-34) make these tests run there.
#![cfg(target_os = "linux")]

#[path = "cli/support.rs"]
mod support;

#[allow(dead_code)]
#[path = "../src/shipped.rs"]
mod shipped;

#[path = "cli/advance.rs"]
mod advance;
#[path = "cli/callers.rs"]
mod callers;
#[path = "cli/claims.rs"]
mod claims;
#[path = "cli/create.rs"]
mod create;
#[path = "cli/import.rs"]
mod import;
#[path = "cli/init.rs"]
mod init;
#[path = "cli/init_update.rs"]
mod init_update;
#[path = "cli/mcp.rs"]
mod mcp;
#[path = "cli/other_build.rs"]
mod other_build;
#[path = "cli/server_life.rs"]
mod server_life;
#[path = "cli/shipped_lists.rs"]
mod shipped_lists;
#[path = "cli/tokens.rs"]
mod tokens;
#[path = "cli/traces.rs"]
mod traces;
#[path = "cli/ui.rs"]
mod ui;
#[path = "cli/unreachable.rs"]
mod unreachable;
#[path = "cli/watch.rs"]
mod watch;
