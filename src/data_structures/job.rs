use std::path::PathBuf;

use super::RunRequest;

/// Work for the background threads. The UI thread never runs git itself.
#[derive(Clone, Debug)]
pub enum Job {
    /// Full snapshot reload; `seq` lets the app drop results that arrive out of order.
    Refresh { cwd: PathBuf, seq: u64 },
    Probe { cwd: PathBuf },
    Preflight { cwd: PathBuf, target: String, dirty: Vec<PathBuf> },
    LogPage { cwd: PathBuf, rev_args: Vec<String>, skip: usize, generation: u64, remotes: Vec<String> },
    Detail { cwd: PathBuf, oid: String, remotes: Vec<String> },
    Run(Box<RunRequest>),
}
