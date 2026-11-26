// doltlite links zlib by name, and this is the zlib it finds on Windows.
#[cfg(windows)]
use libz_sys as _;

pub mod app;
pub mod assets;
pub mod credentials;
pub mod domain;
pub mod git;
pub mod home;
pub mod pairing;
pub mod serve;
pub mod server;
pub mod store;
pub mod web;
