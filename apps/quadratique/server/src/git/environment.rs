//! The environment of every git subprocess that the server starts.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

static NO_USER_CONFIGURATION: AtomicBool = AtomicBool::new(false);

/// From now on, every git subprocess of this process reads an empty global and system
/// configuration, unless its repository names a global file of its own. For tests, so that the
/// git configuration of the machine they run on changes no answer.
pub fn read_no_user_configuration() {
    NO_USER_CONFIGURATION.store(true, Ordering::Relaxed);
}

/// The global configuration that git reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Configuration {
    /// The person's own global and system configuration.
    #[default]
    User,
    /// This file as the global configuration, and no system configuration.
    Global(PathBuf),
}

/// Switch off git's Trace2 for `command`. Trace2 is switched on from global or system
/// configuration as well as from the environment, and for every command its `def_param` events
/// print configuration values, such as a remote's URL with the credential it holds. The older
/// GIT_TRACE* variables have no configuration, and print no configuration value.
pub fn without_trace(command: &mut Command) -> &mut Command {
    for name in ["GIT_TRACE2", "GIT_TRACE2_EVENT", "GIT_TRACE2_PERF"] {
        command.env(name, "0");
    }
    command
}

/// Prepare `command` to run git: no variable of the server's own environment reaches git, and git
/// writes no trace.
pub fn prepare(command: &mut Command, configuration: &Configuration) {
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    without_trace(command);
    command.env("GIT_TERMINAL_PROMPT", "0").env("LC_ALL", "C");
    let global: Option<OsString> = match configuration {
        Configuration::Global(path) => Some(path.clone().into_os_string()),
        Configuration::User if NO_USER_CONFIGURATION.load(Ordering::Relaxed) => {
            Some("/dev/null".into())
        }
        Configuration::User => None,
    };
    if let Some(global) = global {
        command
            .env("GIT_CONFIG_GLOBAL", global)
            .env("GIT_CONFIG_NOSYSTEM", "1");
    }
}
