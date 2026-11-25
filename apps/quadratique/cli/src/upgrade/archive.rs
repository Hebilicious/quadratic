//! Takes the binary out of a release archive, in memory. Nothing of the archive is written to
//! disk, so no entry and no link in it can reach a file outside the folder of the binary. An
//! archive with an entry that names an absolute path or `..`, or with a link that does, is
//! refused as a whole.

use std::io::Read;
use std::path::{Component, Path};

pub fn binary(archive: &[u8], wanted: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut entries = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut found = None;
    for entry in entries
        .entries()
        .map_err(|error| format!("the archive cannot be read: {error}"))?
    {
        let mut entry = entry.map_err(|error| format!("the archive cannot be read: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("an archive entry has an unreadable path: {error}"))?
            .into_owned();
        let name = inside(&path)?;
        if let Some(target) = entry
            .link_name()
            .map_err(|error| format!("{name}: an unreadable link: {error}"))?
        {
            inside(&target).map_err(|refusal| format!("{name} links outside: {refusal}"))?;
        }
        if name != wanted {
            continue;
        }
        if found.is_some() {
            return Err(format!("the archive holds {wanted} twice"));
        }
        if !entry.header().entry_type().is_file() {
            return Err(format!("{wanted} in the archive is not a regular file"));
        }
        if entry.size() > limit {
            return Err(format!(
                "{wanted} in the archive is larger than {limit} bytes"
            ));
        }
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(limit)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("{wanted} cannot be read from the archive: {error}"))?;
        found = Some(bytes);
    }
    found.ok_or_else(|| format!("the archive holds no {wanted}"))
}

/// The entry's path with `/` between its parts, when it stays inside the archive's folder.
fn inside(path: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| format!("{} is not UTF-8", path.display()))?,
            ),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{} is an absolute path", path.display()));
            }
            Component::ParentDir => return Err(format!("{} contains ..", path.display())),
        }
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
pub mod tests {
    use super::binary;

    /// A tar.gz archive written header by header, so that it can hold the entries a careful
    /// archiver refuses to write.
    pub fn archive(entries: &[(&str, tar::EntryType, &[u8], Option<&str>)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, kind, bytes, link) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            let name = &mut header.as_old_mut().name;
            name[..path.len()].copy_from_slice(path.as_bytes());
            if let Some(link) = link {
                let linkname = &mut header.as_old_mut().linkname;
                linkname[..link.len()].copy_from_slice(link.as_bytes());
            }
            header.set_cksum();
            builder.append(&header, *bytes).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    const WANTED: &str = "quadratique-1.0.0-t/bin/quadratique";

    #[test]
    fn the_binary_is_taken_from_its_place_in_the_archive() {
        let bytes = archive(&[
            ("quadratique-1.0.0-t/", tar::EntryType::Directory, b"", None),
            (WANTED, tar::EntryType::Regular, b"the binary", None),
            (
                "quadratique-1.0.0-t/install.sh",
                tar::EntryType::Regular,
                b"#!/bin/sh",
                None,
            ),
        ]);
        assert_eq!(binary(&bytes, WANTED, 1 << 20).unwrap(), b"the binary");
    }

    #[test]
    fn an_archive_with_an_entry_or_a_link_outside_its_folder_is_refused() {
        for (entries, refusal) in [
            (
                vec![("/etc/passwd", tar::EntryType::Regular, &b"x"[..], None)],
                "is an absolute path",
            ),
            (
                vec![("../quadratique", tar::EntryType::Regular, &b"x"[..], None)],
                "contains ..",
            ),
            (
                vec![(
                    "quadratique-1.0.0-t/bin/escape",
                    tar::EntryType::Symlink,
                    &b""[..],
                    Some("../../../../.bashrc"),
                )],
                "links outside",
            ),
            (
                vec![(WANTED, tar::EntryType::Symlink, &b""[..], Some("other"))],
                "is not a regular file",
            ),
        ] {
            let mut all = entries.clone();
            if entries[0].0 != WANTED {
                all.push((WANTED, tar::EntryType::Regular, b"the binary", None));
            }
            let error = binary(&archive(&all), WANTED, 1 << 20).unwrap_err();
            assert!(error.contains(refusal), "{error}");
        }
    }

    #[test]
    fn an_archive_without_the_binary_or_with_a_binary_over_the_limit_is_refused() {
        let empty = archive(&[("README.md", tar::EntryType::Regular, b"x", None)]);
        assert!(binary(&empty, WANTED, 10).unwrap_err().contains("holds no"));
        let large = archive(&[(WANTED, tar::EntryType::Regular, b"0123456789a", None)]);
        assert!(binary(&large, WANTED, 10).unwrap_err().contains("larger"));
        assert!(binary(b"not gzip", WANTED, 10).is_err());
    }
}
