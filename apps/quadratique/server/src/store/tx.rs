use super::engine::{Cells, execute, text_param};
use super::error::{Kind, StoreError};
use super::json::{canonical, canonical_opt};
use super::types::{
    FindingLink, IssueCreated, IssueFields, NewIssue, Project, ProjectChanges, Workflow,
};
use super::view::View;
use doltlite::Value;
use std::collections::BTreeSet;
use std::ops::Deref;

pub struct Tx<'c> {
    view: View<'c>,
    now: String,
    created: BTreeSet<String>,
    revised: BTreeSet<String>,
    reason: Option<String>,
}

impl<'c> Deref for Tx<'c> {
    type Target = View<'c>;

    fn deref(&self) -> &View<'c> {
        &self.view
    }
}

impl<'c> Tx<'c> {
    pub(crate) fn new(view: View<'c>, now: String) -> Tx<'c> {
        Tx {
            view,
            now,
            created: BTreeSet::new(),
            revised: BTreeSet::new(),
            reason: None,
        }
    }

    /// Replaces the reason of the call for this write's commit, once the write has read what
    /// decides it.
    pub fn set_reason(&mut self, reason: String) {
        self.reason = Some(reason);
    }

    pub fn now(&self) -> &str {
        &self.now
    }

    fn execute(&self, sql: &str, params: &[Value]) -> Result<usize, StoreError> {
        execute(self.view.connection, sql, params)
    }

    /// Ends the write: the reason it set, if any.
    pub(crate) fn finish(self) -> Result<Option<String>, StoreError> {
        for id in self.revised.difference(&self.created) {
            self.execute(
                "UPDATE issues SET revision = revision + 1, updated_at = ? WHERE id = ?",
                &[self.now.as_str().into(), id.as_str().into()],
            )?;
        }
        Ok(self.reason)
    }

    pub fn create_project(&mut self, project: &Project) -> Result<(), StoreError> {
        if self
            .row(
                "SELECT 1 FROM projects WHERE key = ?",
                &[project.key.as_str().into()],
            )?
            .is_some()
        {
            return Err(StoreError::ProjectExists(project.key.clone()));
        }
        if let Some(repository) = &project.repository {
            self.refuse_bound_repository(&project.key, repository)?;
        }
        self.execute(
            "INSERT INTO projects (key, name, repository, default_branch, moves, extra) \
             VALUES (?, ?, ?, ?, ?, ?)",
            &[
                project.key.as_str().into(),
                project.name.as_str().into(),
                text_param(project.repository.as_deref()),
                project.default_branch.as_str().into(),
                canonical("moves", &project.workflow.moves)?.into(),
                canonical("extra", &project.extra)?.into(),
            ],
        )?;
        self.write_statuses(&project.key, &project.workflow)
    }

    pub fn update_project(
        &mut self,
        key: &str,
        changes: &ProjectChanges,
    ) -> Result<(), StoreError> {
        self.require_project(key)?;
        if let Some(Some(repository)) = &changes.repository {
            self.refuse_bound_repository(key, repository)?;
        }
        let extra = changes
            .extra
            .as_deref()
            .map(|extra| canonical("extra", extra))
            .transpose()?;
        let updates: [(&str, Option<Value>); 4] = [
            ("name", changes.name.as_deref().map(Value::from)),
            (
                "default_branch",
                changes.default_branch.as_deref().map(Value::from),
            ),
            (
                "repository",
                changes
                    .repository
                    .as_ref()
                    .map(|repository| text_param(repository.as_deref())),
            ),
            ("extra", extra.map(Value::from)),
        ];
        for (column, value) in updates {
            if let Some(value) = value {
                self.execute(
                    &format!(
                        "UPDATE projects SET {column} = ?1 WHERE key = ?2 AND {column} IS NOT ?1"
                    ),
                    &[value, key.into()],
                )?;
            }
        }
        Ok(())
    }

    pub fn replace_workflow(&mut self, key: &str, workflow: &Workflow) -> Result<(), StoreError> {
        self.require_project(key)?;
        self.execute(
            "UPDATE projects SET moves = ?1 WHERE key = ?2 AND moves IS NOT ?1",
            &[canonical("moves", &workflow.moves)?.into(), key.into()],
        )?;
        self.execute("DELETE FROM statuses WHERE project = ?", &[key.into()])?;
        self.write_statuses(key, workflow)
    }

    fn write_statuses(&self, key: &str, workflow: &Workflow) -> Result<(), StoreError> {
        for (position, status) in workflow.statuses.iter().enumerate() {
            self.execute(
                "INSERT INTO statuses (project, key, name, category, position) VALUES (?, ?, ?, ?, ?)",
                &[
                    key.into(),
                    status.key.as_str().into(),
                    status.name.as_str().into(),
                    status.category.as_str().into(),
                    Value::from(position as i64),
                ],
            )?;
        }
        Ok(())
    }

    fn refuse_bound_repository(&self, key: &str, repository: &str) -> Result<(), StoreError> {
        let bound = self.row(
            "SELECT key FROM projects WHERE repository = ? AND key <> ?",
            &[repository.into(), key.into()],
        )?;
        match bound {
            Some(row) => Err(StoreError::RepositoryBound(Cells(&row).text(0))),
            None => Ok(()),
        }
    }

    fn require_project(&self, key: &str) -> Result<(), StoreError> {
        match self.row("SELECT 1 FROM projects WHERE key = ?", &[key.into()])? {
            Some(_) => Ok(()),
            None => Err(StoreError::NotFound {
                kind: Kind::Project,
                id: key.to_owned(),
            }),
        }
    }

    pub(crate) fn require_issue(&self, id: &str) -> Result<(), StoreError> {
        match self.row("SELECT 1 FROM issues WHERE id = ?", &[id.into()])? {
            Some(_) => Ok(()),
            None => Err(StoreError::NotFound {
                kind: Kind::Issue,
                id: id.to_owned(),
            }),
        }
    }

    pub fn create_issue(&mut self, issue: &NewIssue) -> Result<IssueCreated, StoreError> {
        if let Some(key) = &issue.client_key {
            let existing = self.row(
                "SELECT id FROM issues WHERE client_key = ?",
                &[key.as_str().into()],
            )?;
            if let Some(row) = existing {
                return Ok(IssueCreated::Existing(Cells(&row).text(0)));
            }
        }
        let numbered = self.row(
            "UPDATE projects SET next_number = next_number + 1 WHERE key = ? RETURNING next_number - 1",
            &[issue.project.as_str().into()],
        )?;
        let Some(numbered) = numbered else {
            return Err(StoreError::NotFound {
                kind: Kind::Project,
                id: issue.project.clone(),
            });
        };
        let number = Cells(&numbered).int(0);
        let id = format!("{}-{number}", issue.project);
        let order = match issue.order {
            Some(order) => order,
            None => self
                .row(
                    "SELECT coalesce(max(sort_order), 0) + 1 FROM issues WHERE project = ?",
                    &[issue.project.as_str().into()],
                )?
                .map(|row| Cells(&row).real(0))
                .unwrap_or(1.0),
        };
        self.execute(
            "INSERT INTO issues (id, project, number, title, status, priority, assignee, parent, \
             milestone, sort_order, due, description, acceptance, \
             client_key, client_request, created_by, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &[
                id.as_str().into(),
                issue.project.as_str().into(),
                number.into(),
                issue.title.as_str().into(),
                issue.status.as_str().into(),
                issue.priority.into(),
                text_param(canonical_opt("assignee", issue.assignee.as_deref())?.as_deref()),
                text_param(issue.parent.as_deref()),
                text_param(issue.milestone.as_deref()),
                order.into(),
                text_param(issue.due.as_deref()),
                text_param(issue.description.as_deref()),
                text_param(issue.acceptance.as_deref()),
                text_param(issue.client_key.as_deref()),
                text_param(
                    canonical_opt("client request", issue.client_request.as_deref())?.as_deref(),
                ),
                canonical("created_by", &issue.created_by)?.into(),
                self.now.as_str().into(),
                self.now.as_str().into(),
            ],
        )?;
        self.insert_labels(&id, &issue.labels)?;
        if let Some(note) = &issue.notes {
            self.add_note(&id, note, &issue.created_by)?;
        }
        self.created.insert(id.clone());
        Ok(IssueCreated::Created(id))
    }

