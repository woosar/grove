use std::collections::HashMap;
use std::path::PathBuf;

use super::{ChangeArea, FileChange};
use crate::helpers::{bytes_to_path, lossy, split_nul};

type NumStat = HashMap<PathBuf, (Option<u32>, Option<u32>)>;

/// Parsed `git status --porcelain=v2 --branch -z`.
#[derive(Clone, Debug, Default)]
pub struct RepoStatus {
    pub oid: Option<String>,
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub changes: Vec<FileChange>,
}

impl RepoStatus {
    pub fn parse(bytes: &[u8], staged_stat: &NumStat, unstaged_stat: &NumStat) -> Self {
        let mut st = RepoStatus::default();
        let fields = split_nul(bytes);
        let mut i = 0;
        while i < fields.len() {
            let field = fields[i];
            i += 1;
            if let Some(header) = field.strip_prefix(b"# ") {
                let header = lossy(header);
                let (key, value) = header.split_once(' ').unwrap_or((&header, ""));
                match key {
                    "branch.oid" if value != "(initial)" => st.oid = Some(value.to_string()),
                    "branch.head" if value != "(detached)" => st.branch = Some(value.to_string()),
                    "branch.upstream" => st.upstream = Some(value.to_string()),
                    "branch.ab" => {
                        for part in value.split(' ') {
                            if let Some(n) = part.strip_prefix('+') {
                                st.ahead = n.parse().unwrap_or(0);
                            } else if let Some(n) = part.strip_prefix('-') {
                                st.behind = n.parse().unwrap_or(0);
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }
            match field.first() {
                Some(b'1') | Some(b'2') => {
                    let renamed = field[0] == b'2';
                    // "1 XY sub mH mI mW hH hI path"; type 2 adds a score before the path
                    let path_at = if renamed { 9 } else { 8 };
                    let parts: Vec<&[u8]> = field.splitn(path_at + 1, |b| *b == b' ').collect();
                    let xy = parts.get(1).copied().unwrap_or(b"..");
                    let path = bytes_to_path(parts.get(path_at).copied().unwrap_or_default());
                    let orig = if renamed {
                        i += 1;
                        fields.get(i - 1).map(|f| bytes_to_path(f))
                    } else {
                        None
                    };
                    let (x, y) = (xy[0] as char, *xy.get(1).unwrap_or(&b'.') as char);
                    if x != '.' {
                        let (added, removed) = staged_stat.get(&path).copied().unwrap_or((None, None));
                        st.changes.push(FileChange {
                            area: ChangeArea::Staged,
                            code: x,
                            path: path.clone(),
                            orig_path: orig,
                            added,
                            removed,
                        });
                    }
                    if y != '.' {
                        let (added, removed) = unstaged_stat.get(&path).copied().unwrap_or((None, None));
                        st.changes.push(FileChange {
                            area: ChangeArea::Unstaged,
                            code: y,
                            path,
                            orig_path: None,
                            added,
                            removed,
                        });
                    }
                }
                Some(b'u') => {
                    let path = field.splitn(11, |b| *b == b' ').nth(10).unwrap_or_default();
                    st.changes.push(FileChange {
                        area: ChangeArea::Conflicted,
                        code: 'U',
                        path: bytes_to_path(path),
                        orig_path: None,
                        added: None,
                        removed: None,
                    });
                }
                Some(b'?') if field.len() > 2 => st.changes.push(FileChange {
                    area: ChangeArea::Untracked,
                    code: '?',
                    path: bytes_to_path(&field[2..]),
                    orig_path: None,
                    added: None,
                    removed: None,
                }),
                _ => {}
            }
        }
        st
    }

    pub fn in_area(&self, area: ChangeArea) -> impl Iterator<Item = &FileChange> {
        self.changes.iter().filter(move |c| c.area == area)
    }

    pub fn count(&self, area: ChangeArea) -> usize {
        self.in_area(area).count()
    }

    /// Distinct tracked paths with uncommitted changes (staged or not).
    pub fn modified_count(&self) -> usize {
        self.dirty_paths().len()
    }

    /// Tracked paths with uncommitted changes, sorted and deduplicated.
    pub fn dirty_paths(&self) -> Vec<&PathBuf> {
        let mut paths: Vec<&PathBuf> = self
            .changes
            .iter()
            .filter(|c| c.area != ChangeArea::Untracked)
            .map(|c| &c.path)
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::parse_numstat_z;

    #[test]
    fn parses_real_status() {
        let staged = parse_numstat_z(include_bytes!("../../tests/fixtures/numstat_staged.bin"));
        let unstaged = parse_numstat_z(include_bytes!("../../tests/fixtures/numstat_unstaged.bin"));
        let st = RepoStatus::parse(include_bytes!("../../tests/fixtures/status_v2.bin"), &staged, &unstaged);
        assert_eq!(st.branch.as_deref(), Some("feat/journal"));
        assert_eq!(st.oid.as_deref(), Some("ea5cb198de12c20474f79d02c3dc48c7489a794b"));
        assert_eq!(st.count(ChangeArea::Staged), 1);
        assert_eq!(st.count(ChangeArea::Unstaged), 2);
        let untracked: Vec<_> = st.in_area(ChangeArea::Untracked).collect();
        assert_eq!(untracked[0].path, PathBuf::from("new file.sql"));
        let store = st.in_area(ChangeArea::Staged).next().unwrap();
        assert_eq!((store.code, store.added, store.removed), ('M', Some(1), Some(0)));
        assert_eq!(st.modified_count(), 3);
    }

    #[test]
    fn parses_branch_ab_and_rename() {
        let raw = b"# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\x002 R. N... 100644 100644 100644 aaa bbb R100 new.rs\0old.rs\0";
        let st = RepoStatus::parse(raw, &HashMap::new(), &HashMap::new());
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!(st.changes[0].path, PathBuf::from("new.rs"));
        assert_eq!(st.changes[0].orig_path, Some(PathBuf::from("old.rs")));
    }
}
