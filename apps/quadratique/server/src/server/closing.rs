use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::RuntimeFile;

pub(super) type HomeLock = Arc<File>;

pub(super) struct Published {
    path: PathBuf,
    pid: u32,
}

impl Published {
    pub(super) fn new(path: PathBuf, runtime: &RuntimeFile) -> Published {
        Published {
            path,
            pid: runtime.pid,
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Published {
    fn drop(&mut self) {
        let ours = std::fs::read(&self.path)
            .ok()
            .and_then(|text| serde_json::from_slice::<RuntimeFile>(&text).ok())
            .is_some_and(|written| written.pid == self.pid);
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