    fn set_column(&mut self, id: &str, column: &str, value: Value) -> Result<bool, StoreError> {
        self.require_issue(id)?;
        let changed = self.execute(
            &format!("UPDATE issues SET {column} = ?1 WHERE id = ?2 AND {column} IS NOT ?1"),
            &[value, id.into()],
        )? > 0;
        if changed {
            self.revised.insert(id.to_owned());
        }
        Ok(changed)
    }

    pub fn update_fields(&mut self, id: &str, fields: &IssueFields) -> Result<bool, StoreError> {
        self.require_issue(id)?;
        let assignee = match &fields.assignee {
            Some(assignee) => Some(text_param(
                canonical_opt("assignee", assignee.as_deref())?.as_deref(),
            )),
            None => None,
        };
        let text = |value: &Option<Option<String>>| {
            value.as_ref().map(|value| text_param(value.as_deref()))
        };
        let updates: [(&str, Option<Value>); 8] = [
            ("title", fields.title.as_deref().map(Value::from)),
            ("priority", fields.priority.map(Value::from)),
            ("assignee", assignee),
            ("milestone", text(&fields.milestone)),
            ("due", text(&fields.due)),
            ("description", text(&fields.description)),
            ("acceptance", text(&fields.acceptance)),
            ("evidence", text(&fields.evidence)),
        ];
        let mut changed = false;
        for (column, value) in updates {
            if let Some(value) = value {
                changed |= self.set_column(id, column, value)?;
            }
        }
        Ok(changed)
    }

