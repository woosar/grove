/// The five top-level screens, in tab-strip order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenId {
    Dashboard,
    Log,
    Branches,
    Worktrees,
    Journal,
}

impl ScreenId {
    pub const ALL: [ScreenId; 5] =
        [ScreenId::Dashboard, ScreenId::Log, ScreenId::Branches, ScreenId::Worktrees, ScreenId::Journal];

    pub fn title(&self) -> &'static str {
        match self {
            ScreenId::Dashboard => "Dashboard",
            ScreenId::Log => "Log",
            ScreenId::Branches => "Branches",
            ScreenId::Worktrees => "Worktrees",
            ScreenId::Journal => "Journal",
        }
    }

    pub fn number(&self) -> usize {
        ScreenId::ALL.iter().position(|s| s == self).unwrap_or(0) + 1
    }
}
