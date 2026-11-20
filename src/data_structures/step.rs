use std::ffi::OsString;
use std::path::PathBuf;

use crate::helpers::{command_line, relative_to};

/// One thing a plan runs, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// `git <args>`; the argv excludes the leading `git`.
    Git(Vec<OsString>),
    /// Copy untracked files from one checkout into another (new-worktree form).
    Copy { from: PathBuf, to: PathBuf, files: Vec<PathBuf> },
    /// Delete a removed worktree's directory if git left it behind, then the parent
    /// directories that are empty now, stopping below `stop_at`.
    RemoveDir { path: PathBuf, stop_at: PathBuf },
}

impl Step {
    pub fn display(&self, cwd: &std::path::Path) -> String {
        match self {
            Step::Git(args) => command_line(args),
            Step::Copy { to, files, .. } => {
                let names: Vec<String> = files.iter().map(|f| f.to_string_lossy().replace('\\', "/")).collect();
                format!("cp {} → {}/", names.join(" "), relative_to(to, cwd))
            }
            Step::RemoveDir { path, .. } => format!("clean up {}", relative_to(path, cwd)),
        }
    }
}
