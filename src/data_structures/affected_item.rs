/// One row of the error dialog's Affected box: a file or ref and what differs on each side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffectedItem {
    pub marker: char,
    pub name: String,
    pub detail: String,
    /// Involved but not a problem ("would carry over fine"); drawn dim.
    pub fine: bool,
}
