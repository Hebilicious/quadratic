use super::engine::{Cells, query, query_one};
use super::error::StoreError;
use super::types::{
    AsOf, Claim, ClaimFilter, Direction, FindingLink, Graph, IssueFilter, IssueRecord, KeyedIssue,
    Link, NoteRecord, Page, Project, RelationRecord, SourceRecord, WaitNode, Workflow,
    WorkflowStatus,
};
use doltlite::{Connection, Row, Value};

pub struct View<'c> {
    pub(crate) connection: &'c Connection,
    at: Option<At>,
}

#[derive(Debug, Clone)]
pub(crate) struct At {
    pub(crate) hash: String,
    pub(crate) seq: i64,
}

const ISSUE_COLUMNS: &str = "i.id, i.project, i.number, i.title, i.status, s.category, i.priority, \
     i.assignee, i.parent, i.milestone, i.sort_order, i.due, i.description, \
     i.acceptance, i.evidence, i.claim_holder, i.claim_branch, \
     i.claim_worktree, i.claim_since, i.revision, i.created_by, i.created_at, i.updated_at";

impl<'c> View<'c> {
    pub(crate) fn now(connection: &'c Connection) -> View<'c> {
        View {
            connection,
            at: None,
        }
    }

    pub(crate) fn resolve(
        connection: &'c Connection,
        as_of: &AsOf,
    ) -> Result<View<'c>, StoreError> {
        let at = match as_of {
            AsOf::Now => last_indexed_when_behind_head(connection)?,
            AsOf::Commit(prefix) => Some(commit_with_prefix(connection, prefix)?),
            AsOf::Time(time) => Some(commit_at_time(connection, time)?),
        };
        Ok(View { connection, at })
    }

    pub fn commit(&self) -> Option<&str> {
        self.at.as_ref().map(|at| at.hash.as_str())
    }

    pub(crate) fn bound(&self) -> Option<i64> {
        self.at.as_ref().map(|at| at.seq)
    }

    pub(crate) fn table(&self, name: &str) -> String {
        match &self.at {
            None => name.to_owned(),
            Some(at) => format!("dolt_at_{name}('{}')", at.hash),
        }
    }

    pub(crate) fn rows(&self, sql: &str, params: &[Value]) -> Result<Vec<Row>, StoreError> {
        query(self.connection, sql, params)
    }

    pub(crate) fn row(&self, sql: &str, params: &[Value]) -> Result<Option<Row>, StoreError> {
        query_one(self.connection, sql, params)
    }

    pub fn project(&self, key: &str) -> Result<Option<Project>, StoreError> {
        self.projects_where("p.key = ?", &[key.into()])
            .map(|projects| projects.into_iter().next())
    }

    pub fn project_by_repository(&self, repository: &str) -> Result<Option<Project>, StoreError> {
        self.projects_where("p.repository = ?", &[repository.into()])
            .map(|projects| projects.into_iter().next())
    }

    pub fn projects(&self) -> Result<Vec<Project>, StoreError> {
        self.projects_where("1", &[])
    }

    fn projects_where(
        &self,
        condition: &str,
        params: &[Value],
    ) -> Result<Vec<Project>, StoreError> {
        let sql = format!(
            "SELECT p.key, p.name, p.repository, p.default_branch, p.extra, p.moves \
             FROM {} p WHERE {condition} ORDER BY p.key",
            self.table("projects")
        );
        self.rows(&sql, params)?
            .iter()
            .map(|row| {
                let cells = Cells(row);
                let key = cells.text(0);
                Ok(Project {
                    workflow: Workflow {
                        statuses: self.statuses(&key)?,
                        moves: cells.text(5),
                    },
                    key,
                    name: cells.text(1),
                    repository: cells.opt_text(2),
                    default_branch: cells.text(3),
                    extra: cells.text(4),
                })
            })
            .collect()
    }

