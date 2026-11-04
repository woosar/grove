use std::path::PathBuf;

use super::ChangeArea;

/// One line of the status pane: a path with its status letter in one area.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub area: ChangeArea,
    pub code: char,
    pub path: PathBuf,
    pub orig_path: Option<PathBuf>,
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

impl FileChange {
    pub fn display(&self) -> String {
        let path = self.path.to_string_lossy().replace('\\', "/");
        match &self.orig_path {
            Some(orig) => format!("{} → {path}", orig.to_string_lossy().replace('\\', "/")),
            None => path,
        }
    }

    pub fn line_counts(&self) -> String {
        match (self.added, self.removed) {
            (Some(a), Some(r)) => format!("+{a} −{r}"),
            (None, None) if matches!(self.area, ChangeArea::Staged | ChangeArea::Unstaged) => "bin".to_string(),
            _ => String::new(),
        }
    }
}
