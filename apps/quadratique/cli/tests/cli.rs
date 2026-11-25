//! These tests start a server, or watch processes through Linux's pidfd and /proc, and the server serves only on
//! Linux until it can tell which account connects to its local listener elsewhere
//! (docs/decisions/0003-the-caller-check-on-macos-and-windows.md). The issues that bring the check to macOS (QUA-33)
//! and Windows (QUA-34) make these tests run there. Tests that start no server are in tests/portable.rs.
#![cfg(target_os = "linux")]

#[path = "cli/support.rs"]
mod support;

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
#[path = "cli/status.rs"]
mod status;
#[path = "cli/tokens.rs"]
mod tokens;
#[path = "cli/traces.rs"]
mod traces;
#[path = "cli/ui.rs"]
mod ui;
#[path = "cli/unreachable.rs"]
mod unreachable;
#[path = "cli/upgrade.rs"]
mod upgrade;
#[path = "cli/watch.rs"]
mod watch;