    fn statuses(&self, project: &str) -> Result<Vec<WorkflowStatus>, StoreError> {
        let sql = format!(
            "SELECT key, name, category FROM {} WHERE project = ? ORDER BY position",
            self.table("statuses")
        );
        Ok(self
            .rows(&sql, &[project.into()])?
            .iter()
            .map(|row| {
                let cells = Cells(row);
                WorkflowStatus {
                    key: cells.text(0),
                    name: cells.text(1),
                    category: cells.text(2),
                }
            })
            .collect())
    }

    fn issue_source(&self) -> String {
        format!(
            "{} i LEFT JOIN {} s ON s.project = i.project AND s.key = i.status",
            self.table("issues"),
            self.table("statuses")
        )
    }

    pub fn issue(&self, id: &str) -> Result<Option<IssueRecord>, StoreError> {
        let sql = format!(
            "SELECT {ISSUE_COLUMNS} FROM {} WHERE i.id = ?",
            self.issue_source()
        );
        let mut issues = self.issues_from(self.rows(&sql, &[id.into()])?)?;
        Ok(issues.pop())
    }

    pub fn keyed_issue(&self, key: &str) -> Result<Option<KeyedIssue>, StoreError> {
        let sql = format!(
            "SELECT id, client_request FROM {} WHERE client_key = ?",
            self.table("issues")
        );
        Ok(self.row(&sql, &[key.into()])?.map(|row| {
            let cells = Cells(&row);
            KeyedIssue {
                id: cells.text(0),
                request: cells.opt_text(1),
            }
        }))
    }

