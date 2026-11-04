/// Which section of the status pane a change belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeArea {
    Staged,
    Unstaged,
    Untracked,
    Conflicted,
}
