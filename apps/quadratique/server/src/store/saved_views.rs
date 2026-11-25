//! The human's saved views: named filters of the web app, in their order, kept with the issues so
//! that they follow the person across ports and browsers.

use super::engine::{Cells, execute, text_param};
use super::error::StoreError;
use super::tx::Tx;
use super::view::View;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedViewRecord {
    pub id: String,
    pub name: String,
    pub project: Option<String>,
    pub filter: String,
}

/// Every saved view, and the revision of that list: it grows with each commit that changes the
/// list, so a page keeps the newest list it was given whatever order the answers come in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedViewList {
    pub revision: i64,
    pub views: Vec<SavedViewRecord>,
}

impl View<'_> {
    /// The saved views, in their order, with their revision.
    pub fn saved_views(&self) -> Result<SavedViewList, StoreError> {
        let sql = format!(
            "SELECT id, name, project, filter FROM {} ORDER BY position",
            self.table("saved_views")
        );
        let views = self
            .rows(&sql, &[])?
            .iter()
            .map(|row| {
                let cells = Cells(row);
                SavedViewRecord {
                    id: cells.text(0),
                    name: cells.text(1),
                    project: cells.opt_text(2),
                    filter: cells.text(3),
                }
            })
            .collect();
        Ok(SavedViewList {
            revision: self.saved_view_state()?.1,
            views,
        })
    }

    /// The view that an earlier import made from the old view `old` of the browser `source`.
    pub fn saved_view_import(&self, source: &str, old: &str) -> Result<Option<String>, StoreError> {
        let sql = format!(
            "SELECT view FROM {} WHERE source = ? AND old = ?",
            self.table("saved_view_imports")
        );
        Ok(self
            .row(&sql, &[source.into(), old.into()])?
            .map(|row| Cells(&row).text(0)))
    }

    /// The next view id's number and the list's revision.
    fn saved_view_state(&self) -> Result<(i64, i64), StoreError> {
        let sql = format!(
            "SELECT next_id, revision FROM {}",
            self.table("saved_view_state")
        );
        let row = self
            .row(&sql, &[])?
            .ok_or_else(|| StoreError::Engine("saved_view_state has no row".into()))?;
        let cells = Cells(&row);
        Ok((cells.int(0), cells.int(1)))
    }
}

impl Tx<'_> {
    /// A saved view id that no view of this store had: ids are never reused, so a page that shows
    /// a deleted view cannot rename or delete a newer one.
    pub fn new_saved_view_id(&mut self) -> Result<String, StoreError> {
        let (next, _) = self.saved_view_state()?;
        execute(
            self.connection,
            "UPDATE saved_view_state SET next_id = ?",
            &[(next + 1).into()],
        )?;
        Ok(format!("v{next}"))
    }

    /// Replaces the saved views with `views`, in that order, and raises the revision by one. A list
    /// equal to the current one changes nothing.
    pub fn replace_saved_views(&mut self, views: &[SavedViewRecord]) -> Result<(), StoreError> {
        if self.saved_views()?.views == views {
            return Ok(());
        }
        execute(self.connection, "DELETE FROM saved_views", &[])?;
        for (position, view) in views.iter().enumerate() {
            execute(
                self.connection,
                "INSERT INTO saved_views (id, name, project, filter, position) VALUES (?, ?, ?, ?, ?)",
                &[
                    view.id.as_str().into(),
                    view.name.as_str().into(),
                    text_param(view.project.as_deref()),
                    view.filter.as_str().into(),
                    (position as i64).into(),
                ],
            )?;
        }
        execute(
            self.connection,
            "UPDATE saved_view_state SET revision = revision + 1",
            &[],
        )?;
        Ok(())
    }

    /// Records that the old view `old` of the browser `source` became the view `view`.
    pub fn record_saved_view_import(
        &mut self,
        source: &str,
        old: &str,
        view: &str,
    ) -> Result<(), StoreError> {
        execute(
            self.connection,
            "INSERT INTO saved_view_imports (source, old, view) VALUES (?, ?, ?)",
            &[source.into(), old.into(), view.into()],
        )?;
        Ok(())
    }
}
