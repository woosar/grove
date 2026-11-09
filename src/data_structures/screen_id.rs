/// The top-level screens, in tab-strip order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenId {
    Dashboard,
    Log,
    Branches,
    Worktrees,
    Journal,
    Pipelines,
}

impl ScreenId {
    pub const ALL: [ScreenId; 6] =
        [ScreenId::Dashboard, ScreenId::Log, ScreenId::Branches, ScreenId::Worktrees, ScreenId::Journal, ScreenId::Pipelines];

    pub fn title(&self) -> &'static str {
        match self {
            ScreenId::Dashboard => "Dashboard",
            ScreenId::Log => "Log",
            ScreenId::Branches => "Branches",
            ScreenId::Worktrees => "Worktrees",
            ScreenId::Journal => "Journal",
            ScreenId::Pipelines => "Pipelines",
        }
    }

    pub fn number(&self) -> usize {
        ScreenId::ALL.iter().position(|s| s == self).unwrap_or(0) + 1
    }
}
