//! Live reload by polling the file's modification time (FR-15). Polling ~1 s is
//! smaller than pulling in a file-notification crate and costs nothing
//! measurable.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct Watcher {
    path: PathBuf,
    last: Option<SystemTime>,
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Watcher {
    pub fn new(path: &Path) -> Self {
        Watcher {
            path: path.to_path_buf(),
            last: mtime(path),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True once per modification (or creation / deletion) of the file.
    pub fn changed(&mut self) -> bool {
        let now = mtime(&self.path);
        if now == self.last {
            false
        } else {
            self.last = now;
            true
        }
    }
}
