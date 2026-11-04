use std::path::PathBuf;

use super::{Commit, CommitDetail, PreflightInfo, RunOutcome, Snapshot};

/// A finished background job, delivered over the result channel.
#[derive(Debug)]
pub enum JobResult {
    Snapshot { cwd: PathBuf, seq: u64, result: Result<Box<Snapshot>, String> },
    Probe { cwd: PathBuf, bytes: Vec<u8> },
    Preflight { cwd: PathBuf, target: String, info: PreflightInfo },
    LogPage { generation: u64, skip: usize, commits: Vec<Commit>, total: Option<usize> },
    Detail(Box<CommitDetail>),
    Ran(Box<RunOutcome>),
}
