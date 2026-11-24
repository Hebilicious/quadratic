use super::engine::Cells;
use super::error::StoreError;
use super::json;
use super::message::Message;
use super::types::{Change, FieldChange, Page, Subject};
use super::view::View;
use doltlite::{Row, Value};
use serde_json::json;
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
enum Shape {
    Text,
    Number,
    Json,
}

const ISSUE_FIELDS: &[(&str, &str, Shape)] = &[
    ("project", "project", Shape::Text),
    ("title", "title", Shape::Text),
    ("status", "status", Shape::Text),
    ("priority", "priority", Shape::Number),
    ("assignee", "assignee", Shape::Json),
    ("parent", "parent", Shape::Text),
    ("milestone", "milestone", Shape::Text),
    ("sort_order", "order", Shape::Number),
    ("due", "due", Shape::Text),
    ("description", "description", Shape::Text),
    ("acceptance", "acceptance", Shape::Text),
    ("notes", "notes", Shape::Text),
    ("evidence", "evidence", Shape::Text),
    ("revision", "revision", Shape::Number),
    ("created_by", "createdBy", Shape::Json),
    ("created_at", "createdAt", Shape::Text),
    ("updated_at", "updatedAt", Shape::Text),
];

const CLAIM_COLUMNS: [&str; 4] = [
    "claim_holder",
    "claim_branch",
    "claim_worktree",
    "claim_since",
];

const CLAIM_OPERATION: &str = "issue.claim";
const LANDING_OPERATION: &str = "git.merged";

const PROJECT_FIELDS: &[(&str, &str, Shape)] = &[
    ("name", "name", Shape::Text),
    ("repository", "repository", Shape::Text),
    ("default_branch", "defaultBranch", Shape::Text),
    ("extra", "extra", Shape::Json),
];

struct Entry {
    seq: i64,
    project: String,
    issue: String,
    hash: String,
    parent: String,
    message: Message,
}

