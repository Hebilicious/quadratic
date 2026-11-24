use quadratique_server::store::{Store, StoreError};

fn assert_refused_inside_a_git_work_tree(directory: &std::path::Path) {
    let path = directory.join("nested").join("workspace.db");

    let refused = Store::open(&path).err();

    assert!(
        matches!(&refused, Some(StoreError::InsideGitWorkTree(_))),
        "{refused:?}"
    );
    assert!(!directory.join("nested").exists());
}

fn git(directory: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=tester",
            "-c",
            "user.email=tester@example.invalid",
        ])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(directory)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

#[test]
fn a_database_inside_a_git_working_tree_is_refused() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    git(directory.path(), &["init", "--quiet"]);

    assert_refused_inside_a_git_work_tree(directory.path());
}

#[test]
fn a_database_inside_a_git_folder_whose_head_links_to_an_unborn_branch_is_refused() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let dot_git = directory.path().join(".git");
    std::fs::create_dir(&dot_git).expect("a .git folder");
    std::os::unix::fs::symlink("refs/heads/main", dot_git.join("HEAD")).expect("a HEAD link");

    assert_refused_inside_a_git_work_tree(directory.path());
}

#[test]
fn a_database_inside_a_linked_git_worktree_is_refused() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let main = directory.path().join("main");
    std::fs::create_dir(&main).expect("the main worktree");
    git(&main, &["init", "--quiet"]);
    git(
        &main,
        &["commit", "--quiet", "--allow-empty", "-m", "first"],
    );
    git(&main, &["worktree", "add", "--quiet", "../linked"]);
    let linked = directory.path().join("linked");
    assert!(linked.join(".git").is_file(), "git wrote no .git file");

    assert_refused_inside_a_git_work_tree(&linked);
}

#[test]
fn a_database_under_an_empty_git_folder_is_opened() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    std::fs::create_dir(directory.path().join(".git")).expect("an empty .git folder");
    let path = directory.path().join("nested").join("workspace.db");

    let opened = Store::open(&path);

    assert!(opened.is_ok(), "{:?}", opened.err());
}

fn refusal_of(contents: &[u8]) -> (std::path::PathBuf, Option<StoreError>) {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("workspace.db");
    std::fs::write(&path, contents).expect("the file");
    let refused = Store::open(&path).err();
    (path.canonicalize().expect("the file's path"), refused)
}

fn assert_damaged(path: &std::path::Path, refused: Option<StoreError>) {
    match refused {
        Some(error @ StoreError::Damaged { .. }) => {
            let message = error.to_string();
            assert!(
                message.contains(&path.display().to_string()) && message.contains("Backups"),
                "{message}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_file_that_is_not_a_database_is_refused_as_damaged_naming_the_file_and_the_restore() {
    let (path, refused) = refusal_of(&b"notes, not a database\n".repeat(200));

    assert_damaged(&path, refused);
}

#[test]
fn a_store_with_overwritten_bytes_is_refused_as_damaged_naming_the_file_and_the_restore() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("workspace.db");
    let store = doltlite::Connection::open(&path).expect("a doltlite store");
    store
        .execute_batch("CREATE TABLE t (id TEXT PRIMARY KEY); INSERT INTO t VALUES ('x')")
        .expect("a row");
    store
        .query_row("SELECT dolt_commit('-Am', 'a row')", &[])
        .expect("a commit");
    drop(store);
    let mut bytes = std::fs::read(&path).expect("the store file");
    let third = bytes.len() / 3;
    for byte in &mut bytes[third..2 * third] {
        *byte ^= 0x5a;
    }

    let (path, refused) = refusal_of(&bytes);

    assert_damaged(&path, refused);
}

#[test]
fn a_plain_sqlite_database_is_refused_as_damaged_naming_the_file_and_the_restore() {
    let mut empty_database = vec![0_u8; 512];
    empty_database[..16].copy_from_slice(b"SQLite format 3\0");
    empty_database[16..24].copy_from_slice(&[0x02, 0x00, 1, 1, 0, 64, 32, 32]);
    empty_database[24..28].copy_from_slice(&1_u32.to_be_bytes());
    empty_database[28..32].copy_from_slice(&1_u32.to_be_bytes());
    empty_database[56..60].copy_from_slice(&1_u32.to_be_bytes());
    empty_database[92..96].copy_from_slice(&1_u32.to_be_bytes());
    empty_database[100..108].copy_from_slice(&[0x0d, 0, 0, 0, 0, 0x02, 0x00, 0]);

    let (path, refused) = refusal_of(&empty_database);

    assert_damaged(&path, refused);
}

#[test]
fn a_dropped_store_has_closed_every_file_it_opened() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let folder = directory.path().canonicalize().expect("the folder's path");
    let store = Store::open(folder.join("workspace.db")).expect("the store opens");

    drop(store);

    let open: Vec<std::path::PathBuf> = std::fs::read_dir("/proc/self/fd")
        .expect("this process's descriptors")
        .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
        .filter(|target| target.starts_with(&folder))
        .collect();
    assert_eq!(open, Vec::<std::path::PathBuf>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_store_holds_stays_held_until_its_files_are_closed_with_a_read_in_flight() {
    use quadratique_server::store::AsOf;
    use std::fs::OpenOptions;

    let directory = tempfile::tempdir().expect("a temporary directory");
    let folder = directory.path().canonicalize().expect("the folder's path");
    let store = Store::open(folder.join("workspace.db")).expect("the store opens");
    let lock_path = folder.join("server.lock");
    let held = std::fs::File::create(&lock_path).expect("a lock file");
    held.try_lock().expect("the lock");
    store.hold_until_closed(held);
    let (entered, in_flight) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let reading = tokio::spawn(async move {
        store
            .read(AsOf::Now, move |_| {
                let _ = entered.send(());
                let _ = gate.recv();
                Ok::<_, StoreError>(())
            })
            .await
    });
    tokio::task::spawn_blocking(move || in_flight.recv_timeout(crate::support::HANG_GUARD))
        .await
        .expect("the waiting thread")
        .expect("the read starts");
    let contender = OpenOptions::new()
        .write(true)
        .open(&lock_path)
        .expect("the lock file");

    assert!(
        contender.try_lock().is_err(),
        "the lock is free while a read is in flight"
    );
    release.send(()).expect("the read waits");
    reading
        .await
        .expect("the read's task")
        .expect("the read succeeds");
    let open = tokio::time::timeout(
        crate::support::HANG_GUARD,
        tokio::task::spawn_blocking(move || {
            contender.lock().expect("the lock, once free");
            std::fs::read_dir("/proc/self/fd")
                .expect("this process's descriptors")
                .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
                .filter(|target| target.starts_with(&folder) && !target.ends_with("server.lock"))
                .collect::<Vec<_>>()
        }),
    )
    .await
    .expect("the lock is freed within the hang guard")
    .expect("the waiting thread");
    assert_eq!(open, Vec::<std::path::PathBuf>::new());
}
