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

impl View<'_> {
    /// The saved views, in their order.
    pub fn saved_views(&self) -> Result<Vec<SavedViewRecord>, StoreError> {
        let sql = format!(
            "SELECT id, name, project, filter FROM {} ORDER BY position",
            self.table("saved_views")
        );
        Ok(self
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
            .collect())
    }
}

impl Tx<'_> {
    /// A saved view id that no view of this store had: ids are never reused, so a page that shows
    /// a deleted view cannot rename or delete a newer one.
    pub fn new_saved_view_id(&mut self) -> Result<String, StoreError> {
        let row = self
            .row("SELECT next FROM saved_view_numbers", &[])?
            .ok_or_else(|| StoreError::Engine("saved_view_numbers has no row".into()))?;
        let next = Cells(&row).int(0);
        execute(
            self.connection,
            "UPDATE saved_view_numbers SET next = ?",
            &[(next + 1).into()],
        )?;
        Ok(format!("v{next}"))
    }

    /// Replaces the saved views with `views`, in that order. Rows that stay the same are no change
    /// in the commit.
    pub fn replace_saved_views(&mut self, views: &[SavedViewRecord]) -> Result<(), StoreError> {
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
        Ok(())
    }
}
