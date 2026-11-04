use super::Plan;
use crate::helpers::{command_line, short_oid, to_os_args};

/// One row of the journal: a git command grove ran, and what it did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JournalEntry {
    pub id: i64,
    pub started_at: i64,
    pub duration_ms: i64,
    pub repo_root: String,
    pub worktree: String,
    pub branch: Option<String>,
    /// Key label that triggered it, `:` for raw commands, `remedy` for error remedies.
    pub via: String,
    /// Arguments after `git`.
    pub argv: Vec<String>,
    /// `None` when git could not be spawned.
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub head_before: Option<String>,
    pub head_after: Option<String>,
    pub reflog_before: Option<String>,
    pub mutating: bool,
    pub parent_id: Option<i64>,
    pub note: Option<String>,
}

impl JournalEntry {
    pub const NOTE_DELETED: &'static str = "grove: deleted branch ";

    pub fn command(&self) -> String {
        command_line(&to_os_args(&self.argv))
    }

    pub fn ok(&self) -> bool {
        self.exit_code == Some(0)
    }

    pub fn subcommand(&self) -> &str {
        self.argv.iter().find(|a| !a.starts_with('-')).map(String::as_str).unwrap_or("")
    }

    fn args_after_sub(&self) -> Vec<&str> {
        self.argv.iter().skip_while(|a| a.starts_with('-')).skip(1).map(String::as_str).collect()
    }

    /// `(branch, oid)` recorded before a `git branch -d/-D`.
    pub fn deleted_branch(&self) -> Option<(String, String)> {
        let rest = self.note.as_deref()?.lines().find_map(|l| l.strip_prefix(Self::NOTE_DELETED))?;
        let (name, oid) = rest.split_once(" at ")?;
        Some((name.to_string(), oid.trim().to_string()))
    }

    /// How to undo this entry through the reflog, or why that's not applicable.
    /// `current_head` guards against undoing something that later commands built on.
    pub fn undo(&self, current_head: Option<&str>) -> Result<(Plan, String), String> {
        if !self.mutating {
            return Err("n/a — read-only command".into());
        }
        let moved = self.head_before.is_some() && self.head_after.is_some() && self.head_before != self.head_after;
        let sub = self.subcommand();
        let rest = self.args_after_sub();
        if !self.ok() && !moved {
            return Err("n/a — this entry changed nothing".into());
        }
        if sub == "stash" && matches!(rest.first(), None | Some(&"push") | Some(&"save")) {
            return Ok((Plan::git(&["stash", "pop"]), "re-applies the stash entry this created, then drops it".into()));
        }
        if let Some((name, oid)) = self.deleted_branch() {
            let short = short_oid(&oid).to_string();
            return Ok((Plan::git(&["branch", &name, &oid]), format!("recreates {name} at {short}")));
        }
        if matches!(sub, "switch" | "checkout") && !rest.iter().any(|a| *a == "--" || *a == "-p") {
            return match (&self.branch, &self.head_before) {
                (Some(b), _) => Ok((Plan::git(&["switch", b]), format!("switches back to {b}"))),
                (None, Some(oid)) => Ok((
                    Plan::git(&["switch", "--detach", oid]),
                    format!("goes back to the detached HEAD at {}", short_oid(oid)),
                )),
                _ => Err("n/a — nothing recorded to go back to".into()),
            };
        }
        if moved {
            let before = self.head_before.as_deref().unwrap_or_default();
            if current_head.is_some_and(|h| Some(h) != self.head_after.as_deref()) {
                return Err("n/a — HEAD has moved since; undo the later entries first".into());
            }
            let on = self.branch.as_deref().unwrap_or("HEAD");
            return Ok((
                Plan::git(&["reset", "--keep", before]),
                format!(
                    "moves {on} back to {}; undone commits stay in the reflog, uncommitted edits are kept (refuses if they'd be lost)",
                    short_oid(before)
                ),
            ));
        }
        Err("n/a — this entry changed nothing".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(argv: &str, before: &str, after: &str) -> JournalEntry {
        JournalEntry {
            argv: crate::helpers::split_shell_words(argv).unwrap(),
            exit_code: Some(0),
            mutating: true,
            branch: Some("feat/x".into()),
            head_before: Some(before.into()),
            head_after: Some(after.into()),
            ..Default::default()
        }
    }

    fn undo_cmd(e: &JournalEntry, head: Option<&str>) -> String {
        match e.undo(head) {
            Ok((plan, _)) => plan.display(std::path::Path::new(".")),
            Err(reason) => reason,
        }
    }

    #[test]
    fn undo_plans() {
        assert_eq!(undo_cmd(&entry("commit -m x", "aaa", "bbb"), Some("bbb")), "git reset --keep aaa");
        assert!(undo_cmd(&entry("commit -m x", "aaa", "bbb"), Some("ccc")).starts_with("n/a"));
        assert_eq!(undo_cmd(&entry("switch main", "aaa", "bbb"), None), "git switch feat/x");
        assert_eq!(undo_cmd(&entry("stash push -m x", "aaa", "aaa"), None), "git stash pop");
        let mut del = entry("branch -d old", "aaa", "aaa");
        del.note = Some("grove: deleted branch old at 0123456789".into());
        assert_eq!(undo_cmd(&del, None), "git branch old 0123456789");
        let mut failed = entry("rebase main", "aaa", "aaa");
        failed.exit_code = Some(1);
        assert_eq!(undo_cmd(&failed, None), "n/a — this entry changed nothing");
    }
}
