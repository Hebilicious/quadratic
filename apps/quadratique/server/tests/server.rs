//! Every test here starts a server, and the server serves only on Linux and macOS until it can tell which account
//! connects to its local listener on Windows (docs/decisions/0003-the-caller-check-on-macos-and-windows.md). QUA-34
//! makes these tests run there.
#![cfg(any(target_os = "linux", target_os = "macos"))]

#[path = "server/repository.rs"]
mod repository;
#[path = "server/support.rs"]
mod support;

#[path = "server/advance.rs"]
mod advance;
#[path = "server/changed.rs"]
mod changed;
#[path = "server/claims.rs"]
mod claims;
#[path = "server/close_several.rs"]
mod close_several;
#[path = "server/creation.rs"]
mod creation;
#[path = "server/ended_holder.rs"]
mod ended_holder;
#[path = "server/failures.rs"]
mod failures;
#[path = "server/findings.rs"]
mod findings;
#[path = "server/git.rs"]
mod git;
#[path = "server/history.rs"]
mod history;
#[path = "server/human.rs"]
mod human;
#[path = "server/instructions.rs"]
mod instructions;
#[path = "server/links.rs"]
mod links;
#[path = "server/notes.rs"]
mod notes;
#[path = "server/other_accounts.rs"]
mod other_accounts;
#[path = "server/process.rs"]
mod process;
#[path = "server/projects.rs"]
mod projects;
#[path = "server/ready.rs"]
mod ready;
#[path = "server/stacked.rs"]
mod stacked;
#[path = "server/tokens.rs"]
mod tokens;
#[path = "server/updates.rs"]
mod updates;
#[path = "server/watcher.rs"]
mod watcher;
#[path = "server/web.rs"]
mod web;
