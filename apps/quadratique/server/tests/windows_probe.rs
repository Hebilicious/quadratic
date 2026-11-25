//! Temporary diagnosis for QUA-32, removed before review: does doltlite open a store on Windows, by a verbatim and by
//! a plain path?
#![cfg(windows)]

use doltlite::Connection;

fn probe(label: &str, path: &std::path::Path) {
    let opened = Connection::open(path);
    let Ok(connection) = opened else {
        println!("{label}: open fails: {:?}", opened.err());
        return;
    };
    println!("{label}: {}", path.display());
    for sql in [
        "SELECT dolt_version()",
        "SELECT dolt_hashof('HEAD')",
        "SELECT active_branch()",
        "SELECT name FROM dolt_branches",
    ] {
        println!("  {sql}: {:?}", connection.query_row(sql, &[]));
    }
}

#[test]
fn doltlite_on_windows() {
    let folder = tempfile::tempdir().expect("a folder");
    let plain = folder.path().join("plain.db");
    probe("plain", &plain);
    let verbatim = folder.path().canonicalize().expect("canonical").join("verbatim.db");
    probe("verbatim", &verbatim);
    let relative_folder = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(folder.path()).expect("cd");
    probe("relative", std::path::Path::new("relative.db"));
    std::env::set_current_dir(relative_folder).expect("cd back");
}