impl View<'_> {
    pub fn history(
        &self,
        subject: &Subject,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page<Change>, StoreError> {
        let mut conditions = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        match subject {
            Subject::Board => {}
            Subject::Project(key) => {
                conditions.push("s.project = ?");
                params.push(key.as_str().into());
            }
            Subject::Issue(id) => {
                conditions.push("s.issue = ?");
                params.push(id.as_str().into());
            }
        }
        if let Some(bound) = self.bound() {
            conditions.push("s.seq <= ?");
            params.push(bound.into());
        }
        if let Some(cursor) = cursor {
            let (seq, project, issue) = parse_history_cursor(cursor)?;
            conditions.push(
                "(s.seq < ? OR (s.seq = ? AND (s.project > ? OR (s.project = ? AND s.issue > ?))))",
            );
            params.extend([
                seq.into(),
                seq.into(),
                project.as_str().into(),
                project.as_str().into(),
                issue.as_str().into(),
            ]);
        }
        let condition = if conditions.is_empty() {
            "1".to_owned()
        } else {
            conditions.join(" AND ")
        };
        params.push(Value::from(limit as i64 + 1));
        let sql = format!(
            "SELECT s.seq, s.project, s.issue, c.hash, c.parent, c.message \
             FROM commit_subjects s JOIN commit_index c ON c.seq = s.seq \
             WHERE {condition} ORDER BY s.seq DESC, s.project, s.issue LIMIT ?"
        );
        let mut entries = self
            .rows(&sql, &params)?
            .iter()
            .map(entry_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if entries.len() > limit {
            entries.truncate(limit);
            entries
                .last()
                .map(|entry| format!("h:{}:{}:{}", entry.seq, entry.project, entry.issue))
        } else {
            None
        };
        let items = entries
            .into_iter()
            .map(|entry| self.change(entry))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Page { items, next_cursor })
    }

    /// The branch of the last claim on `issue` and the position of that claim's commit, in this
    /// view; it stays after the claim ends.
    pub fn last_claim(&self, issue: &str) -> Result<Option<(String, i64)>, StoreError> {
        let Some((seq, hash)) = self.last_issue_commit(issue, CLAIM_OPERATION)? else {
            return Ok(None);
        };
        Ok(self
            .row(
                &format!("SELECT claim_branch FROM dolt_at_issues('{hash}') WHERE id = ?"),
                &[issue.into()],
            )?
            .and_then(|row| Cells(&row).opt_text(0))
            .map(|branch| (branch, seq)))
    }

    /// The position of the last commit in which the ref watcher recorded a landing of `issue`.
    pub fn last_landing(&self, issue: &str) -> Result<Option<i64>, StoreError> {
        Ok(self
            .last_issue_commit(issue, LANDING_OPERATION)?
            .map(|(seq, _)| seq))
    }

    fn last_issue_commit(
        &self,
        issue: &str,
        operation: &str,
    ) -> Result<Option<(i64, String)>, StoreError> {
        let first_line = format!("{operation}\n\n");
        let mut sql = "SELECT c.seq, c.hash FROM commit_subjects s \
                       JOIN commit_index c ON c.seq = s.seq \
                       WHERE s.issue = ? AND substr(c.message, 1, ?) = ?"
            .to_owned();
        let mut params: Vec<Value> = vec![
            issue.into(),
            (first_line.len() as i64).into(),
            first_line.as_str().into(),
        ];
        if let Some(bound) = self.bound() {
            sql.push_str(" AND s.seq <= ?");
            params.push(bound.into());
        }
        sql.push_str(" ORDER BY s.seq DESC LIMIT 1");
        Ok(self.row(&sql, &params)?.map(|row| {
            let cells = Cells(&row);
            (cells.int(0), cells.text(1))
        }))
    }

    /// The position of the first commit since which `project` has been bound to the repository it
    /// is bound to in this view; `None` when it is bound to none. Reads one project commit more
    /// than the project has had since that binding.
    pub fn bound_since(&self, project: &str) -> Result<Option<i64>, StoreError> {
        let mut sql = "SELECT c.seq, c.hash FROM commit_subjects s \
                       JOIN commit_index c ON c.seq = s.seq \
                       WHERE s.project = ? AND s.issue = ''"
            .to_owned();
        let mut params: Vec<Value> = vec![project.into()];
        if let Some(bound) = self.bound() {
            sql.push_str(" AND s.seq <= ?");
            params.push(bound.into());
        }
        sql.push_str(" ORDER BY s.seq DESC");
        let mut bound: Option<String> = None;
        let mut since = None;
        for row in self.rows(&sql, &params)? {
            let cells = Cells(&row);
            let repository = self
                .row(
                    &format!(
                        "SELECT repository FROM dolt_at_projects('{}') WHERE key = ?",
                        cells.text(1)
                    ),
                    &[project.into()],
                )?
                .and_then(|row| Cells(&row).opt_text(0));
            let Some(repository) = repository else {
                break;
            };
            match &bound {
                None => bound = Some(repository),
                Some(current) if *current != repository => break,
                Some(_) => {}
            }
            since = Some(cells.int(0));
        }
        Ok(since)
    }

    fn change(&self, entry: Entry) -> Result<Change, StoreError> {
        let fields = if entry.issue.is_empty() {
            self.project_fields(&entry.parent, &entry.hash, &entry.project)?
        } else {
            self.issue_fields(&entry.parent, &entry.hash, &entry.issue)?
        };
        Ok(Change {
            commit: entry.hash,
            at: entry.message.at,
            author: entry.message.actor,
            operation: entry.message.operation,
            reason: entry.message.reason,
            branch: entry.message.branch,
            worktree: entry.message.worktree,
            project: entry.project,
            issue: (!entry.issue.is_empty()).then_some(entry.issue),
            fields,
        })
    }

    fn issue_fields(
        &self,
        parent: &str,
        hash: &str,
        id: &str,
    ) -> Result<Vec<FieldChange>, StoreError> {
        let refs = |id: &str| -> Vec<Value> { vec![parent.into(), hash.into(), id.into()] };
        let mut columns: Vec<&str> = ISSUE_FIELDS.iter().map(|(column, _, _)| *column).collect();
        columns.extend(CLAIM_COLUMNS);
        let selected: Vec<String> = columns
            .iter()
            .flat_map(|column| [format!("from_{column}"), format!("to_{column}")])
            .collect();
        let sql = format!(
            "SELECT diff_type, {} FROM dolt_diff_issues(?1, ?2) WHERE to_id = ?3 OR from_id = ?3",
            selected.join(", ")
        );
        let mut fields = Vec::new();
        let mut existed_before = true;
        if let Some(row) = self.row(&sql, &refs(id))? {
            let cells = Cells(&row);
            existed_before = cells.text(0) != "added";
            for (index, (_, field, shape)) in ISSUE_FIELDS.iter().enumerate() {
                let before = render(&cells.value(1 + 2 * index), *shape);
                let after = render(&cells.value(2 + 2 * index), *shape);
                if before != after {
                    fields.push(FieldChange {
                        field: (*field).to_owned(),
                        before,
                        after,
                    });
                }
            }
            let claim_at = 1 + 2 * ISSUE_FIELDS.len();
            let claim = |offset: usize| {
                let value = |column: usize| cells.opt_text(claim_at + 2 * column + offset);
                value(0).map(|holder| {
                    let holder: serde_json::Value =
                        serde_json::from_str(&holder).unwrap_or(serde_json::Value::Null);
                    json::text(&json!({
                        "holder": holder,
                        "branch": value(1),
                        "worktree": value(2),
                        "since": value(3),
                    }))
                })
            };
            let (before, after) = (claim(0), claim(1));
            if before != after {
                fields.push(FieldChange {
                    field: "claim".into(),
                    before,
                    after,
                });
            }
        }
        fields.extend(self.label_change(parent, hash, id, existed_before)?);
        fields.extend(self.notes_added(parent, hash, id)?);
        fields.extend(self.source_changes(parent, hash, id)?);
        fields.extend(self.links_change(parent, hash, id)?);
        fields.extend(self.relation_changes(parent, hash, id)?);
        Ok(fields)
    }

    fn label_change(
        &self,
        parent: &str,
        hash: &str,
        id: &str,
        existed_before: bool,
    ) -> Result<Option<FieldChange>, StoreError> {
        let rows = self.rows(
            "SELECT from_label, to_label FROM dolt_diff_labels(?1, ?2) \
             WHERE to_issue = ?3 OR from_issue = ?3",
            &[parent.into(), hash.into(), id.into()],
        )?;
        if rows.is_empty() {
            return Ok(None);
        }
        let after: BTreeSet<String> = self
            .rows(
                &format!("SELECT label FROM dolt_at_labels('{hash}') WHERE issue = ?"),
                &[id.into()],
            )?
            .iter()
            .map(|row| Cells(row).text(0))
            .collect();
        let mut before = after.clone();
        for row in &rows {
            let cells = Cells(row);
            if let Some(added) = cells.opt_text(1) {
                before.remove(&added);
            }
            if let Some(removed) = cells.opt_text(0) {
                before.insert(removed);
            }
        }
        let list = |labels: BTreeSet<String>| {
            serde_json::Value::from(labels.into_iter().collect::<Vec<_>>()).to_string()
        };
        Ok(Some(FieldChange {
            field: "labels".into(),
            before: existed_before.then(|| list(before)),
            after: Some(list(after)),
        }))
    }

    fn notes_added(
        &self,
        parent: &str,
        hash: &str,
        id: &str,
    ) -> Result<Vec<FieldChange>, StoreError> {
        let rows = match self.rows(
            "SELECT to_text FROM dolt_diff_notes(?1, ?2) \
             WHERE to_issue = ?3 AND diff_type = 'added' ORDER BY to_number",
            &[parent.into(), hash.into(), id.into()],
        ) {
            Err(error) if error.is_missing_table("notes") => return Ok(Vec::new()),
            rows => rows?,
        };
        Ok(rows
            .iter()
            .map(|row| FieldChange {
                field: "notes".into(),
                before: None,
                after: Some(json::string(&Cells(row).text(0))),
            })
            .collect())
    }

    fn source_changes(
        &self,
        parent: &str,
        hash: &str,
        id: &str,
    ) -> Result<Vec<FieldChange>, StoreError> {
        let rows = match self.rows(
            "SELECT diff_type, to_tool, to_key, from_decision, to_decision \
             FROM dolt_diff_sources(?1, ?2) WHERE to_issue = ?3",
            &[parent.into(), hash.into(), id.into()],
        ) {
            Err(error) if error.is_missing_table("sources") => return Ok(Vec::new()),
            rows => rows?,
        };
        let decision = |value: Option<i64>| value.map(|value| (value != 0).to_string());
        let mut fields = Vec::new();
        for row in &rows {
            let cells = Cells(row);
            if cells.text(0) == "added" {
                fields.push(FieldChange {
                    field: "source".into(),
                    before: None,
                    after: Some(json::text(
                        &json!({ "tool": cells.text(1), "key": cells.text(2) }),
                    )),
                });
            }
            let (before, after) = (decision(cells.opt_int(3)), decision(cells.opt_int(4)));
            if before != after {
                fields.push(FieldChange {
                    field: "decision".into(),
                    before,
                    after,
                });
            }
        }
        Ok(fields)
    }

    fn links_change(
        &self,
        parent: &str,
        hash: &str,
        id: &str,
    ) -> Result<Option<FieldChange>, StoreError> {
        let changed = match self.row(
            "SELECT 1 FROM dolt_diff_finding_links(?1, ?2) WHERE to_issue = ?3 OR from_issue = ?3",
            &[parent.into(), hash.into(), id.into()],
        ) {
            Err(error) if error.is_missing_table("finding_links") => return Ok(None),
            row => row?.is_some(),
        };
        if !changed {
            return Ok(None);
        }
        let links_at = |commit: &str| -> Result<Option<String>, StoreError> {
            let rows = match self.rows(
                &format!(
                    "SELECT kind, id FROM dolt_at_finding_links('{commit}') \
                     WHERE issue = ? ORDER BY kind, id"
                ),
                &[id.into()],
            ) {
                Err(error) if error.is_missing_table("finding_links") => Vec::new(),
                rows => rows?,
            };
            let links: Vec<serde_json::Value> = rows
                .iter()
                .map(|row| {
                    let cells = Cells(row);
                    json!({ "kind": cells.text(0), "id": cells.text(1) })
                })
                .collect();
            Ok((!links.is_empty()).then(|| json::text(&serde_json::Value::from(links))))
        };
        Ok(Some(FieldChange {
            field: "links".into(),
            before: links_at(parent)?,
            after: links_at(hash)?,
        }))
    }

    fn relation_changes(
        &self,
        parent: &str,
        hash: &str,
        id: &str,
    ) -> Result<Vec<FieldChange>, StoreError> {
        let rows = self.rows(
            "SELECT from_source, from_kind, from_target, to_source, to_kind, to_target \
             FROM dolt_diff_relations(?1, ?2) \
             WHERE to_source = ?3 OR to_target = ?3 OR from_source = ?3 OR from_target = ?3",
            &[parent.into(), hash.into(), id.into()],
        )?;
        let relation = |cells: &Cells<'_>, at: usize| -> Option<String> {
            let source = cells.opt_text(at)?;
            let kind = cells.text(at + 1);
            let target = cells.text(at + 2);
            let (direction, other) = if source == id {
                ("outgoing", target)
            } else {
                ("incoming", source)
            };
            Some(json::text(
                &json!({ "kind": kind, "direction": direction, "issue": other }),
            ))
        };
        Ok(rows
            .iter()
            .map(|row| {
                let cells = Cells(row);
                FieldChange {
                    field: "relations".into(),
                    before: relation(&cells, 0),
                    after: relation(&cells, 3),
                }
            })
            .collect())
    }

    fn project_fields(
        &self,
        parent: &str,
        hash: &str,
        key: &str,
    ) -> Result<Vec<FieldChange>, StoreError> {
        let selected: Vec<String> = PROJECT_FIELDS
            .iter()
            .flat_map(|(column, _, _)| [format!("from_{column}"), format!("to_{column}")])
            .collect();
        let sql = format!(
            "SELECT {} FROM dolt_diff_projects(?1, ?2) WHERE to_key = ?3 OR from_key = ?3",
            selected.join(", ")
        );
        let mut fields = Vec::new();
        if let Some(row) = self.row(&sql, &[parent.into(), hash.into(), key.into()])? {
            let cells = Cells(&row);
            for (index, (_, field, shape)) in PROJECT_FIELDS.iter().enumerate() {
                let before = render(&cells.value(2 * index), *shape);
                let after = render(&cells.value(1 + 2 * index), *shape);
                if before != after {
                    fields.push(FieldChange {
                        field: (*field).to_owned(),
                        before,
                        after,
                    });
                }
            }
        }
        let before = self.workflow_at(parent, key)?;
        let after = self.workflow_at(hash, key)?;
        if before != after {
            fields.push(FieldChange {
                field: "workflow".into(),
                before,
                after,
            });
        }
        Ok(fields)
    }

    fn workflow_at(&self, commit: &str, key: &str) -> Result<Option<String>, StoreError> {
        let moves = self.row(
            &format!("SELECT moves FROM dolt_at_projects('{commit}') WHERE key = ?"),
            &[key.into()],
        )?;
        let Some(moves) = moves.and_then(|row| Cells(&row).opt_text(0)) else {
            return Ok(None);
        };
        let statuses: Vec<serde_json::Value> = self
            .rows(
                &format!(
                    "SELECT key, name, category FROM dolt_at_statuses('{commit}') \
                     WHERE project = ? ORDER BY position"
                ),
                &[key.into()],
            )?
            .iter()
            .map(|row| {
                let cells = Cells(row);
                json!({ "key": cells.text(0), "name": cells.text(1), "category": cells.text(2) })
            })
            .collect();
        let moves: serde_json::Value =
            serde_json::from_str(&moves).unwrap_or(serde_json::Value::Null);
        Ok(Some(json::text(
            &json!({ "statuses": statuses, "moves": moves }),
        )))
    }
}

