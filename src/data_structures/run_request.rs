use std::path::PathBuf;

use super::{Plan, Snapshot};

/// A plan handed to the mutation worker.
#[derive(Clone, Debug)]
pub struct RunRequest {
    pub plan: Plan,
    /// Journal `via`: the key label, `:` for raw commands, `remedy` for remedies.
    pub via: String,
    /// Human label of the action ("stash · switch · re-apply"), for follow-up notes.
    pub label: String,
    /// The failed journal entry this run remedies.
    pub parent: Option<i64>,
    pub cwd: PathBuf,
    /// Repository context at the time of the request, for classification and journaling.
    pub snapshot: Snapshot,
}
