use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use acid_cat_core::{Defect, Emitted, EventDefinition, Subscription};
use futures::StreamExt;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::{self, error::RecvError};

use super::board::{Board, all_issues};
use super::model::{self, IssueSummary};
use super::{Context, Refused};
use crate::domain::{Actor, Caller};
use crate::store::{self, AsOf, Commit, IssueFilter, Subject, View};

const MOST_ISSUES: usize = 500;
const MOST_FIELDS: usize = 50;
const HISTORY_PAGE: usize = 200;

#[derive(Deserialize)]
pub(super) struct ChangedInput {
    project: Option<String>,
    issue: Option<String>,
}

#[derive(Serialize)]
pub(super) struct Payload {
    commit: String,
    at: String,
    author: Actor,
    operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    issues: Vec<ChangedIssue>,
    projects: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cut: Option<bool>,
}

#[derive(Serialize)]
struct ChangedIssue {
    issue: IssueSummary,
    fields: Vec<String>,
}

pub(super) fn event() -> EventDefinition<Context, ChangedInput, Payload> {
    super::catalog::event::<ChangedInput, Payload>("issue.changed").subscribe(follow)
}

#[derive(Clone, Debug, PartialEq)]
struct Mark {
    hash: String,
    at: String,
}

type Computed = BTreeMap<String, IssueSummary>;

struct Follower {
    context: Arc<Context>,
    input: ChangedInput,
    commits: broadcast::Receiver<Commit>,
    max_age_ms: Option<u64>,
    phase: Phase,
    continuity: Continuity,
    last: Option<Mark>,
    computed: Option<(String, Computed)>,
}

enum Phase {
    Initial { cursor: Option<String> },
    Replaying { pending: VecDeque<Mark> },
    Live,
}

enum Continuity {
    Unbroken,
    AfterGap,
}

fn follow(
    input: ChangedInput,
    subscription: Subscription<Context>,
) -> BoxStream<'static, Result<Emitted<Payload>, Defect>> {
    let context = subscription.context_arc();
    let commits = match context.store() {
        Ok(store) => store.subscribe(),
        Err(refused) => {
            return futures::stream::once(async move { Err(Defect::new(format!("{refused:?}"))) })
                .boxed();
        }
    };
    let follower = Follower {
        commits,
        context,
        input,
        max_age_ms: subscription.max_age_ms(),
        phase: Phase::Initial {
            cursor: subscription.cursor().map(str::to_owned),
        },
        continuity: Continuity::Unbroken,
        last: None,
        computed: None,
    };
    futures::stream::unfold(follower, |mut follower| async move {
        match follower.next().await {
            Ok(Some(emitted)) => Some((Ok(emitted), follower)),
            Ok(None) => None,
            Err(refused) => Some((Err(Defect::new(format!("{refused:?}"))), follower)),
        }
    })
    .boxed()
}

impl Follower {
    async fn next(&mut self) -> Result<Option<Emitted<Payload>>, Refused> {
        loop {
            let mark = match &mut self.phase {
                Phase::Initial { cursor } => {
                    let cursor = cursor.take();
                    self.phase = self.resume(cursor).await?;
                    continue;
                }
                Phase::Replaying { pending } => match pending.pop_front() {
                    Some(mark) => mark,
                    None => {
                        self.phase = Phase::Live;
                        continue;
                    }
                },
                Phase::Live => match self.receive().await {
                    Some(mark) => mark,
                    None => return Ok(None),
                },
            };
            if let Some(emitted) = self.deliver(mark).await? {
                return Ok(Some(emitted));
            }
        }
    }

    async fn resume(&mut self, cursor: Option<String>) -> Result<Phase, Refused> {
        let Some(cursor) = cursor else {
            return Ok(Phase::Live);
        };
        let start = match self.read(AsOf::Commit(cursor), head).await {
            Ok(start) => start,
            Err(Refused::Failure(_)) => {
                self.continuity = Continuity::AfterGap;
                return Ok(Phase::Live);
            }
            Err(other) => return Err(other),
        };
        let since = start.as_ref().map(|mark| mark.at.clone());
        let later = self
            .read(AsOf::Now, move |view| marks_after(view, since.as_deref()))
            .await?;
        self.last = start;
        Ok(Phase::Replaying {
            pending: later.into(),
        })
    }

    async fn receive(&mut self) -> Option<Mark> {
        loop {
            match self.commits.recv().await {
                Ok(commit) => {
                    let newer = self.last.as_ref().is_none_or(|last| commit.at > last.at);
                    if newer {
                        return Some(Mark {
                            hash: commit.hash,
                            at: commit.at,
                        });
                    }
                }
                Err(RecvError::Lagged(_)) => self.continuity = Continuity::AfterGap,
                Err(RecvError::Closed) => return None,
            }
        }
    }

