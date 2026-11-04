/// A file touched by a commit, with its status letter and line counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStat {
    pub status: char,
    pub path: String,
    pub added: Option<u32>,
    pub removed: Option<u32>,
}
