use std::path::{Path, PathBuf};

use super::GitOutput;
use crate::helpers::{bytes_to_path, lossy, split_nul};

/// Switch preflight data that costs a git call, computed off the UI thread per target.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreflightInfo {
    /// Dirty paths that also differ between HEAD and the target; switching would
    /// have to overwrite them.
    pub overlap: Vec<PathBuf>,
    /// Commits on HEAD that the target lacks (what a rebase onto it would replay).
    pub head_only: usize,
}

impl PreflightInfo {
    pub fn load(cwd: &Path, target: &str, dirty: &[PathBuf]) -> Self {
        let differing: Vec<PathBuf> = GitOutput::read(cwd, &["diff", "--name-only", "-z", "HEAD", target, "--"])
            .map(|out| split_nul(&out).into_iter().map(bytes_to_path).collect())
            .unwrap_or_default();
        let overlap = dirty.iter().filter(|d| differing.contains(d)).cloned().collect();
        let head_only = GitOutput::read(cwd, &["rev-list", "--count", &format!("{target}..HEAD")])
            .ok()
            .and_then(|o| lossy(&o).trim().parse().ok())
            .unwrap_or(0);
        PreflightInfo { overlap, head_only }
    }
}