    async fn deliver(&mut self, mark: Mark) -> Result<Option<Emitted<Payload>>, Refused> {
        if self.too_old(&mark) {
            self.continuity = Continuity::AfterGap;
            self.last = Some(mark);
            return Ok(None);
        }
        let payload = self.occurrence(&mark).await?;
        self.last = Some(mark.clone());
        let Some(payload) = payload else {
            return Ok(None);
        };
        let emitted = Emitted::new(payload)
            .event_id(&mark.hash)
            .timestamp(&mark.at)
            .cursor(&mark.hash);
        Ok(Some(
            match std::mem::replace(&mut self.continuity, Continuity::Unbroken) {
                Continuity::AfterGap => emitted.truncated(),
                Continuity::Unbroken => emitted,
            },
        ))
    }

    fn too_old(&self, mark: &Mark) -> bool {
        let Some(max_age_ms) = self.max_age_ms else {
            return false;
        };
        let age = jiff::SignedDuration::from_millis(i64::try_from(max_age_ms).unwrap_or(i64::MAX));
        let oldest = jiff::Timestamp::now()
            .checked_sub(age)
            .unwrap_or(jiff::Timestamp::MIN);
        mark.at < oldest.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }

    async fn read<T, F>(&self, as_of: AsOf, work: F) -> Result<T, Refused>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&View<'c>) -> Result<T, Refused> + Send + 'static,
    {
        self.context.read(as_of, work).await
    }

    async fn computed_at(&self, commit: Option<String>) -> Result<Computed, Refused> {
        let Some(commit) = commit else {
            return Ok(Computed::new());
        };
        let caller = self.context.caller.clone();
        self.read(AsOf::Commit(commit), move |view| computed(view, &caller))
            .await
    }

    async fn occurrence(&mut self, mark: &Mark) -> Result<Option<Payload>, Refused> {
        let caller = self.context.caller.clone();
        let hash = mark.hash.clone();
        let (changes, previous, after) = self
            .read(AsOf::Commit(hash.clone()), move |view| {
                let (changes, previous) = changes_of(view, &hash)?;
                Ok((changes, previous, computed(view, &caller)?))
            })
            .await?;
        let before = match self.computed.take() {
            Some((commit, computed)) if previous.as_ref() == Some(&commit) => computed,
            _ => self.computed_at(previous).await?,
        };
        let payload = self.payload(mark, &changes, &before, &after)?;
        self.computed = Some((mark.hash.clone(), after));
        Ok(payload)
    }

    fn payload(
        &self,
        mark: &Mark,
        changes: &[store::Change],
        before: &Computed,
        after: &Computed,
    ) -> Result<Option<Payload>, Refused> {
        let Some(first) = changes.first() else {
            return Ok(None);
        };
        let mut fields: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        let mut projects: BTreeSet<String> = BTreeSet::new();
        for change in changes {
            match &change.issue {
                Some(issue) => fields
                    .entry(issue)
                    .or_default()
                    .extend(change.fields.iter().map(|field| field.field.clone())),
                None => {
                    projects.insert(change.project.clone());
                }
            }
        }
        for (id, summary) in after {
            let was = before.get(id);
            let computed = [
                (
                    "category",
                    was.is_none_or(|was| was.category != summary.category),
                ),
                ("ready", was.is_none_or(|was| was.ready != summary.ready)),
                (
                    "blockedBy",
                    was.is_none_or(|was| was.blocked_by != summary.blocked_by),
                ),
                (
                    "notReady",
                    was.is_none_or(|was| was.not_ready != summary.not_ready),
                ),
            ];
            for (field, changed) in computed {
                if changed {
                    fields.entry(id).or_default().insert(field.into());
                }
            }
        }
        let mut issues: Vec<ChangedIssue> = fields
            .into_iter()
            .filter_map(|(id, names)| {
                let issue = after.get(id)?.clone();
                Some(ChangedIssue {
                    issue,
                    fields: names.into_iter().take(MOST_FIELDS).collect(),
                })
            })
            .filter(|changed| self.wants_issue(&changed.issue))
            .collect();
        let projects: Vec<String> = match (&self.input.issue, &self.input.project) {
            (Some(_), _) => Vec::new(),
            (None, Some(project)) => projects.into_iter().filter(|key| key == project).collect(),
            (None, None) => projects.into_iter().collect(),
        };
        if issues.is_empty() && projects.is_empty() {
            return Ok(None);
        }
        let cut = (issues.len() > MOST_ISSUES).then_some(true);
        issues.truncate(MOST_ISSUES);
        Ok(Some(Payload {
            commit: mark.hash.clone(),
            at: mark.at.clone(),
            author: model::actor(&first.author)?,
            operation: first.operation.clone(),
            reason: first.reason.clone(),
            issues,
            projects,
            cut,
        }))
    }

