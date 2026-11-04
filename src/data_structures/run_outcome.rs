use std::path::PathBuf;

use super::{ErrorReport, Plan};

/// What happened when the worker ran a plan.
#[derive(Clone, Debug)]
pub struct RunOutcome {
    pub plan: Plan,
    pub cwd: PathBuf,
    pub entry_ids: Vec<i64>,
    pub failure: Option<ErrorReport>,
    /// Concatenated stdout of the git steps.
    pub stdout: String,
    /// Problems in non-git steps (e.g. a file that couldn't be copied).
    pub warnings: Vec<String>,
}
