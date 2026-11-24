use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::support::{HANG_GUARD, call, open};
use quadratic_server::store::{AsOf, StoreError};

const THE_WRITERS_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

// The unix VFS of SQLite, which doltlite keeps, takes a writer's exclusive lock as a POSIX
// write lock on the 510 bytes of the shared range, from offset 0x40000002.
const HOLD_THE_EXCLUSIVE_LOCK: &str = "\
import fcntl, os, sys
database = os.open(sys.argv[1], os.O_RDWR)
fcntl.lockf(database, fcntl.LOCK_EX | fcntl.LOCK_NB, 510, 0x40000002)
print('held', flush=True)
sys.stdin.readline()
";

struct LockHolder {
    child: Child,
    release: Option<ChildStdin>,
}

impl LockHolder {
    fn take(database: &Path) -> LockHolder {
        let mut child = Command::new("python3")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .args(["-c", HOLD_THE_EXCLUSIVE_LOCK])
            .arg(database)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("python3 runs");
        let release = child.stdin.take();
        let output = child.stdout.take().expect("the holder's output");
        let holder = LockHolder { child, release };
        let (answer, answered) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(output).read_line(&mut line);
            let _ = answer.send(line);
        });
        let line = answered
            .recv_timeout(HANG_GUARD)
            .expect("the holder answers within the hang guard");
        assert_eq!(line.trim(), "held", "the holder takes the database's lock");
        holder
    }

    fn release(&mut self) {
        let mut input = self.release.take().expect("the lock is still held");
        writeln!(input, "release").expect("the holder hears the release");
    }
}

impl Drop for LockHolder {
    fn drop(&mut self) {
        drop(self.release.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_waits_for_a_publication_that_outlasts_the_writers_busy_timeout() {
    let board = open();
    board.project("DEM").await;
    let id = board.issue("DEM", "Watched").await.value;
    assert_eq!(
        board.title(AsOf::Now, &id).await.expect("a first read"),
        Some("Watched".into()),
        "the read connection exists and its snapshot is taken"
    );
    board
        .store
        .write(call("agent-1", "issue.update"), {
            let id = id.clone();
            move |tx| tx.set_status(&id, "in-progress")
        })
        .await
        .expect("the status changes");

    let mut holder = LockHolder::take(board.store.path());
    let mut read = tokio::spawn({
        let store = board.store.clone();
        let id = id.clone();
        async move {
            store
                .read(AsOf::Now, move |view| {
                    Ok::<_, StoreError>(view.issue(&id)?.map(|issue| issue.status))
                })
                .await
        }
    });

    let past_the_old_cutoff = THE_WRITERS_BUSY_TIMEOUT * 2;
    if let Ok(ended) = tokio::time::timeout(past_the_old_cutoff, &mut read).await {
        panic!("the read ended while the lock was held, before {past_the_old_cutoff:?}: {ended:?}");
    }
    holder.release();

    let status = tokio::time::timeout(HANG_GUARD, read)
        .await
        .expect("the read ends within the hang guard once the lock is released")
        .expect("the read's task ends")
        .expect("the read succeeds");
    assert_eq!(status.as_deref(), Some("in-progress"));
}
