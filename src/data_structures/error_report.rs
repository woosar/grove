use std::path::PathBuf;

use super::{AffectedItem, GitOutput, GroveError, Remedy, Snapshot};
use crate::helpers::{lossy, to_os_args};

/// Everything the error dialog shows for one failed command.
#[derive(Clone, Debug)]
pub struct ErrorReport {
    pub error: GroveError,
    pub args: Vec<String>,
    pub exit: Option<i32>,
    pub duration_ms: i64,
    pub entry_id: Option<i64>,
    /// Raw git output, stderr first, verbatim.
    pub raw: String,
    pub affected: Vec<AffectedItem>,
    pub remedies: Vec<Remedy>,
    pub cwd: PathBuf,
}

impl ErrorReport {
    /// Classifies the failure and gathers the extra context the dialog needs; may run
    /// a few read-only git commands, so call it off the UI thread.
    pub fn build(args: &[String], out: &GitOutput, entry_id: Option<i64>, snap: &Snapshot) -> ErrorReport {
        let stderr = lossy(&out.stderr);
        let stdout = lossy(&out.stdout);
        let combined = format!("{stderr}\n{stdout}");
        let error = GroveError::classify(args, out.status, &combined);
        let affected = match &error {
            GroveError::WouldOverwrite { files, target, .. } => {
                let touching = |f: &str| -> usize {
                    let Some(t) = target else { return 0 };
                    let argv = to_os_args(&["rev-list", "--count", &format!("HEAD..{t}"), "--", f]);
                    let out = GitOutput::run(&argv, &snap.workdir);
                    out.stdout_text().trim().parse().unwrap_or(0)
                };
                GroveError::affected_files(files, target.as_deref(), snap, touching)
            }
            GroveError::MergeConflict { files, .. } => files
                .iter()
                .map(|f| AffectedItem { marker: 'U', name: f.clone(), detail: "changed on both sides".into(), fine: false })
                .collect(),
            _ => Vec::new(),
        };
        let remedies = error.remedies(args, snap, &affected);
        let raw = if stdout.trim().is_empty() || !combined.contains("CONFLICT") {
            stderr.trim_end().to_string()
        } else {
            format!("{}\n{}", stdout.trim_end(), stderr.trim_end()).trim().to_string()
        };
        ErrorReport {
            error,
            args: args.to_vec(),
            exit: out.status,
            duration_ms: out.duration_ms,
            entry_id,
            raw,
            affected,
            remedies,
            cwd: snap.workdir.clone(),
        }
    }
}