fn render(value: &Value, shape: Shape) -> Option<String> {
    match (value, shape) {
        (Value::Null, _) => None,
        (Value::Text(text), Shape::Json) => Some(text.clone()),
        (Value::Text(text), _) => Some(json::string(text)),
        (Value::Integer(number), _) => Some(number.to_string()),
        (Value::Real(number), _) => Some(serde_json::Value::from(*number).to_string()),
        (Value::Blob(_), _) => None,
    }
}

fn entry_from_row(row: &Row) -> Result<Entry, StoreError> {
    let cells = Cells(row);
    Ok(Entry {
        seq: cells.int(0),
        project: cells.text(1),
        issue: cells.text(2),
        hash: cells.text(3),
        parent: cells.text(4),
        message: Message::decode(&cells.text(5))?,
    })
}

fn parse_history_cursor(cursor: &str) -> Result<(i64, String, String), StoreError> {
    let mut parts = cursor.splitn(4, ':');
    let (Some("h"), Some(seq), Some(project), Some(issue)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(StoreError::BadCursor);
    };
    let seq: i64 = seq.parse().map_err(|_| StoreError::BadCursor)?;
    if project.is_empty() {
        return Err(StoreError::BadCursor);
    }
    Ok((seq, project.to_owned(), issue.to_owned()))
}
