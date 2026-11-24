use std::path::PathBuf;

#[path = "src/assets.rs"]
mod assets;

const DIST_VARIABLE: &str = "QUADRATIC_WEB_DIST";
const PACKAGED_DIST: &str = "web-dist";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src/assets.rs");
    if std::env::var_os("CARGO_FEATURE_EMBEDDED_WEB").is_none() {
        return;
    }
    println!("cargo::rerun-if-env-changed={DIST_VARIABLE}");
    let dist = std::env::var_os(DIST_VARIABLE)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_dist);
    println!("cargo::rerun-if-changed={}", dist.display());
    let collected = assets::collect(&dist).unwrap_or_else(|error| {
        panic!("the embedded-web feature cannot bundle the built web app: {error}. Run `moon run quadratic-web:build`")
    });
    let entries: String = collected
        .iter()
        .map(|asset| {
            let file = asset.file.to_str().expect("a UTF-8 path");
            format!(
                "    EmbeddedFile {{ relative_path: {:?}, media_type: {:?}, bytes: include_bytes!({file:?}) }},\n",
                asset.relative_path, asset.media_type
            )
        })
        .collect();
    let source = format!("pub static FILES: &[EmbeddedFile] = &[\n{entries}];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets it"));
    std::fs::write(out.join("web_app.rs"), source).expect("web_app.rs is written");
}

/// The web app of the source tree. A crate packaged for crates.io has no source tree around it, so it carries a copy
/// of the built web app in `web-dist`, which `cargo package` takes in.
fn default_dist() -> PathBuf {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let source_tree = manifest.join("../web/dist");
    let packaged = manifest.join(PACKAGED_DIST);
    println!("cargo::rerun-if-changed={}", packaged.display());
    if !source_tree.exists() && packaged.join("index.html").is_file() {
        packaged
    } else {
        source_tree
    }
}
