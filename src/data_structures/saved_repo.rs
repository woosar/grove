use std::path::{Path, PathBuf};

use super::{GroveError, Snapshot, Worktree};

/// A base repository remembered in the journal database, offered by the repo picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedRepo {
    pub id: i64,
    /// The main worktree's top level.
    pub path: PathBuf,
    /// Unix ms of the last time grove opened it; `None` when only added.
    pub last_opened: Option<i64>,
}

impl SavedRepo {
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }

    /// The main worktree of the repository containing `path`; any of its worktrees or
    /// subdirectories leads to the same base. `Err` explains in plain words.
    pub fn locate(path: &Path) -> Result<PathBuf, String> {
        if !path.is_dir() {
            return Err(format!("{} isn't a directory", path.display()));
        }
        let (top, _) = Snapshot::locate(path).map_err(|out| {
            let err = GroveError::classify(&["rev-parse".to_string()], out.status, &out.stderr_text());
            let (pre, name, post) = err.headline();
            format!("{pre}{name}{post}")
        })?;
        let main = Worktree::list(&top).ok().and_then(|w| w.into_iter().next()).map(|w| w.path);
        Ok(main.unwrap_or(top))
    }
}
