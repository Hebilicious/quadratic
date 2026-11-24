use std::path::PathBuf;

#[path = "src/assets.rs"]
mod assets;

const DIST_VARIABLE: &str = "QUADRATIQUE_WEB_DIST";
const PACKAGED_DIST: &str = "web-dist";

/// Embeds the built web app in two cases. With the feature `embedded-web`, as the release build does, it embeds
/// `QUADRATIQUE_WEB_DIST`, else `../web/dist`. Without it, a crate packaged for crates.io, which has no source tree
/// around it but carries a copy of the web app in `web-dist`, embeds that copy. A build in the source tree without the
/// feature embeds nothing: it reads `../web/dist` at each request.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src/assets.rs");
    println!("cargo::rustc-check-cfg=cfg(embedded_web)");
    println!("cargo::rerun-if-env-changed={DIST_VARIABLE}");
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let source_tree = manifest.join("../web");
    let packaged = manifest.join(PACKAGED_DIST);
    if !source_tree.exists() {
        println!("cargo::rerun-if-changed={}", packaged.display());
    }
    let dist = if std::env::var_os("CARGO_FEATURE_EMBEDDED_WEB").is_some() {
        std::env::var_os(DIST_VARIABLE)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| source_tree.join("dist"))
    } else if !source_tree.exists() && packaged.join("index.html").is_file() {
        packaged
    } else {
        return;
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
