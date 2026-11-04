use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{DirtySummary, GitOutput};
use crate::helpers::{bytes_to_path, lossy};

/// One entry of `git worktree list --porcelain -z`, plus its dirty state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    /// Short branch name; `None` when detached or bare.
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
    pub is_main: bool,
    /// Filled in separately; `None` when it could not be read (e.g. missing dir).
    pub dirty: Option<DirtySummary>,
}

impl Worktree {
    /// Lists all worktrees of the repo at `cwd`. Git before 2.36 has no `-z` for
    /// `worktree list`; there the newline-separated form is read and converted, which
    /// only misparses paths containing newlines.
    pub fn list(cwd: &Path) -> Result<Vec<Worktree>> {
        let bytes = match GitOutput::read(cwd, &["worktree", "list", "--porcelain", "-z"]) {
            Ok(b) => b,
            Err(_) => {
                let mut b = GitOutput::read(cwd, &["worktree", "list", "--porcelain"])?;
                b.iter_mut().filter(|c| **c == b'\n').for_each(|c| *c = 0);
                b
            }
        };
        Ok(Self::parse_porcelain(&bytes))
    }

    pub fn parse_porcelain(bytes: &[u8]) -> Vec<Worktree> {
        let mut out: Vec<Worktree> = Vec::new();
        let mut cur: Option<Worktree> = None;
        for field in bytes.split(|b| *b == 0) {
            if field.is_empty() {
                out.extend(cur.take());
                continue;
            }
            let (key, value) = match field.iter().position(|b| *b == b' ') {
                Some(i) => (&field[..i], &field[i + 1..]),
                None => (field, &b""[..]),
            };
            if key == b"worktree" {
                out.extend(cur.take());
                cur = Some(Worktree { path: bytes_to_path(value), ..Default::default() });
                continue;
            }
            let Some(wt) = cur.as_mut() else { continue };
            match key {
                b"HEAD" => wt.head = lossy(value),
                b"branch" => {
                    let full = lossy(value);
                    wt.branch = Some(full.strip_prefix("refs/heads/").unwrap_or(&full).to_string());
                }
                b"bare" => wt.bare = true,
                b"detached" => wt.detached = true,
                b"locked" => wt.locked = Some(lossy(value)),
                b"prunable" => wt.prunable = Some(lossy(value)),
                _ => {}
            }
        }
        out.extend(cur);
        if let Some(first) = out.first_mut() {
            first.is_main = true;
        }
        out
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty.is_some_and(|d| !d.is_clean())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_worktree_list() {
        let wts = Worktree::parse_porcelain(include_bytes!("../../tests/fixtures/worktree_list.bin"));
        assert_eq!(wts.len(), 4);
        assert!(wts[0].is_main);
        assert_eq!(wts[0].branch.as_deref(), Some("feat/journal"));
        let gone = wts.iter().find(|w| w.branch.as_deref() == Some("gone-br")).unwrap();
        assert_eq!(gone.prunable.as_deref(), Some("gitdir file points to non-existent location"));
        let locked = wts.iter().find(|w| w.branch.as_deref() == Some("locked-br")).unwrap();
        assert_eq!(locked.locked.as_deref(), Some("waiting on CI"));
        assert!(wts[3].path.ends_with("repo.wt/log-graph"));
    }

    #[test]
    fn parses_newline_form_converted_to_nul() {
        let mut raw = b"worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /r.wt/x\nHEAD def\ndetached\n\n".to_vec();
        raw.iter_mut().filter(|c| **c == b'\n').for_each(|c| *c = 0);
        let wts = Worktree::parse_porcelain(&raw);
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert!(wts[1].detached);
    }
}
