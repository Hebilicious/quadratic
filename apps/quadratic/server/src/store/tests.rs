use super::{
    AsOf, Author, Call, NewIssue, Project, Store, StoreError, Subject, Workflow, WorkflowStatus,
};

fn project(key: &str) -> Project {
    Project {
        key: key.into(),
        name: format!("Project {key}"),
        repository: None,
        default_branch: "main".into(),
        extra: "{}".into(),
        workflow: Workflow {
            statuses: vec![WorkflowStatus {
                key: "todo".into(),
                name: "Todo".into(),
                category: "unstarted".into(),
            }],
            moves: r#"{"claim":"todo","review":"todo","merge":"todo","release":"todo"}"#.into(),
        },
    }
}

async fn create(store: &Store, key: &str) -> Result<Option<String>, StoreError> {
    let author = Author::new(r#"{"kind":"human","name":"sam"}"#)?;
    let project = project(key);
    let written = store
        .write(Call::new(author, "project.create"), move |tx| {
            tx.create_project(&project)
        })
        .await?;
    Ok(written.commit.map(|commit| commit.hash))
}

#[tokio::test]
async fn a_commit_whose_indexing_fails_once_reaches_the_history_and_the_subscribers() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let store = Store::open(directory.path().join("workspace.db")).expect("the store opens");
    let mut commits = store.subscribe();

    store.fail_next_index();
    let written = create(&store, "DEM").await;

    let history = store
        .read(AsOf::Now, |view| view.history(&Subject::Board, None, 10))
        .await
        .expect("the history is readable");
    let hash = written
        .expect("the durable write succeeds")
        .expect("a commit");
    assert_eq!(
        history.items.first().map(|change| &change.commit),
        Some(&hash)
    );
    let published = commits
        .try_recv()
        .expect("the commit reached the subscriber");
    assert_eq!(published.hash, hash);
    assert_eq!(published.projects, vec!["DEM".to_owned()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_never_sees_a_commit_that_its_history_does_not_list_yet() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let store = Store::open(directory.path().join("workspace.db")).expect("the store opens");
    create(&store, "DEM")
        .await
        .expect("the first project is created");
    let (sealed, resume) = store.pause_after_next_seal();
    let writing = tokio::spawn({
        let store = store.clone();
        async move { create(&store, "ABC").await }
    });
    tokio::task::spawn_blocking(move || sealed.recv_timeout(std::time::Duration::from_secs(60)))
        .await
        .expect("the wait for the seal ends")
        .expect("the write is sealed");

    let seen = store
        .read(AsOf::Now, |view| {
            let project = view.project("ABC")?.is_some();
            let history = view.history(&Subject::Project("ABC".into()), None, 10)?;
            Ok::<_, StoreError>((project, !history.items.is_empty()))
        })
        .await;
    resume.send(()).expect("the writer waits");
    writing
        .await
        .expect("the write's task ends")
        .expect("the second project is created");

    let (project, listed) = seen.expect("the read succeeds");
    assert_eq!(
        project, listed,
        "the read sees the project: {project}, its history lists the commit: {listed}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_store_made_before_notes_were_a_list_keeps_its_notes_text_as_the_first_note() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("workspace.db");
    let store = Store::open(&path).expect("the store opens");
    create(&store, "DEM").await.expect("the project");
    let author = Author::new(r#"{"kind":"human","name":"sam"}"#).expect("an author");
    store
        .write(Call::new(author, "issue.create"), |tx| {
            tx.create_issue(&NewIssue {
                project: "DEM".into(),
                title: "Old issue".into(),
                status: "todo".into(),
                created_by: r#"{"kind":"human","name":"sam"}"#.into(),
                ..NewIssue::default()
            })?;
            tx.connection.execute_batch(
                "DROP TABLE notes; UPDATE issues SET notes = 'Written before the list.'; \
                 PRAGMA user_version = 0",
            )?;
            Ok::<_, StoreError>(())
        })
        .await
        .expect("an issue as a store before the change wrote it");
    let before = store
        .read(AsOf::Now, |view| view.history(&Subject::Board, None, 1))
        .await
        .expect("the history")
        .items[0]
        .commit
        .clone();
    drop(store);

    let store = Store::open(&path).expect("the store made before the change opens");

    let notes = store
        .read(AsOf::Now, |view| view.notes("DEM-1", 50))
        .await
        .expect("the notes");
    let texts: Vec<&str> = notes.iter().map(|note| note.text.as_str()).collect();
    assert_eq!(texts, ["Written before the list."]);
    let then = store
        .read(AsOf::Commit(before), |view| view.notes("DEM-1", 50))
        .await
        .expect("the notes as of a commit before the change");
    assert!(then.is_empty(), "{then:?}");
}

#[tokio::test]
async fn a_store_made_before_findings_opens_with_issues_that_have_no_source_and_takes_one() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("workspace.db");
    let store = Store::open(&path).expect("the store opens");
    create(&store, "DEM").await.expect("the project");
    let author = Author::new(r#"{"kind":"human","name":"sam"}"#).expect("an author");
    store
        .write(Call::new(author, "issue.create"), |tx| {
            tx.create_issue(&NewIssue {
                project: "DEM".into(),
                title: "Old issue".into(),
                status: "todo".into(),
                created_by: r#"{"kind":"human","name":"sam"}"#.into(),
                ..NewIssue::default()
            })?;
            tx.connection.execute_batch(
                "DROP TABLE sources; DROP TABLE finding_links; DROP TABLE finding_blocks; \
                 PRAGMA user_version = 0",
            )?;
            Ok::<_, StoreError>(())
        })
        .await
        .expect("an issue as a store before the change wrote it");
    let before = store
        .read(AsOf::Now, |view| view.history(&Subject::Board, None, 1))
        .await
        .expect("the history")
        .items[0]
        .commit
        .clone();
    drop(store);

    let store = Store::open(&path).expect("the store made before the change opens");

    let author = Author::new(r#"{"kind":"human","name":"sam"}"#).expect("an author");
    store
        .write(Call::new(author, "findings.import"), |tx| {
            tx.set_source("DEM-1", "linter", "L204 f:49f3eaf2628a1f35", false)
        })
        .await
        .expect("the old issue takes a source");
    let now = store
        .read(AsOf::Now, |view| view.source("DEM-1"))
        .await
        .expect("the source");
    assert_eq!(
        now.map(|source| source.key),
        Some("L204 f:49f3eaf2628a1f35".to_owned())
    );
    let then = store
        .read(AsOf::Commit(before), |view| view.source("DEM-1"))
        .await
        .expect("the source as of a commit before the change");
    assert_eq!(then, None);
}

const TEST_COLUMN: super::migrate::Migration = super::migrate::Migration {
    name: "test only: issues take a mood",
    sql: "ALTER TABLE issues ADD COLUMN mood TEXT",
};

async fn issue(store: &Store, title: &str) {
    let author = Author::new(r#"{"kind":"human","name":"sam"}"#).expect("an author");
    let title = title.to_owned();
    store
        .write(Call::new(author, "issue.create"), move |tx| {
            tx.create_issue(&NewIssue {
                project: "DEM".into(),
                title,
                status: "todo".into(),
                created_by: r#"{"kind":"human","name":"sam"}"#.into(),
                ..NewIssue::default()
            })
        })
        .await
        .expect("the issue is created");
}

async fn raw(store: &Store, sql: &'static str) {
    let author = Author::new(r#"{"kind":"human","name":"sam"}"#).expect("an author");
    store
        .write(Call::new(author, "test.raw"), move |tx| {
            tx.connection.execute_batch(sql).map_err(StoreError::from)
        })
        .await
        .expect("the raw write");
}

async fn version_and_head(store: &Store) -> (i64, String, String) {
    store
        .read(AsOf::Now, |view| {
            let version = view.row("PRAGMA user_version", &[])?.expect("a version");
            let head = view
                .row("SELECT committer, message FROM dolt_log LIMIT 1", &[])?
                .expect("a commit");
            Ok::<_, StoreError>((
                super::engine::Cells(&version).int(0),
                super::engine::Cells(&head).text(0),
                super::engine::Cells(&head).text(1),
            ))
        })
        .await
        .expect("the version and the newest commit")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_store_made_before_schema_versions_is_adopted_as_version_1_by_a_store_commit_keeping_its_rows()
 {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("workspace.db");
    let store = Store::open(&path).expect("the store opens");
    create(&store, "DEM").await.expect("the project");
    issue(&store, "First").await;
    issue(&store, "Second").await;
    raw(&store, "DROP TABLE git_upstream; PRAGMA user_version = 0").await;
    drop(store);

    let store = Store::open(&path).expect("the store made before versions opens");

    let (version, committer, message) = version_and_head(&store).await;
    assert_eq!(version, 1);
    assert!(committer.starts_with("quadratic"), "{committer}");
    assert!(
        message.starts_with("store.migrate\n") && message.contains("schema version 1"),
        "{message}"
    );
    let titles = store
        .read(AsOf::Now, |view| {
            view.issues(&super::IssueFilter::default(), None, 10)
        })
        .await
        .expect("the issues")
        .items
        .into_iter()
        .map(|issue| issue.title)
        .collect::<Vec<_>>();
    assert_eq!(titles, ["First", "Second"]);
    let upstream = store
        .read(AsOf::Now, |view| {
            view.rows("SELECT count(*) FROM git_upstream", &[])
        })
        .await;
    assert!(upstream.is_ok(), "{upstream:?}");
}

const COMMITS_AND_CHANGES: &str = "SELECT commit_hash FROM dolt_log \
     UNION ALL SELECT 'changed ' || table_name FROM dolt_status";

async fn commits_and_changes(store: &Store) -> Vec<String> {
    store
        .read(AsOf::Now, |view| view.rows(COMMITS_AND_CHANGES, &[]))
        .await
        .expect("the commits")
        .iter()
        .map(|row| super::engine::Cells(row).text(0))
        .collect()
}

fn commits_and_changes_in(path: &std::path::Path) -> Vec<String> {
    let connection = super::engine::open_reader(path).expect("a reader");
    super::engine::query(&connection, COMMITS_AND_CHANGES, &[])
        .expect("the commits")
        .iter()
        .map(|row| super::engine::Cells(row).text(0))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_store_at_the_current_version_opens_without_writing() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("workspace.db");
    let store = Store::open(&path).expect("the store opens");
    create(&store, "DEM").await.expect("the project");
    let before = commits_and_changes(&store).await;
    drop(store);

    let store = Store::open(&path).expect("the store reopens");

    assert_eq!(commits_and_changes(&store).await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_store_newer_than_the_binary_is_refused_and_left_as_it_was() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("workspace.db");
    let store = Store::open_with(&path, &[super::migrate::MIGRATIONS[0], TEST_COLUMN])
        .expect("a store of a newer binary");
    let before = commits_and_changes(&store).await;
    drop(store);

    let refused = Store::open(&path).err();

    assert!(
        matches!(
            &refused,
            Some(StoreError::NewerStore {
                found: 2,
                known: 1,
                ..
            })
        ),
        "{refused:?}"
    );
    assert_eq!(commits_and_changes_in(&path), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_issue_and_its_history_read_as_of_a_commit_before_a_new_column() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("workspace.db");
    let store = Store::open(&path).expect("the store opens");
    create(&store, "DEM").await.expect("the project");
    issue(&store, "Before the column").await;
    let before = store
        .read(AsOf::Now, |view| view.history(&Subject::Board, None, 1))
        .await
        .expect("the history")
        .items[0]
        .commit
        .clone();
    drop(store);
    let store = Store::open_with(&path, &[super::migrate::MIGRATIONS[0], TEST_COLUMN])
        .expect("the store migrates");
    raw(
        &store,
        "UPDATE issues SET mood = 'calm', title = 'After the column'",
    )
    .await;

    let then = store
        .read(AsOf::Commit(before.clone()), move |view| {
            let issue = view.issue("DEM-1")?.expect("the issue then");
            let mood = view.row(
                &format!(
                    "SELECT mood FROM {} WHERE id = 'DEM-1'",
                    view.table("issues")
                ),
                &[],
            )?;
            let history = view.history(&Subject::Issue("DEM-1".into()), None, 10)?;
            Ok::<_, StoreError>((issue, mood, history))
        })
        .await
        .expect("the reads as of a commit before the column");
    let now = store
        .read(AsOf::Now, |view| {
            view.history(&Subject::Issue("DEM-1".into()), None, 10)
        })
        .await
        .expect("the history now");

    let (issue, mood, history) = then;
    assert_eq!(issue.title, "Before the column");
    assert_eq!(
        mood.map(|row| super::engine::Cells(&row).value(0)),
        Some(doltlite::Value::Null)
    );
    assert_eq!(history.items.len(), 1, "{history:?}");
    let operations: Vec<&str> = now
        .items
        .iter()
        .map(|change| change.operation.as_str())
        .collect();
    assert_eq!(operations, ["test.raw", "issue.create"], "{now:?}");
}

fn commit_outside_the_store(path: &std::path::Path, sql: &str, operation: &str, at: &str) {
    let connection = doltlite::Connection::open(path).expect("the store file");
    connection
        .execute_batch(&format!("{sql}; PRAGMA user_version = 0"))
        .expect("the change");
    let message = super::message::Message {
        operation: operation.into(),
        actor: r#"{"kind":"human","name":"sam"}"#.into(),
        reason: None,
        branch: None,
        worktree: None,
        at: at.into(),
    }
    .encode();
    connection
        .query_row(
            "SELECT dolt_commit('-Am', ?, '--author', 'sam <actor@quadratic.local>')",
            &[message.into()],
        )
        .expect("a commit the index has not seen");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_commit_an_older_binary_left_unindexed_reaches_the_history_when_the_store_migrates() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("workspace.db");
    let store = Store::open(&path).expect("the store opens");
    create(&store, "DEM").await.expect("the project");
    drop(store);
    commit_outside_the_store(
        &path,
        "UPDATE projects SET name = 'Renamed' WHERE key = 'DEM'",
        "project.update",
        "2026-11-20T12:00:00.000Z",
    );

    let store = Store::open(&path).expect("the store migrates");

    let history = store
        .read(AsOf::Now, |view| {
            view.history(&Subject::Project("DEM".into()), None, 10)
        })
        .await
        .expect("the history");
    let operations: Vec<&str> = history
        .items
        .iter()
        .map(|change| change.operation.as_str())
        .collect();
    assert_eq!(operations, ["project.update", "project.create"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_migration_commit_comes_after_its_parent_even_when_the_clock_is_behind() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("workspace.db");
    let store = Store::open(&path).expect("the store opens");
    create(&store, "DEM").await.expect("the project");
    drop(store);
    let future = "2999-01-01T00:00:00.000Z";
    commit_outside_the_store(
        &path,
        "UPDATE projects SET name = 'Renamed' WHERE key = 'DEM'",
        "project.update",
        future,
    );

    let store = Store::open(&path).expect("the store migrates");

    let times = store
        .read(AsOf::Now, |view| {
            view.rows("SELECT at FROM commit_index ORDER BY seq DESC LIMIT 2", &[])
        })
        .await
        .expect("the newest two commits")
        .iter()
        .map(|row| super::engine::Cells(row).text(0))
        .collect::<Vec<_>>();
    assert_eq!(times[1], future);
    assert!(times[0] > times[1], "{times:?}");
}

const KILLED_WRITER: &str = "QUADRATIC_TEST_KILLED_WRITER";
const KILLED_WRITER_TEST: &str = "store::tests::a_store_ended_with_sigkill_while_a_write_is_in_flight_keeps_every_answered_write";

fn write_until_killed(path: &str) -> ! {
    use std::io::Write;

    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    runtime.block_on(async {
        let store = Store::open(path).expect("the store opens");
        create(&store, "DEM").await.expect("the answered write");
        println!("answered");
        let (sealed, _resume) = store.pause_after_next_seal();
        let writer = store.clone();
        let _in_flight = tokio::spawn(async move { create(&writer, "ABC").await });
        tokio::task::spawn_blocking(move || sealed.recv())
            .await
            .expect("the waiting thread")
            .expect("the write is sealed");
        println!("sealed");
        std::io::stdout()
            .flush()
            .expect("the parent reads the lines");
        std::future::pending::<()>().await;
    });
    unreachable!("the parent ends this process")
}

#[test]
fn a_store_ended_with_sigkill_while_a_write_is_in_flight_keeps_every_answered_write() {
    use std::io::BufRead;

    if let Ok(path) = std::env::var(KILLED_WRITER) {
        write_until_killed(&path);
    }
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("workspace.db");
    let mut child = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--exact",
            KILLED_WRITER_TEST,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(KILLED_WRITER, &path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the writer process starts");
    let output = child.stdout.take().expect("the writer's output");
    let (lines, seen) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(output)
            .lines()
            .map_while(Result::ok)
        {
            let _ = lines.send(line);
        }
    });
    let mut answered = false;
    loop {
        let line = seen
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the writer reaches its in-flight write");
        answered |= line.ends_with("answered");
        if line.ends_with("sealed") {
            break;
        }
    }

    child.kill().expect("SIGKILL reaches the writer");
    child.wait().expect("the writer ends");
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let store = Store::open(&path).expect("the store of a killed writer opens");

    let (projects, history) = runtime
        .block_on(store.read(AsOf::Now, |view| {
            let projects =
                ["DEM", "ABC"].map(|key| view.project(key).map(|project| project.is_some()));
            let history = view.history(&Subject::Board, None, 10)?;
            Ok::<_, StoreError>((projects, history))
        }))
        .expect("the reads");
    let listed: Vec<&str> = history
        .items
        .iter()
        .flat_map(|change| change.project.as_str().split(','))
        .collect();
    assert!(answered);
    assert!(
        matches!(projects[0], Ok(true)),
        "the answered write is lost"
    );
    assert!(listed.contains(&"DEM"), "{listed:?}");
    let in_flight = matches!(projects[1], Ok(true));
    assert_eq!(in_flight, listed.contains(&"ABC"), "{listed:?}");
}