    pub fn issues(
        &self,
        filter: &IssueFilter,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page<IssueRecord>, StoreError> {
        let mut conditions: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        if let Some(project) = &filter.project {
            conditions.push("i.project = ?".into());
            params.push(project.as_str().into());
        }
        if !filter.statuses.is_empty() {
            conditions.push("i.status IN (SELECT value FROM json_each(?))".into());
            params.push(json_array(&filter.statuses).into());
        }
        if !filter.categories.is_empty() {
            conditions.push("s.category IN (SELECT value FROM json_each(?))".into());
            params.push(json_array(&filter.categories).into());
        }
        for label in &filter.labels {
            conditions.push(format!(
                "EXISTS (SELECT 1 FROM {} l WHERE l.issue = i.id AND l.label = ?)",
                self.table("labels")
            ));
            params.push(label.as_str().into());
        }
        if let Some(assignee) = &filter.assignee {
            conditions.push("i.assignee = ?".into());
            params.push(super::json::canonical("assignee", assignee)?.into());
        }
        if let Some(parent) = &filter.parent {
            conditions.push("i.parent = ?".into());
            params.push(parent.as_str().into());
        }
        match &filter.claim {
            Some(ClaimFilter::HeldBy(key)) => {
                conditions.push("i.claim_key = ?".into());
                params.push(key.as_str().into());
            }
            Some(ClaimFilter::Any) => conditions.push("i.claim_holder IS NOT NULL".into()),
            Some(ClaimFilter::Unclaimed) => conditions.push("i.claim_holder IS NULL".into()),
            None => {}
        }
        if let Some(tool) = &filter.tool {
            conditions.push(format!(
                "EXISTS (SELECT 1 FROM {} src WHERE src.issue = i.id AND src.tool = ?)",
                self.table("sources")
            ));
            params.push(tool.as_str().into());
        }
        for word in filter.text.iter().flat_map(|text| text.split_whitespace()) {
            conditions.push(
                "(instr(lower(i.title), lower(?)) > 0 \
                 OR instr(lower(coalesce(i.description, '')), lower(?)) > 0)"
                    .into(),
            );
            params.push(word.into());
            params.push(word.into());
        }
        if let Some(cursor) = cursor {
            let (order, project, number) = parse_issue_cursor(cursor)?;
            conditions.push(
                "(i.sort_order > ? OR (i.sort_order = ? AND (i.project > ? \
                 OR (i.project = ? AND i.number > ?))))"
                    .into(),
            );
            params.extend([
                order.into(),
                order.into(),
                project.as_str().into(),
                project.as_str().into(),
                number.into(),
            ]);
        }
        let condition = if conditions.is_empty() {
            "1".to_owned()
        } else {
            conditions.join(" AND ")
        };
        let sql = format!(
            "SELECT {ISSUE_COLUMNS} FROM {} WHERE {condition} \
             ORDER BY i.sort_order, i.project, i.number LIMIT ?",
            self.issue_source()
        );
        params.push(Value::from(limit as i64 + 1));
        let rows = match self.rows(&sql, &params) {
            Err(error) if filter.tool.is_some() && error.is_missing_table("sources") => Vec::new(),
            rows => rows?,
        };
        let mut items = self.issues_from(rows)?;
        let next_cursor = if items.len() > limit {
            items.truncate(limit);
            items.last().map(IssueRecord::cursor)
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    fn issues_from(&self, rows: Vec<Row>) -> Result<Vec<IssueRecord>, StoreError> {
        let mut issues: Vec<IssueRecord> = rows.iter().map(issue_from_row).collect();
        if issues.is_empty() {
            return Ok(issues);
        }
        let ids: Vec<String> = issues.iter().map(|issue| issue.id.clone()).collect();
        let sql = format!(
            "SELECT issue, label FROM {} WHERE issue IN (SELECT value FROM json_each(?)) \
             ORDER BY issue, label",
            self.table("labels")
        );
        for row in self.rows(&sql, &[json_array(&ids).into()])? {
            let cells = Cells(&row);
            let issue_id = cells.text(0);
            if let Some(issue) = issues.iter_mut().find(|issue| issue.id == issue_id) {
                issue.labels.push(cells.text(1));
            }
        }
        Ok(issues)
    }

    pub fn relations(&self, id: &str) -> Result<Vec<RelationRecord>, StoreError> {
        let relations = self.table("relations");
        let issues = self.table("issues");
        let statuses = self.table("statuses");
        let side = |own: &str, other: &str, direction: &str| {
            format!(
                "SELECT r.kind, '{direction}', o.id, o.title, o.status, s.category \
                 FROM {relations} r JOIN {issues} o ON o.id = r.{other} \
                 LEFT JOIN {statuses} s ON s.project = o.project AND s.key = o.status \
                 WHERE r.{own} = ?"
            )
        };
        let sql = format!(
            "{} UNION ALL {} ORDER BY 2 DESC, 1, 3",
            side("source", "target", "outgoing"),
            side("target", "source", "incoming")
        );
        Ok(self
            .rows(&sql, &[id.into(), id.into()])?
            .iter()
            .map(|row| {
                let cells = Cells(row);
                RelationRecord {
                    kind: cells.text(0),
                    direction: if cells.text(1) == "outgoing" {
                        Direction::Outgoing
                    } else {
                        Direction::Incoming
                    },
                    issue: cells.text(2),
                    title: cells.text(3),
                    status: cells.text(4),
                    category: cells.opt_text(5),
                }
            })
            .collect())
    }

    pub fn children(&self, id: &str) -> Result<Vec<String>, StoreError> {
        let sql = format!(
            "SELECT id FROM {} WHERE parent = ? ORDER BY sort_order, project, number",
            self.table("issues")
        );
        Ok(self
            .rows(&sql, &[id.into()])?
            .iter()
            .map(|row| Cells(row).text(0))
            .collect())
    }

    pub fn notes(&self, id: &str, limit: usize) -> Result<Vec<NoteRecord>, StoreError> {
        let sql = format!(
            "SELECT text, author, at FROM (SELECT number, text, author, at FROM {} \
             WHERE issue = ? ORDER BY number DESC LIMIT ?) ORDER BY number",
            self.table("notes")
        );
        let rows = match self.rows(&sql, &[id.into(), Value::from(limit as i64)]) {
            Err(error) if error.is_missing_table("notes") => return Ok(Vec::new()),
            rows => rows?,
        };
        Ok(rows
            .iter()
            .map(|row| {
                let cells = Cells(row);
                NoteRecord {
                    text: cells.text(0),
                    author: cells.text(1),
                    at: cells.text(2),
                }
            })
            .collect())
    }

    pub fn source(&self, id: &str) -> Result<Option<SourceRecord>, StoreError> {
        Ok(self.sources_where("src.issue = ?", &[id.into()])?.pop())
    }

    pub fn sources(&self, project: &str, tool: &str) -> Result<Vec<SourceRecord>, StoreError> {
        self.sources_where(
            "src.project = ? AND src.tool = ?",
            &[project.into(), tool.into()],
        )
    }

    fn sources_where(
        &self,
        condition: &str,
        params: &[Value],
    ) -> Result<Vec<SourceRecord>, StoreError> {
        let sql = format!(
            "SELECT src.issue, src.tool, src.key, src.decision, s.category, src.finished_evidence \
             FROM {} src JOIN {} i ON i.id = src.issue \
             LEFT JOIN {} s ON s.project = i.project AND s.key = i.status \
             WHERE {condition} ORDER BY src.issue",
            self.table("sources"),
            self.table("issues"),
            self.table("statuses")
        );
        let rows = match self.rows(&sql, params) {
            Err(error) if error.is_missing_table("sources") => return Ok(Vec::new()),
            rows => rows?,
        };
        Ok(rows
            .iter()
            .map(|row| {
                let cells = Cells(row);
                SourceRecord {
                    issue: cells.text(0),
                    tool: cells.text(1),
                    key: cells.text(2),
                    decision: cells.int(3) != 0,
                    category: cells.opt_text(4),
                    finished_evidence: cells.opt_text(5),
                }
            })
            .collect())
    }

    pub fn finding_links(&self, id: &str) -> Result<Vec<FindingLink>, StoreError> {
        let sql = format!(
            "SELECT kind, id FROM {} WHERE issue = ? ORDER BY kind, id",
            self.table("finding_links")
        );
        let rows = match self.rows(&sql, &[id.into()]) {
            Err(error) if error.is_missing_table("finding_links") => return Ok(Vec::new()),
            rows => rows?,
        };
        Ok(rows
            .iter()
            .map(|row| {
                let cells = Cells(row);
                FindingLink {
                    kind: cells.text(0),
                    id: cells.text(1),
                }
            })
            .collect())
    }

    pub fn finding_blocks(
        &self,
        project: &str,
        tool: &str,
    ) -> Result<Vec<(String, String)>, StoreError> {
        let sql = format!(
            "SELECT b.source, b.target FROM {} b JOIN {} src ON src.issue = b.source \
             WHERE src.project = ? AND src.tool = ? ORDER BY b.source, b.target",
            self.table("finding_blocks"),
            self.table("sources")
        );
        Ok(self
            .rows(&sql, &[project.into(), tool.into()])?
            .iter()
            .map(|row| {
                let cells = Cells(row);
                (cells.text(0), cells.text(1))
            })
            .collect())
    }

    pub fn graph(&self) -> Result<Graph, StoreError> {
        let sql = format!(
            "SELECT i.id, i.parent, i.status, s.category FROM {} ORDER BY i.id",
            self.issue_source()
        );
        let issues = self
            .rows(&sql, &[])?
            .iter()
            .map(|row| {
                let cells = Cells(row);
                WaitNode {
                    id: cells.text(0),
                    parent: cells.opt_text(1),
                    status: cells.text(2),
                    category: cells.opt_text(3),
                }
            })
            .collect();
        let sql = format!(
            "SELECT source, kind, target FROM {} ORDER BY source, kind, target",
            self.table("relations")
        );
        let links = self
            .rows(&sql, &[])?
            .iter()
            .map(|row| {
                let cells = Cells(row);
                Link {
                    source: cells.text(0),
                    kind: cells.text(1),
                    target: cells.text(2),
                }
            })
            .collect();
        Ok(Graph { issues, links })
    }
}

fn issue_from_row(row: &Row) -> IssueRecord {
    let cells = Cells(row);
    let claim = cells.opt_text(15).map(|holder| Claim {
        holder,
        branch: cells.text(16),
        worktree: cells.text(17),
        since: cells.text(18),
    });
    IssueRecord {
        id: cells.text(0),
        project: cells.text(1),
        number: cells.int(2),
        title: cells.text(3),
        status: cells.text(4),
        category: cells.opt_text(5),
        priority: cells.int(6),
        labels: Vec::new(),
        assignee: cells.opt_text(7),
        parent: cells.opt_text(8),
        milestone: cells.opt_text(9),
        order: cells.real(10),
        due: cells.opt_text(11),
        description: cells.opt_text(12),
        acceptance: cells.opt_text(13),
        evidence: cells.opt_text(14),
        claim,
        revision: cells.int(19),
        created_by: cells.text(20),
        created_at: cells.text(21),
        updated_at: cells.text(22),
    }
}

impl IssueRecord {
    pub fn cursor(&self) -> String {
        format!("i:{}:{}:{}", self.order, self.project, self.number)
    }
}

fn parse_issue_cursor(cursor: &str) -> Result<(f64, String, i64), StoreError> {
    let mut parts = cursor.splitn(4, ':');
    let (Some("i"), Some(order), Some(project), Some(number)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(StoreError::BadCursor);
    };
    let order: f64 = order.parse().map_err(|_| StoreError::BadCursor)?;
    let number: i64 = number.parse().map_err(|_| StoreError::BadCursor)?;
    if !order.is_finite() || project.is_empty() {
        return Err(StoreError::BadCursor);
    }
    Ok((order, project.to_owned(), number))
}

pub(crate) fn json_array(items: &[String]) -> String {
    serde_json::Value::from(items.to_vec()).to_string()
}

fn last_indexed_when_behind_head(connection: &Connection) -> Result<Option<At>, StoreError> {
    let indexed = query_one(
        connection,
        "SELECT seq, hash FROM commit_index ORDER BY seq DESC LIMIT 1",
        &[],
    )?
    .map(|row| at_from_row(&row));
    match indexed {
        Some(at) if at.hash != super::index::head(connection)? => Ok(Some(at)),
        _ => Ok(None),
    }
}

fn commit_with_prefix(connection: &Connection, prefix: &str) -> Result<At, StoreError> {
    let unknown = || StoreError::UnknownCommit(prefix.to_owned());
    let well_formed = (7..=64).contains(&prefix.len())
        && prefix
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !well_formed {
        return Err(unknown());
    }
    let rows = query(
        connection,
        "SELECT seq, hash FROM commit_index WHERE substr(hash, 1, ?) = ? LIMIT 2",
        &[Value::from(prefix.len() as i64), prefix.into()],
    )?;
    match rows.as_slice() {
        [row] => Ok(at_from_row(row)),
        _ => Err(unknown()),
    }
}

fn commit_at_time(connection: &Connection, time: &str) -> Result<At, StoreError> {
    let moment = super::clock::parse(time)?;
    let latest = query_one(
        connection,
        "SELECT seq, hash FROM commit_index WHERE at <= ? ORDER BY seq DESC LIMIT 1",
        &[moment.into()],
    )?;
    let row = match latest {
        Some(row) => row,
        None => query_one(
            connection,
            "SELECT seq, hash FROM commit_index ORDER BY seq LIMIT 1",
            &[],
        )?
        .ok_or_else(|| StoreError::Corrupt("the commit index is empty".into()))?,
    };
    Ok(at_from_row(&row))
}

fn at_from_row(row: &Row) -> At {
    let cells = Cells(row);
    At {
        seq: cells.int(0),
        hash: cells.text(1),
    }
}
