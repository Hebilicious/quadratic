use super::error::StoreError;
use doltlite::{Connection, OpenFlags, Row, Value};
use std::path::{Path, PathBuf};

pub(crate) const SCHEMA: &str = include_str!("schema.sql");

const SQLITE_BUSY: i32 = 5;
const SQLITE_LOCKED: i32 = 6;
const SQLITE_CORRUPT: i32 = 11;
const SQLITE_NOTADB: i32 = 26;

const WRITER_BUSY_TIMEOUT_MS: i32 = 5_000;

// A doltlite reader whose snapshot is behind takes the shared lock to refresh, so it waits while the
// writer publishes.
const READER_BUSY_TIMEOUT_MS: i32 = 60_000;
const _: () = assert!(READER_BUSY_TIMEOUT_MS > WRITER_BUSY_TIMEOUT_MS);

pub(crate) fn database_path(path: &Path) -> Result<PathBuf, StoreError> {
    let file_name = path.file_name().ok_or_else(|| {
        StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} names no file", path.display()),
        ))
    })?;
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    refuse_git_work_tree(directory)?;
    std::fs::create_dir_all(directory)?;
    Ok(directory.canonicalize()?.join(file_name))
}

pub fn refuse_git_work_tree(directory: &Path) -> Result<(), StoreError> {
    let existing = directory
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .unwrap_or(Path::new("."))
        .canonicalize()?;
    match existing
        .ancestors()
        .find(|ancestor| holds_git(&ancestor.join(".git")))
    {
        Some(work_tree) => Err(StoreError::InsideGitWorkTree(work_tree.to_path_buf())),
        None => Ok(()),
    }
}

/// A sandbox may mount an empty `.git` folder to keep commands from writing one; git does not take it for a
/// repository, so neither does Quadratique.
fn holds_git(dot_git: &Path) -> bool {
    dot_git.is_file() || dot_git.join("HEAD").symlink_metadata().is_ok()
}

pub(crate) fn open_writer(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(WRITER_BUSY_TIMEOUT_MS)?;
    check_store(path, &connection)?;
    Ok(connection)
}

pub(crate) fn open_reader(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open_with_flags(path, OpenFlags::READ_ONLY)?;
    connection.busy_timeout(READER_BUSY_TIMEOUT_MS)?;
    check_store(path, &connection)?;
    Ok(connection)
}

fn check_store(path: &Path, connection: &Connection) -> Result<(), StoreError> {
    connection
        .query_row("SELECT dolt_version()", &[])
        .map_err(|error| engine_refusal(path, error))?;
    connection
        .query_row("SELECT dolt_hashof('HEAD')", &[])
        .map_err(|error| match error.code {
            SQLITE_BUSY | SQLITE_LOCKED => StoreError::from(error),
            _ => damaged(path, error),
        })?;
    Ok(())
}

fn engine_refusal(path: &Path, error: doltlite::Error) -> StoreError {
    match error.code {
        SQLITE_CORRUPT | SQLITE_NOTADB => damaged(path, error),
        _ => StoreError::NotDoltlite(error.to_string()),
    }
}

fn damaged(path: &Path, error: doltlite::Error) -> StoreError {
    StoreError::Damaged {
        path: path.to_path_buf(),
        reason: error.to_string(),
    }
}

pub(crate) fn query(
    connection: &Connection,
    sql: &str,
    params: &[Value],
) -> Result<Vec<Row>, StoreError> {
    Ok(connection.query(sql, params)?)
}

pub(crate) fn query_one(
    connection: &Connection,
    sql: &str,
    params: &[Value],
) -> Result<Option<Row>, StoreError> {
    Ok(connection.query_row(sql, params)?)
}

pub(crate) fn execute(
    connection: &Connection,
    sql: &str,
    params: &[Value],
) -> Result<usize, StoreError> {
    Ok(connection.execute(sql, params)?)
}

pub(crate) fn text_param(value: Option<&str>) -> Value {
    value.map_or(Value::Null, Value::from)
}

pub(crate) struct Cells<'r>(pub &'r Row);

impl Cells<'_> {
    pub(crate) fn opt_text(&self, index: usize) -> Option<String> {
        match self.0.get(index)? {
            Value::Text(text) => Some(text.clone()),
            Value::Integer(number) => Some(number.to_string()),
            Value::Real(number) => Some(number.to_string()),
            Value::Null | Value::Blob(_) => None,
        }
    }

    pub(crate) fn text(&self, index: usize) -> String {
        self.opt_text(index).unwrap_or_default()
    }

    pub(crate) fn opt_int(&self, index: usize) -> Option<i64> {
        self.0.get(index)?.as_i64()
    }

    pub(crate) fn int(&self, index: usize) -> i64 {
        self.opt_int(index).unwrap_or_default()
    }

    pub(crate) fn opt_real(&self, index: usize) -> Option<f64> {
        self.0.get(index)?.as_f64()
    }

    pub(crate) fn real(&self, index: usize) -> f64 {
        self.opt_real(index).unwrap_or_default()
    }

    pub(crate) fn value(&self, index: usize) -> Value {
        self.0.get(index).cloned().unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(code: i32, message: &str) -> doltlite::Error {
        doltlite::Error {
            code,
            message: message.into(),
        }
    }

    #[test]
    fn a_build_whose_sqlite_lacks_dolt_version_is_told_apart_from_a_damaged_file() {
        let path = Path::new("/data/workspace.db");

        let plain = engine_refusal(path, failure(1, "no such function: dolt_version"));
        let malformed = engine_refusal(
            path,
            failure(SQLITE_CORRUPT, "database disk image is malformed"),
        );

        assert!(matches!(plain, StoreError::NotDoltlite(_)), "{plain:?}");
        assert!(
            matches!(malformed, StoreError::Damaged { .. }),
            "{malformed:?}"
        );
    }
}
