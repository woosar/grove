use std::collections::HashMap;
use std::path::PathBuf;

use super::{ActionFactory, JournalEntry, Pipeline, PipelineRun, PreflightInfo, Snapshot, Theme};

/// Read-only view of app state handed to screens and modals while drawing and
/// handling keys.
pub struct Ctx<'a> {
    pub snap: &'a Snapshot,
    pub theme: &'a Theme,
    pub preflight: &'a HashMap<String, PreflightInfo>,
    /// Newest journal entries of this repo (dashboard tail).
    pub tail: &'a [JournalEntry],
    /// Last journal activity per normalized worktree path, unix ms.
    pub last_used: &'a HashMap<String, i64>,
    /// Untracked config files offered for copying into new worktrees.
    pub copy_files: &'a [PathBuf],
    /// This repository's saved pipelines.
    pub pipelines: &'a [Pipeline],
    /// The running or most recently finished pipeline.
    pub pipeline_run: Option<&'a PipelineRun>,
}

impl<'a> Ctx<'a> {
    pub fn actions(&self) -> ActionFactory<'a> {
        ActionFactory::new(self.snap, self.preflight)
    }
}
