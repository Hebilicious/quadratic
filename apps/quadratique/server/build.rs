use std::path::PathBuf;

#[path = "src/assets.rs"]
mod assets;

const DIST_VARIABLE: &str = "QUADRATIQUE_WEB_DIST";
const PACKAGED_DIST: &str = "web-dist";
/// A file of this crate's folder in the repository that `include` keeps out of the package: only the repository has it.
const REPOSITORY_MARKER: &str = "moon.yml";

/// In the repository, the built web app is embedded only with the feature `embedded-web`, as the release build asks:
/// `QUADRATIQUE_WEB_DIST`, else `../web/dist`. Any other build there embeds nothing and reads `../web/dist` at each
/// request. A packaged crate, as crates.io serves it, always embeds the copy it carries in `web-dist`, with or without
/// the feature and whatever `QUADRATIQUE_WEB_DIST` says, and fails to build when that copy is missing.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src/assets.rs");
    println!("cargo::rustc-check-cfg=cfg(embedded_web)");
    // doltlite reads the system's certificate store on Windows and does not link the library that holds it.
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        println!("cargo::rustc-link-lib=crypt32");
    }
    println!("cargo::rerun-if-env-changed={DIST_VARIABLE}");
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let feature = std::env::var_os("CARGO_FEATURE_EMBEDDED_WEB").is_some();
    let dist = if manifest.join(REPOSITORY_MARKER).is_file() {
        if !feature {
            return;
        }
        std::env::var_os(DIST_VARIABLE)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| manifest.join("../web/dist"))
    } else {
        let packaged = manifest.join(PACKAGED_DIST);
        println!("cargo::rerun-if-changed={}", packaged.display());
        if !packaged.join("index.html").is_file() {
            panic!(
                "this quadratique-server crate was packaged without its web app: {} has no index.html. Run `moon run quadratique-server:stage-web` before `cargo package`",
                packaged.display()
            );
        }
        packaged
    };
    println!("cargo::rustc-cfg=embedded_web");
    println!("cargo::rerun-if-changed={}", dist.display());
    let collected = assets::collect(&dist).unwrap_or_else(|error| {
        panic!("the embedded-web feature cannot bundle the built web app: {error}. Run `moon run quadratique-web:build`")
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
