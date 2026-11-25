// doltlite links zlib by name, and this is the zlib it finds on Windows.
#[cfg(windows)]
use libz_sys as _;

pub mod app;
pub mod assets;
pub mod credentials;
pub mod domain;
pub mod git;
#[cfg(target_os = "linux")]
pub mod peer;
pub mod serve;
pub mod server;
pub mod store;
pub mod web;
