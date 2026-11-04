use std::path::PathBuf;

/// What a one-line text prompt is asking for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// `:` — runs `git <input>`.
    Raw,
    Commit { then_switch: Option<String> },
    NewBranch,
    RenameBranch(String),
    SetUpstream(String),
    Tag(String),
    MoveWorktree(PathBuf),
    LockWorktree(PathBuf),
    LogFilter,
    JournalSearch,
    OpenRepo,
}

impl PromptKind {
    pub fn title(&self) -> &'static str {
        match self {
            PromptKind::Raw => "Raw git",
            PromptKind::Commit { .. } => "Commit message",
            PromptKind::NewBranch => "New branch",
            PromptKind::RenameBranch(_) => "Rename branch",
            PromptKind::SetUpstream(_) => "Set upstream",
            PromptKind::Tag(_) => "Tag name",
            PromptKind::MoveWorktree(_) => "Move worktree to",
            PromptKind::LockWorktree(_) => "Lock reason",
            PromptKind::LogFilter => "Log filter",
            PromptKind::JournalSearch => "Search journal",
            PromptKind::OpenRepo => "Open repository",
        }
    }
}
