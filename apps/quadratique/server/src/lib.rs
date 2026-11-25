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
#[cfg(target_os = "macos")]
pub mod peer_macos;
pub mod serve;
pub mod server;
#[cfg(any(test, target_os = "macos"))]
pub mod socket_fdinfo;
pub mod store;
pub mod web;
