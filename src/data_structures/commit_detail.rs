use std::path::Path;

use super::{FileStat, RefLabel};
use crate::helpers::{lossy, parse_numstat_z, split_nul};

/// Everything the Commit and Files panes show for one commit.
#[derive(Clone, Debug, Default)]
pub struct CommitDetail {
    pub oid: String,
    pub parents: Vec<String>,
    pub refs: Vec<RefLabel>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub subject: String,
    pub body: String,
    pub files: Vec<FileStat>,
    /// Local and remote branches containing the commit.
    pub contained_in: Vec<String>,
    /// First-parent patch, used for hunk previews.
    pub patch: String,
}

impl CommitDetail {
    pub const HEADER_FORMAT: &'static str = "--format=%H%x00%P%x00%D%x00%an%x00%ae%x00%at%x00%s%x00%b";

    pub fn parse(
        header: &[u8],
        numstat: &[u8],
        name_status: &[u8],
        contains: &[u8],
        patch: &[u8],
        remotes: &[String],
    ) -> Self {
        let f: Vec<String> = header.splitn(8, |b| *b == 0).map(lossy).collect();
        let get = |i: usize| f.get(i).cloned().unwrap_or_default();
        let stats = parse_numstat_z(numstat);
        let mut files = Vec::new();
        let fields = split_nul(name_status);
        let mut i = 0;
        while i < fields.len() {
            let status = lossy(fields[i]);
            let letter = status.trim().chars().next().unwrap_or('M');
            // renames and copies carry two paths
            let path_idx = if matches!(letter, 'R' | 'C') { i + 2 } else { i + 1 };
            let Some(path) = fields.get(path_idx) else { break };
            let path = lossy(path);
            let (added, removed) = stats.get(Path::new(&path)).copied().unwrap_or((None, None));
            files.push(FileStat { status: letter, path, added, removed });
            i = path_idx + 1;
        }
        CommitDetail {
            oid: get(0).trim().to_string(),
            parents: get(1).split_whitespace().map(str::to_string).collect(),
            refs: RefLabel::parse_decorations(&get(2), remotes),
            author: get(3),
            email: get(4),
            time: get(5).trim().parse().unwrap_or(0),
            subject: get(6),
            body: get(7).trim_end().to_string(),
            files,
            contained_in: lossy(contains)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.ends_with("/HEAD") && !l.starts_with('('))
                .collect(),
            patch: lossy(patch),
        }
    }

    /// The first hunk of `path` in the patch, header line included.
    pub fn first_hunk(&self, path: &str) -> Vec<String> {
        let marker = format!(" b/{path}");
        let mut in_file = false;
        let mut out: Vec<String> = Vec::new();
        for line in self.patch.lines() {
            if line.starts_with("diff --git ") {
                if !out.is_empty() {
                    break;
                }
                in_file = line.ends_with(&marker);
                continue;
            }
            if !in_file {
                continue;
            }
            if line.starts_with("@@") {
                if !out.is_empty() {
                    break;
                }
                out.push(line.to_string());
            } else if !out.is_empty() {
                out.push(line.to_string());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_show() {
        let numstat = include_bytes!("../../tests/fixtures/show_numstat.bin");
        // the fixture holds header + numstat from one call; grove asks for them separately
        let stats_start = numstat.iter().position(|b| *b == b'\n').unwrap() + 1;
        let header = b"f4009be8\x00428ab6b4\x00feat/log-graph\x00woosar\x00w@x\x001793100961\x00graph: render edges\x00Body line\n";
        let d = CommitDetail::parse(
            header,
            &numstat[stats_start..],
            include_bytes!("../../tests/fixtures/show_name_status.bin"),
            b"feat/log-graph\nmain\n",
            b"diff --git a/lanes.rs b/lanes.rs\nnew file mode 100644\n--- /dev/null\n+++ b/lanes.rs\n@@ -0,0 +1 @@\n+y\n",
            &[],
        );
        assert_eq!(d.subject, "graph: render edges");
        assert_eq!(d.body, "Body line");
        assert_eq!(d.files, vec![FileStat { status: 'A', path: "lanes.rs".into(), added: Some(1), removed: Some(0) }]);
        assert_eq!(d.contained_in, vec!["feat/log-graph", "main"]);
        assert_eq!(d.first_hunk("lanes.rs"), vec!["@@ -0,0 +1 @@", "+y"]);
    }
}