    pub fn add_note(&mut self, id: &str, text: &str, author: &str) -> Result<(), StoreError> {
        self.require_issue(id)?;
        self.execute(
            "INSERT INTO notes (issue, number, text, author, at) \
             SELECT ?1, coalesce(max(number), 0) + 1, ?2, ?3, ?4 FROM notes WHERE issue = ?1",
            &[
                id.into(),
                text.into(),
                canonical("note author", author)?.into(),
                self.now.as_str().into(),
            ],
        )?;
        self.revised.insert(id.to_owned());
        Ok(())
    }

    pub fn set_status(&mut self, id: &str, status: &str) -> Result<bool, StoreError> {
        self.set_column(id, "status", status.into())
    }

    pub fn set_parent(&mut self, id: &str, parent: Option<&str>) -> Result<bool, StoreError> {
        if let Some(parent) = parent {
            self.require_issue(parent)?;
        }
        self.set_column(id, "parent", text_param(parent))
    }

    pub fn set_order(&mut self, id: &str, order: f64) -> Result<bool, StoreError> {
        self.set_column(id, "sort_order", order.into())
    }

    pub fn take_claim(
        &mut self,
        id: &str,
        holder: &str,
        key: &str,
        branch: &str,
        worktree: &str,
    ) -> Result<bool, StoreError> {
        self.require_issue(id)?;
        let won = self.execute(
            "UPDATE issues SET claim_holder = ?, claim_key = ?, claim_branch = ?, claim_worktree = ?, \
             claim_since = ? WHERE id = ? AND claim_holder IS NULL",
            &[
                canonical("claim holder", holder)?.into(),
                key.into(),
                branch.into(),
                worktree.into(),
                self.now.as_str().into(),
                id.into(),
            ],
        )? == 1;
        if won {
            self.revised.insert(id.to_owned());
        }
        Ok(won)
    }

    pub fn clear_claim(&mut self, id: &str) -> Result<bool, StoreError> {
        self.require_issue(id)?;
        let cleared = self.execute(
            "UPDATE issues SET claim_holder = NULL, claim_key = NULL, claim_branch = NULL, \
             claim_worktree = NULL, claim_since = NULL WHERE id = ? AND claim_holder IS NOT NULL",
            &[id.into()],
        )? == 1;
        if cleared {
            self.revised.insert(id.to_owned());
        }
        Ok(cleared)
    }

