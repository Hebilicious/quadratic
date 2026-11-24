use std::path::{Path, PathBuf};

pub struct Asset {
    pub relative_path: String,
    pub media_type: &'static str,
    pub file: PathBuf,
}

#[derive(Debug)]
pub enum AssetError {
    Unreadable(PathBuf, std::io::Error),
    LinksOutside { link: PathBuf, root: PathBuf },
    NotAFile(PathBuf),
    NotUtf8(PathBuf),
    NoIndex(PathBuf),
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetError::Unreadable(path, error) => write!(formatter, "{}: {error}", path.display()),
            AssetError::LinksOutside { link, root } => {
                write!(
                    formatter,
                    "{} links outside {}",
                    link.display(),
                    root.display()
                )
            }
            AssetError::NotAFile(path) => write!(formatter, "{} is not a file", path.display()),
            AssetError::NotUtf8(path) => {
                write!(formatter, "{} is not a UTF-8 name", path.display())
            }
            AssetError::NoIndex(root) => write!(formatter, "{} has no index.html", root.display()),
        }
    }
}

pub fn collect(root: &Path) -> Result<Vec<Asset>, AssetError> {
    let root = root
        .canonicalize()
        .map_err(|error| AssetError::Unreadable(root.to_path_buf(), error))?;
    let mut assets = Vec::new();
    walk(&root, &root, &mut assets)?;
    assets.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    if !assets
        .iter()
        .any(|asset| asset.relative_path == "index.html")
    {
        return Err(AssetError::NoIndex(root));
    }
    Ok(assets)
}

fn walk(root: &Path, directory: &Path, assets: &mut Vec<Asset>) -> Result<(), AssetError> {
    let unreadable = |path: &Path| {
        let path = path.to_path_buf();
        move |error| AssetError::Unreadable(path, error)
    };
    let entries = std::fs::read_dir(directory).map_err(unreadable(directory))?;
    for entry in entries {
        let entry = entry.map_err(unreadable(directory))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(unreadable(&path))?;
        if kind.is_dir() {
            walk(root, &path, assets)?;
            continue;
        }
        let file = path.canonicalize().map_err(unreadable(&path))?;
        if !file.starts_with(root) {
            return Err(AssetError::LinksOutside {
                link: path,
                root: root.to_path_buf(),
            });
        }
        if !file.is_file() {
            return Err(AssetError::NotAFile(path));
        }
        let parts: Vec<&str> = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .components()
            .map(|part| {
                part.as_os_str()
                    .to_str()
                    .ok_or_else(|| AssetError::NotUtf8(path.clone()))
            })
            .collect::<Result<_, _>>()?;
        assets.push(Asset {
            relative_path: parts.join("/"),
            media_type: media_type(&file),
            file,
        });
    }
    Ok(())
}

pub fn media_type(file: &Path) -> &'static str {
    match file.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::{AssetError, collect};

    fn built() -> tempfile::TempDir {
        let folder = tempfile::tempdir().expect("a folder of assets");
        std::fs::write(folder.path().join("index.html"), "<title>Quadratic</title>")
            .expect("an index page");
        folder
    }

    #[test]
    fn a_link_inside_the_assets_is_bundled_under_its_name_with_the_type_of_the_file_it_names() {
        let assets = built();
        symlink("index.html", assets.path().join("alias.js")).expect("a link to the index page");

        let collected = collect(assets.path()).expect("the assets");

        let alias = collected
            .iter()
            .find(|asset| asset.relative_path == "alias.js")
            .expect("alias.js is bundled");
        assert_eq!(alias.media_type, "text/html; charset=utf-8");
    }

    #[test]
    fn a_link_that_leaves_the_assets_is_refused() {
        let assets = built();
        let outside = tempfile::tempdir().expect("a folder outside the assets");
        std::fs::write(outside.path().join("secret.txt"), "a secret").expect("a file outside");
        symlink(
            outside.path().join("secret.txt"),
            assets.path().join("secret.js"),
        )
        .expect("a link out of the assets");

        let refused = collect(assets.path()).err().expect("the link is refused");

        assert!(
            matches!(&refused, AssetError::LinksOutside { link, .. } if link.ends_with("secret.js")),
            "{refused}"
        );
    }
}
