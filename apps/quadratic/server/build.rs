use std::path::{Path, PathBuf};

#[path = "src/assets.rs"]
mod assets;

const DIST_VARIABLE: &str = "QUADRATIC_WEB_DIST";

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
        .unwrap_or_else(|| {
            Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"))
                .join("../web/dist")
        });
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