    pub fn set_labels(&mut self, id: &str, labels: &[String]) -> Result<bool, StoreError> {
        self.require_issue(id)?;
        let wanted: BTreeSet<&str> = labels.iter().map(String::as_str).collect();
        let current = self
            .issue(id)?
            .map(|issue| issue.labels)
            .unwrap_or_default();
        let current: BTreeSet<&str> = current.iter().map(String::as_str).collect();
        if wanted == current {
            return Ok(false);
        }
        self.execute("DELETE FROM labels WHERE issue = ?", &[id.into()])?;
        self.insert_labels(id, labels)?;
        self.revised.insert(id.to_owned());
        Ok(true)
    }

    fn insert_labels(&self, id: &str, labels: &[String]) -> Result<(), StoreError> {
        for label in labels.iter().collect::<BTreeSet<_>>() {
            self.execute(
                "INSERT INTO labels (issue, label) VALUES (?, ?)",
                &[id.into(), label.as_str().into()],
            )?;
        }
        Ok(())
    }

    pub fn add_relation(
        &mut self,
        source: &str,
        kind: &str,
        target: &str,
    ) -> Result<bool, StoreError> {
        self.require_issue(source)?;
        self.require_issue(target)?;
        Ok(self.execute(
            "INSERT OR IGNORE INTO relations (source, kind, target) VALUES (?, ?, ?)",
            &[source.into(), kind.into(), target.into()],
        )? == 1)
    }

    pub fn remove_relation(
        &mut self,
        source: &str,
        kind: &str,
        target: &str,
    ) -> Result<bool, StoreError> {
        if kind == "blocks" {
            self.execute(
                "DELETE FROM finding_blocks WHERE source = ? AND target = ?",
                &[source.into(), target.into()],
            )?;
        }
        Ok(self.execute(
            "DELETE FROM relations WHERE source = ? AND kind = ? AND target = ?",
            &[source.into(), kind.into(), target.into()],
        )? == 1)
    }

    pub fn add_finding_block(&mut self, source: &str, target: &str) -> Result<bool, StoreError> {
        if !self.add_relation(source, "blocks", target)? {
            return Ok(false);
        }
        self.execute(
            "INSERT INTO finding_blocks (source, target) VALUES (?, ?)",
            &[source.into(), target.into()],
        )?;
        Ok(true)
    }

    pub fn set_source(
        &mut self,
        id: &str,
        tool: &str,
        key: &str,
        decision: bool,
    ) -> Result<bool, StoreError> {
        let project =
            self.issue(id)?
                .map(|issue| issue.project)
                .ok_or_else(|| StoreError::NotFound {
                    kind: Kind::Issue,
                    id: id.to_owned(),
                })?;
        let changed = self.execute(
            "INSERT INTO sources (issue, project, tool, key, decision) VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT (issue) DO UPDATE SET decision = ?5 WHERE decision IS NOT ?5",
            &[
                id.into(),
                project.as_str().into(),
                tool.into(),
                key.into(),
                Value::from(i64::from(decision)),
            ],
        )? > 0;
        if changed {
            self.revised.insert(id.to_owned());
        }
        Ok(changed)
    }

    pub fn set_finished_evidence(
        &mut self,
        id: &str,
        evidence: Option<&str>,
    ) -> Result<(), StoreError> {
        self.execute(
            "UPDATE sources SET finished_evidence = ?1 WHERE issue = ?2 AND finished_evidence IS NOT ?1",
            &[text_param(evidence), id.into()],
        )?;
        Ok(())
    }

    pub fn set_finding_links(
        &mut self,
        id: &str,
        links: &[FindingLink],
    ) -> Result<bool, StoreError> {
        self.require_issue(id)?;
        let wanted: BTreeSet<&FindingLink> = links.iter().collect();
        let current = self.finding_links(id)?;
        if wanted == current.iter().collect::<BTreeSet<_>>() {
            return Ok(false);
        }
        self.execute("DELETE FROM finding_links WHERE issue = ?", &[id.into()])?;
        for link in wanted {
            self.execute(
                "INSERT INTO finding_links (issue, kind, id) VALUES (?, ?, ?)",
                &[
                    id.into(),
                    link.kind.as_str().into(),
                    link.id.as_str().into(),
                ],
            )?;
        }
        self.revised.insert(id.to_owned());
        Ok(true)
    }
}
