//! What a store records, read without a server, without writing and without migrating: its
//! schema version and the repositories of its projects. `quadratique upgrade` reads it before it
//! replaces the binary, and for its report of the repositories.

use std::path::{Path, PathBuf};

use super::engine::{Cells, open_reader, query, refuse_git_work_tree};
use super::error::StoreError;
use super::migrate::{MIGRATIONS, schema_version};

/// The schema version this build migrates a store to.
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

pub struct Recorded {
    pub schema: i64,
    /// Empty when the store's schema is newer than this build knows: its tables may differ.
    pub repositories: Vec<PathBuf>,
}

/// `None` when no store is at `path`. Creates no folder and no file.
pub fn read_recorded(path: &Path) -> Result<Option<Recorded>, StoreError> {
    if let Some(folder) = path.parent() {
        refuse_git_work_tree(folder)?;
    }
    if !path.is_file() {
        return Ok(None);
    }
    let connection = open_reader(path)?;
    let schema = schema_version(&connection)?;
    let repositories = if schema <= SCHEMA_VERSION {
        query(
            &connection,
            "SELECT repository FROM projects WHERE repository IS NOT NULL ORDER BY key",
            &[],
        )?
        .iter()
        .filter_map(|row| Cells(row).opt_text(0).map(PathBuf::from))
        .collect()
    } else {
        Vec::new()
    };
    Ok(Some(Recorded {
        schema,
        repositories,
    }))
}
