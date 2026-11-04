use std::path::{Path, PathBuf};

use super::{PromptKind, ScreenId};
use crate::helpers::relative_to;

/// Something grove does itself, without running git.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InternalOp {
    /// Make a worktree the active context, opening a tab for it if needed.
    OpenWorktree(PathBuf),
    GoTo(ScreenId),
    /// Ask for a commit message; optionally switch branches afterwards.
    CommitDialog { then_switch: Option<String> },
    Copy(String),
    Prompt(PromptKind),
    /// Open the new-worktree form, prefilled with a branch.
    NewWorktreeForm(Option<String>),
    /// The soft / mixed / keep / hard submenu for resetting to a commit.
    ResetMenu(String),
    /// Open the `:` prompt prefilled with a command line (without `git `).
    EditRaw(String),
}

impl InternalOp {
    /// Paths are shown relative to `cwd`, like the git commands next to them.
    pub fn describe(&self, cwd: &Path) -> String {
        match self {
            InternalOp::OpenWorktree(p) => format!("open worktree {}", relative_to(p, cwd)),
            InternalOp::GoTo(s) => format!("go to {}", s.title()),
            InternalOp::CommitDialog { then_switch: Some(b) } => format!("open the commit dialog, then switch to {b}"),
            InternalOp::CommitDialog { then_switch: None } => "open the commit dialog".to_string(),
            InternalOp::Copy(_) => "copy to clipboard".to_string(),
            InternalOp::Prompt(kind) => format!("ask for {}", kind.title().to_lowercase()),
            InternalOp::NewWorktreeForm(_) => "open the new-worktree form".to_string(),
            InternalOp::ResetMenu(oid) => format!("choose how to reset to {}", &oid[..oid.len().min(7)]),
            InternalOp::EditRaw(cmd) => format!("open the : prompt with `git {cmd}`"),
        }
    }
}
