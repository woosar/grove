use std::path::PathBuf;

use crate::helpers::{bytes_to_path, lossy};

/// A local or remote-tracking branch, from `git for-each-ref`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// Short name: `feat/x` or `origin/feat/x`.
    pub name: String,
    pub remote: bool,
    pub oid: String,
    pub upstream: Option<String>,
    pub upstream_gone: bool,
    /// Ahead/behind relative to the upstream.
    pub ahead: usize,
    pub behind: usize,
    /// Ahead/behind relative to the main branch, when git can tell.
    pub vs_main: Option<(usize, usize)>,
    pub time: i64,
    pub is_head: bool,
    pub worktree: Option<PathBuf>,
    pub subject: String,
}

impl Branch {
    const FIELDS: &'static str = "%(refname)%00%(refname:short)%00%(objectname)%00%(upstream:short)%00%(upstream:track)%00%(committerdate:unix)%00%(HEAD)%00%(worktreepath)%00%(ahead-behind:MAIN)%00%(subject)";

    /// The `--format` argument. `ahead-behind` needs git ≥ 2.41 and is left out when
    /// `main` is `None`.
    pub fn format(main: Option<&str>) -> String {
        match main {
            Some(m) => format!("--format={}", Self::FIELDS.replace("MAIN", m)),
            None => format!("--format={}", Self::FIELDS.replace("%(ahead-behind:MAIN)", "")),
        }
    }

    pub fn parse_for_each_ref(bytes: &[u8]) -> Vec<Branch> {
        let mut out = Vec::new();
        for line in bytes.split(|b| *b == b'\n') {
            let f: Vec<&[u8]> = line.split(|b| *b == 0).collect();
            if f.len() < 10 {
                continue;
            }
            let full = lossy(f[0]);
            if full.ends_with("/HEAD") {
                continue;
            }
            let track = lossy(f[4]);
            let mut ahead = 0;
            let mut behind = 0;
            for part in track.trim_matches(|c| c == '[' || c == ']').split(", ") {
                if let Some(n) = part.strip_prefix("ahead ") {
                    ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("behind ") {
                    behind = n.parse().unwrap_or(0);
                }
            }
            let vs_main = lossy(f[8])
                .split_once(' ')
                .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)));
            let upstream = lossy(f[3]);
            out.push(Branch {
                name: lossy(f[1]),
                remote: full.starts_with("refs/remotes/"),
                oid: lossy(f[2]),
                upstream: (!upstream.is_empty()).then_some(upstream),
                upstream_gone: track == "[gone]",
                ahead,
                behind,
                vs_main,
                time: lossy(f[5]).parse().unwrap_or(0),
                is_head: f[6] == b"*",
                worktree: (!f[7].is_empty()).then(|| bytes_to_path(f[7])),
                subject: lossy(f[9]),
            });
        }
        out
    }

    /// `origin/feat/x` → `feat/x`, given the remotes that exist.
    pub fn local_name<'a>(&'a self, remotes: &[String]) -> &'a str {
        if !self.remote {
            return &self.name;
        }
        remotes
            .iter()
            .find_map(|r| self.name.strip_prefix(&format!("{r}/")))
            .unwrap_or(&self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_for_each_ref() {
        let branches = Branch::parse_for_each_ref(include_bytes!("../../tests/fixtures/for_each_ref.txt"));
        assert_eq!(branches.len(), 7);
        let head = &branches[0];
        assert_eq!(head.name, "feat/journal");
        assert!(head.is_head && !head.remote);
        assert_eq!(head.vs_main, Some((1, 0)));
        assert!(head.worktree.is_some());
        let main = branches.iter().find(|b| b.name == "main").unwrap();
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
        assert_eq!((main.ahead, main.behind), (3, 0));
        let remote = branches.iter().find(|b| b.remote).unwrap();
        assert_eq!(remote.name, "origin/main");
        assert_eq!(remote.local_name(&["origin".into()]), "main");
        let lg = branches.iter().find(|b| b.name == "feat/log-graph").unwrap();
        assert_eq!(lg.subject, "graph: render edges");
        assert_eq!(lg.vs_main, Some((2, 3)));
    }

    #[test]
    fn parses_gone_upstream() {
        let raw = b"refs/heads/x\x00x\x00abc\x00origin/x\x00[gone]\x00100\x00 \x00\x00\x00subj\n";
        let b = &Branch::parse_for_each_ref(raw)[0];
        assert!(b.upstream_gone);
        assert_eq!(b.vs_main, None);
    }
}
