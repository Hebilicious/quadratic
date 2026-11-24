use super::engine::{Cells, execute, query, query_one};
use super::error::StoreError;
use super::message::Message;
use doltlite::{Connection, Value};
use std::collections::BTreeSet;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Subjects {
    pub(crate) issues: BTreeSet<String>,
    pub(crate) projects: BTreeSet<String>,
}

impl Subjects {
    fn rows(&self) -> impl Iterator<Item = (String, String)> + '_ {
        let issues = self
            .issues
            .iter()
            .map(|issue| (project_of(issue).to_owned(), issue.clone()));
        let projects = self
            .projects
            .iter()
            .map(|project| (project.clone(), String::new()));
        issues.chain(projects)
    }
}

pub(crate) fn project_of(issue: &str) -> &str {
    issue.rsplit_once('-').map_or(issue, |(project, _)| project)
}

pub(crate) fn subjects(
    connection: &Connection,
    from: &str,
    to: &str,
) -> Result<Subjects, StoreError> {
    let refs = [Value::from(from), Value::from(to)];
    let column = |sql: &str| -> Result<Vec<String>, StoreError> {
        Ok(query(connection, sql, &refs)?
            .iter()
            .flat_map(|row| {
                let cells = Cells(row);
                (0..row.values().len())
                    .filter_map(|index| cells.opt_text(index))
                    .collect::<Vec<_>>()
            })
            .collect())
    };
    let mut subjects = Subjects::default();
    subjects.issues.extend(column(
        "SELECT to_id, from_id FROM dolt_diff_issues(?1, ?2)",
    )?);
    subjects.issues.extend(column(
        "SELECT to_issue, from_issue FROM dolt_diff_labels(?1, ?2)",
    )?);
    subjects.issues.extend(column(
        "SELECT to_source, to_target, from_source, from_target FROM dolt_diff_relations(?1, ?2)",
    )?);
    for table in ["git_landings", "notes"] {
        match column(&format!(
            "SELECT to_issue, from_issue FROM dolt_diff_{table}(?1, ?2)"
        )) {
            Ok(issues) => subjects.issues.extend(issues),
            Err(error) if error.is_missing_table(table) => {}
            Err(error) => return Err(error),
        }
    }
    subjects.projects.extend(column(
        "SELECT to_key, from_key FROM dolt_diff_projects(?1, ?2) \
         WHERE diff_type <> 'modified' OR from_name IS NOT to_name \
         OR from_repository IS NOT to_repository OR from_default_branch IS NOT to_default_branch \
         OR from_moves IS NOT to_moves OR from_extra IS NOT to_extra",
    )?);
    subjects.projects.extend(column(
        "SELECT to_project, from_project FROM dolt_diff_statuses(?1, ?2)",
    )?);
    Ok(subjects)
}

pub(crate) fn record(
    connection: &Connection,
    parent: &str,
    hash: &str,
    message: &str,
    at: &str,
    subjects: &Subjects,
) -> Result<(), StoreError> {
    execute(
        connection,
        "INSERT INTO commit_index (hash, parent, at, message) VALUES (?, ?, ?, ?)",
        &[hash.into(), parent.into(), at.into(), message.into()],
    )?;
    let seq = connection.last_insert_rowid();
    for (project, issue) in subjects.rows() {
        execute(
            connection,
            "INSERT INTO commit_subjects (seq, project, issue) VALUES (?, ?, ?)",
            &[seq.into(), project.into(), issue.into()],
        )?;
    }
    Ok(())
}

pub(crate) fn head(connection: &Connection) -> Result<String, StoreError> {
    let row = query_one(connection, "SELECT dolt_hashof('HEAD')", &[])?
        .ok_or_else(|| StoreError::Engine("the database has no HEAD".into()))?;
    Ok(Cells(&row).text(0))
}

pub(crate) fn last_at(connection: &Connection) -> Result<Option<String>, StoreError> {
    Ok(query_one(
        connection,
        "SELECT at FROM commit_index ORDER BY seq DESC LIMIT 1",
        &[],
    )?
    .and_then(|row| Cells(&row).opt_text(0)))
}

pub(crate) struct Recorded {
    pub(crate) hash: String,
    pub(crate) message: String,
    pub(crate) subjects: Subjects,
}

pub(crate) fn catch_up(connection: &Connection) -> Result<Vec<Recorded>, StoreError> {
    let last = query_one(
        connection,
        "SELECT hash FROM commit_index ORDER BY seq DESC LIMIT 1",
        &[],
    )?
    .map(|row| Cells(&row).text(0));
    if last.as_deref() == Some(head(connection)?.as_str()) {
        return Ok(Vec::new());
    }
    let mut missing = Vec::new();
    for row in query(connection, "SELECT commit_hash, message FROM dolt_log", &[])? {
        let cells = Cells(&row);
        let hash = cells.text(0);
        let message = cells.text(1);
        if Some(&hash) == last.as_ref() || !Message::is_ours(&message) {
            break;
        }
        missing.push((hash, message));
    }
    connection.execute_batch("BEGIN IMMEDIATE")?;
    let outcome = (|| {
        let mut recorded = Vec::new();
        let mut first = last.is_none();
        for (hash, message) in missing.iter().rev() {
            let parent = query_one(
                connection,
                "SELECT parent_hash FROM dolt_commit_ancestors WHERE commit_hash = ? AND parent_index = 0",
                &[hash.as_str().into()],
            )?
            .map(|row| Cells(&row).text(0))
            .unwrap_or_default();
            let subjects = if first {
                Subjects::default()
            } else {
                subjects(connection, &parent, hash)?
            };
            first = false;
            let at = Message::decode(message)?.at;
            record(connection, &parent, hash, message, &at, &subjects)?;
            recorded.push(Recorded {
                hash: hash.clone(),
                message: message.clone(),
                subjects,
            });
        }
        Ok(recorded)
    })();
    match outcome {
        Ok(recorded) => {
            connection.execute_batch("COMMIT")?;
            Ok(recorded)
        }
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}
