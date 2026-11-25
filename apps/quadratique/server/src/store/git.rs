use super::engine::{Cells, execute, text_param};
use super::error::StoreError;
use super::tx::Tx;
use super::view::View;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWatch {
    pub repository: String,
    pub default_branch: String,
    pub tip: String,
    pub upstream: Option<String>,
    pub work: Vec<GitWork>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWork {
    pub branch: String,
    pub tip: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedWork {
    pub issue: String,
    pub branch: String,
    pub since: String,
    pub tip_at_start: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimStart {
    pub issue: String,
    pub claim_key: String,
    pub since: String,
    pub repository: String,
    pub tip: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLanding {
    pub issue: String,
    pub git_commit: String,
    pub project: String,
    pub branch: Option<String>,
    pub landed_in: String,
}

impl View<'_> {
    pub fn git_watch(&self, project: &str) -> Result<Option<GitWatch>, StoreError> {
        let Some(row) = self.row(
            "SELECT repository, default_branch, tip FROM git_watch WHERE project = ?",
            &[project.into()],
        )?
        else {
            return Ok(None);
        };
        let cells = Cells(&row);
        let upstream = self
            .row(
                "SELECT tip FROM git_upstream WHERE project = ?",
                &[project.into()],
            )?
            .map(|row| Cells(&row).text(0));
        let work = self
            .rows(
                "SELECT branch, tip FROM git_work WHERE project = ? ORDER BY branch",
                &[project.into()],
            )?
            .iter()
            .map(|row| {
                let cells = Cells(row);
                GitWork {
                    branch: cells.text(0),
                    tip: cells.text(1),
                }
            })
            .collect();
        Ok(Some(GitWatch {
            repository: cells.text(0),
            default_branch: cells.text(1),
            tip: cells.text(2),
            upstream,
            work,
        }))
    }

    pub fn git_landed(&self, issue: &str, git_commit: &str) -> Result<bool, StoreError> {
        Ok(self
            .row(
                "SELECT 1 FROM git_landings WHERE issue = ? AND git_commit = ?",
                &[issue.into(), git_commit.into()],
            )?
            .is_some())
    }

    pub fn git_landings(&self, project: &str) -> Result<Vec<GitLanding>, StoreError> {
        Ok(self
            .rows(
                "SELECT issue, git_commit, project, branch, landed_in FROM git_landings \
                 WHERE project = ? ORDER BY issue, git_commit",
                &[project.into()],
            )?
            .iter()
            .map(|row| {
                let cells = Cells(row);
                GitLanding {
                    issue: cells.text(0),
                    git_commit: cells.text(1),
                    project: cells.text(2),
                    branch: cells.opt_text(3),
                    landed_in: cells.text(4),
                }
            })
            .collect())
    }

    /// The commits that landed `issue`, in this view.
    pub fn landings_of(&self, issue: &str) -> Result<Vec<GitLanding>, StoreError> {
        Ok(self
            .rows(
                &format!(
                    "SELECT issue, git_commit, project, branch, landed_in FROM {} \
                     WHERE issue = ? ORDER BY git_commit",
                    self.table("git_landings")
                ),
                &[issue.into()],
            )?
            .iter()
            .map(|row| {
                let cells = Cells(row);
                GitLanding {
                    issue: cells.text(0),
                    git_commit: cells.text(1),
                    project: cells.text(2),
                    branch: cells.opt_text(3),
                    landed_in: cells.text(4),
                }
            })
            .collect())
    }

    pub fn claimed_work(
        &self,
        project: &str,
        repository: &str,
    ) -> Result<Vec<ClaimedWork>, StoreError> {
        Ok(self
            .rows(
                "SELECT i.id, i.claim_branch, i.claim_since, c.tip FROM issues i \
                 JOIN claim_starts c ON c.issue = i.id AND c.claim_key = i.claim_key \
                 AND c.since = i.claim_since \
                 WHERE i.project = ? AND c.repository = ? ORDER BY i.number",
                &[project.into(), repository.into()],
            )?
            .iter()
            .map(|row| {
                let cells = Cells(row);
                ClaimedWork {
                    issue: cells.text(0),
                    branch: cells.text(1),
                    since: cells.text(2),
                    tip_at_start: cells.text(3),
                }
            })
            .collect())
    }

    pub fn held_merges(&self) -> Result<Vec<String>, StoreError> {
        Ok(self
            .rows("SELECT issue FROM held_merges ORDER BY issue", &[])?
            .iter()
            .map(|row| Cells(row).text(0))
            .collect())
    }
}

impl Tx<'_> {
    pub fn save_git_watch(&mut self, project: &str, watch: &GitWatch) -> Result<(), StoreError> {
        let connection = self.connection;
        execute(
            connection,
            "DELETE FROM git_work WHERE project = ?",
            &[project.into()],
        )?;
        execute(
            connection,
            "DELETE FROM git_upstream WHERE project = ?",
            &[project.into()],
        )?;
        if let Some(upstream) = &watch.upstream {
            execute(
                connection,
                "INSERT INTO git_upstream (project, tip) VALUES (?, ?)",
                &[project.into(), upstream.as_str().into()],
            )?;
        }
        execute(
            connection,
            "INSERT INTO git_watch (project, repository, default_branch, tip) VALUES (?, ?, ?, ?) \
             ON CONFLICT (project) DO UPDATE SET repository = excluded.repository, \
             default_branch = excluded.default_branch, tip = excluded.tip",
            &[
                project.into(),
                watch.repository.as_str().into(),
                watch.default_branch.as_str().into(),
                watch.tip.as_str().into(),
            ],
        )?;
        for work in &watch.work {
            execute(
                connection,
                "INSERT INTO git_work (project, branch, tip) VALUES (?, ?, ?)",
                &[
                    project.into(),
                    work.branch.as_str().into(),
                    work.tip.as_str().into(),
                ],
            )?;
        }
        Ok(())
    }

    pub fn record_claim_start(&mut self, start: &ClaimStart) -> Result<bool, StoreError> {
        Ok(execute(
            self.connection,
            "INSERT OR REPLACE INTO claim_starts (issue, claim_key, since, repository, tip) \
             SELECT id, claim_key, claim_since, ?, ? FROM issues \
             WHERE id = ? AND claim_key = ? AND claim_since = ?",
            &[
                start.repository.as_str().into(),
                start.tip.as_str().into(),
                start.issue.as_str().into(),
                start.claim_key.as_str().into(),
                start.since.as_str().into(),
            ],
        )? == 1)
    }

    pub fn record_git_landing(&mut self, landing: &GitLanding) -> Result<bool, StoreError> {
        self.require_issue(&landing.issue)?;
        Ok(execute(
            self.connection,
            "INSERT OR IGNORE INTO git_landings (issue, git_commit, project, branch, landed_in) \
             VALUES (?, ?, ?, ?, ?)",
            &[
                landing.issue.as_str().into(),
                landing.git_commit.as_str().into(),
                landing.project.as_str().into(),
                text_param(landing.branch.as_deref()),
                landing.landed_in.as_str().into(),
            ],
        )? == 1)
    }

    pub fn hold_merge(&mut self, issue: &str) -> Result<bool, StoreError> {
        self.require_issue(issue)?;
        Ok(execute(
            self.connection,
            "INSERT OR IGNORE INTO held_merges (issue) VALUES (?)",
            &[issue.into()],
        )? == 1)
    }

    pub fn end_held_merge(&mut self, issue: &str) -> Result<bool, StoreError> {
        Ok(execute(
            self.connection,
            "DELETE FROM held_merges WHERE issue = ?",
            &[issue.into()],
        )? == 1)
    }
}
