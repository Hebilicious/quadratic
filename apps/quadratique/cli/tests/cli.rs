//! These tests start a server and watch processes through Linux's pidfd, /proc and inotify, so they run on Linux.
//! Tests that start no server are in tests/portable.rs.
#![cfg(target_os = "linux")]

#[path = "cli/support.rs"]
mod support;

#[path = "cli/advance.rs"]
mod advance;
#[path = "cli/claims.rs"]
mod claims;
#[path = "cli/create.rs"]
mod create;
#[path = "cli/human_word.rs"]
mod human_word;
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
#[path = "cli/socket_only.rs"]
mod socket_only;
#[path = "cli/states.rs"]
mod states;
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
