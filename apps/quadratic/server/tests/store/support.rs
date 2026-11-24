use std::time::Duration;

use quadratic_server::store::{
    AsOf, Author, Call, Change, NewIssue, Project, Store, StoreError, Subject, Workflow,
    WorkflowStatus, Written,
};
use tempfile::TempDir;

pub const HANG_GUARD: Duration = Duration::from_secs(60);

pub struct Board {
    pub store: Store,
    _directory: TempDir,
}

pub fn open() -> Board {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let store = Store::open(directory.path().join("workspace.db")).expect("the store opens");
    Board {
        store,
        _directory: directory,
    }
}

pub fn actor(name: &str) -> String {
    serde_json::json!({ "harness": "claude-code", "kind": "agent", "name": name }).to_string()
}

pub fn call(name: &str, operation: &str) -> Call {
    Call::new(Author::new(&actor(name)).expect("a valid actor"), operation)
}

pub fn workflow() -> Workflow {
    let status = |key: &str, name: &str, category: &str| WorkflowStatus {
        key: key.into(),
        name: name.into(),
        category: category.into(),
    };
    Workflow {
        statuses: vec![
            status("backlog", "Backlog", "backlog"),
            status("todo", "Todo", "unstarted"),
            status("in-progress", "In progress", "started"),
            status("in-review", "In review", "started"),
            status("merged", "Merged", "started"),
            status("done", "Done", "completed"),
            status("canceled", "Canceled", "canceled"),
        ],
        moves: r#"{"claim":"in-progress","review":"in-review","merge":"merged","release":"todo"}"#
            .into(),
    }
}

impl Board {
    pub fn reopen(self) -> Board {
        let path = self.store.path().to_path_buf();
        drop(self.store);
        Board {
            store: Store::open(path).expect("the store reopens"),
            _directory: self._directory,
        }
    }

    pub async fn project(&self, key: &str) {
        let project = Project {
            key: key.into(),
            name: format!("Project {key}"),
            repository: None,
            default_branch: "main".into(),
            extra: "{}".into(),
            workflow: workflow(),
        };
        self.store
            .write(call("human", "project.create"), move |tx| {
                tx.create_project(&project)
            })
            .await
            .expect("the project is created");
    }

    pub async fn issue(&self, project: &str, title: &str) -> Written<String> {
        let issue = NewIssue {
            project: project.into(),
            title: title.into(),
            status: "todo".into(),
            created_by: actor("human"),
            ..NewIssue::default()
        };
        self.store
            .write(call("human", "issue.create"), move |tx| {
                tx.create_issue(&issue)
                    .map(|created| created.id().to_owned())
            })
            .await
            .expect("the issue is created")
    }

    pub async fn history(&self, subject: Subject) -> Vec<Change> {
        self.store
            .read(AsOf::Now, move |view| view.history(&subject, None, 200))
            .await
            .map(|page: quadratic_server::store::Page<Change>| page.items)
            .expect("the history is readable")
    }

    pub async fn title(&self, as_of: AsOf, id: &str) -> Result<Option<String>, StoreError> {
        let id = id.to_owned();
        self.store
            .read(as_of, move |view| {
                Ok(view.issue(&id)?.map(|issue| issue.title))
            })
            .await
    }
}
