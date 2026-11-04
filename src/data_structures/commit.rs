use super::RefLabel;
use crate::helpers::{lossy, split_nul};

/// One row of the log, from `git log -z --format=%H%x00%P%x00%D%x00%an%x00%at%x00%s`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub refs: Vec<RefLabel>,
    pub author: String,
    pub time: i64,
    pub subject: String,
}

impl Commit {
    pub const FORMAT: &'static str = "--format=%H%x00%P%x00%D%x00%an%x00%at%x00%s";

    pub fn parse_log(bytes: &[u8], remotes: &[String]) -> Vec<Commit> {
        split_nul(bytes)
            .as_chunks::<6>()
            .0
            .iter()
            .map(|f| Commit {
                oid: lossy(f[0]).trim().to_string(),
                parents: lossy(f[1]).split_whitespace().map(str::to_string).collect(),
                refs: RefLabel::parse_decorations(&lossy(f[2]), remotes),
                author: lossy(f[3]),
                time: lossy(f[4]).trim().parse().unwrap_or(0),
                subject: lossy(f[5]),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_structures::RefKind;

    #[test]
    fn parses_real_log() {
        let commits = Commit::parse_log(include_bytes!("../../tests/fixtures/log.bin"), &["origin".into()]);
        assert_eq!(commits.len(), 7);
        assert_eq!(commits[0].refs[0], RefLabel { name: "feat/journal".into(), kind: RefKind::Head });
        assert_eq!(commits[1].parents.len(), 2);
        assert_eq!(commits[1].subject, "Merge branch 'fix/utf8'");
        assert_eq!(commits[3].refs[0].kind, RefKind::Tag);
        assert_eq!(commits[6].refs[0].kind, RefKind::Remote);
        assert!(commits[6].parents.is_empty());
        assert_eq!(commits[5].refs, vec![]);
    }
}