    fn wants_issue(&self, issue: &IssueSummary) -> bool {
        self.input.issue.as_ref().is_none_or(|id| *id == issue.id)
            && self
                .input
                .project
                .as_ref()
                .is_none_or(|key| *key == issue.project)
    }
}

fn head(view: &View<'_>) -> Result<Option<Mark>, Refused> {
    Ok(view
        .history(&Subject::Board, None, 1)?
        .items
        .into_iter()
        .next()
        .map(|change| Mark {
            hash: change.commit,
            at: change.at,
        }))
}

fn marks_after(view: &View<'_>, since: Option<&str>) -> Result<Vec<Mark>, Refused> {
    let mut marks: Vec<Mark> = Vec::new();
    let mut cursor: Option<String> = None;
    'pages: loop {
        let page = view.history(&Subject::Board, cursor.as_deref(), HISTORY_PAGE)?;
        for change in page.items {
            if since.is_some_and(|since| change.at.as_str() <= since) {
                break 'pages;
            }
            if marks.last().is_none_or(|mark| mark.hash != change.commit) {
                marks.push(Mark {
                    hash: change.commit,
                    at: change.at,
                });
            }
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    marks.reverse();
    Ok(marks)
}

fn changes_of(
    view: &View<'_>,
    hash: &str,
) -> Result<(Vec<store::Change>, Option<String>), Refused> {
    let mut changes = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = view.history(&Subject::Board, cursor.as_deref(), HISTORY_PAGE)?;
        for change in page.items {
            if change.commit != hash {
                return Ok((changes, Some(change.commit)));
            }
            changes.push(change);
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok((changes, None)),
        }
    }
}

fn computed(view: &View<'_>, caller: &Caller) -> Result<Computed, Refused> {
    let board = Board::load(view)?;
    all_issues(view, &IssueFilter::default())?
        .iter()
        .map(|record| Ok((record.id.clone(), board.summary(record, caller)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use acid_cat_core::{Outcome, Session, SubscribeOptions};
    use jiff::{SignedDuration, Timestamp};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use crate::app::{Context, application};
    use crate::domain::{Caller, WorkingCopy};
    use crate::store::{Author, Call, NewIssue, Store};

    const HUMAN: &str = r#"{"kind":"human","name":"sam"}"#;
    const HANG_GUARD: Duration = Duration::from_secs(60);
    // Far longer than the hang guard: an occurrence that arrives at all is younger than this, however slow the machine.
    const MAX_AGE: SignedDuration = SignedDuration::from_hours(1);

    async fn create(store: &Store, title: &str) {
        let issue = NewIssue {
            project: "DEM".into(),
            title: title.into(),
            status: "todo".into(),
            created_by: HUMAN.into(),
            ..NewIssue::default()
        };
        let call = Call::new(Author::new(HUMAN).expect("an author"), "issue.create");
        store
            .write(call, move |tx| tx.create_issue(&issue).map(|_| ()))
            .await
            .expect("the write ends");
    }

    #[tokio::test]
    async fn a_live_occurrence_older_than_max_age_is_skipped_and_the_next_one_is_truncated() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(home.path().join("workspace.db")).expect("the store opens");
        let human = Caller {
            actor: serde_json::from_str(HUMAN).expect("an actor"),
            working_copy: WorkingCopy::default(),
        };
        let app = application().expect("the application");
        let session = Session::open(&app, "cli", Arc::new(Context::new(store.clone(), human)));
        let created = session
            .invoke(
                "project.create",
                json!({ "key": "DEM", "name": "Demo" }),
                &CancellationToken::new(),
            )
            .await;
        assert!(matches!(created, Outcome::Value(_)), "{created:?}");
        let mut live = session
            .subscribe(
                "issue.changed",
                json!({}),
                SubscribeOptions {
                    max_age_ms: Some(MAX_AGE.as_secs_f64() * 1000.0),
                    ..SubscribeOptions::default()
                },
            )
            .expect("the subscription starts");

        store.date_next_commit(Timestamp::now() - MAX_AGE * 2);
        create(&store, "Aged").await;
        create(&store, "Fresh").await;

        let first = tokio::time::timeout(HANG_GUARD, live.next())
            .await
            .expect("an occurrence within the hang guard")
            .expect("the subscription goes on")
            .expect("an occurrence");
        assert_eq!(first.data["issues"][0]["issue"]["title"], "Fresh");
        assert!(first.truncated, "{first:?}");
    }
}
