use super::engine::{Cells, SCHEMA, query_one};
use super::error::StoreError;
use doltlite::Connection;

#[derive(Clone, Copy)]
pub(crate) struct Migration {
    pub(crate) name: &'static str,
    pub(crate) sql: &'static str,
}

// The schema version of a store is `PRAGMA user_version`, which doltlite keeps in each commit.
pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        name: "the first schema",
        sql: SCHEMA,
    },
    Migration {
        name: "saved views",
        sql: include_str!("saved_views.sql"),
    },
];

pub(crate) enum Version {
    Current,
    Behind(usize),
    Ahead(i64),
}

pub(crate) fn numbered(migrations: &[Migration]) -> impl Iterator<Item = (usize, &Migration)> {
    migrations
        .iter()
        .enumerate()
        .map(|(index, migration)| (index + 1, migration))
}

pub(crate) fn compare(found: i64, migrations: &[Migration]) -> Version {
    match usize::try_from(found) {
        Ok(found) if found == migrations.len() => Version::Current,
        Ok(found) if found < migrations.len() => Version::Behind(found),
        _ => Version::Ahead(found),
    }
}

pub(crate) fn schema_version(connection: &Connection) -> Result<i64, StoreError> {
    let row = query_one(connection, "PRAGMA user_version", &[])?
        .ok_or_else(|| StoreError::Engine("PRAGMA user_version returned no row".into()))?;
    Ok(Cells(&row).int(0))
}
