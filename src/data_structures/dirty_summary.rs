/// Change counts of one worktree, from `git -C <path> status --porcelain=v2`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirtySummary {
    pub modified: usize,
    pub staged: usize,
    pub untracked: usize,
    pub conflicted: usize,
}

impl DirtySummary {
    pub fn from_porcelain_v2(bytes: &[u8]) -> Self {
        let mut s = DirtySummary::default();
        for line in bytes.split(|b| *b == b'\n' || *b == 0) {
            match line {
                [b'1' | b'2', b' ', x, y, ..] => {
                    if *x != b'.' {
                        s.staged += 1;
                    }
                    if *y != b'.' {
                        s.modified += 1;
                    }
                }
                [b'u', b' ', ..] => s.conflicted += 1,
                [b'?', b' ', ..] => s.untracked += 1,
                _ => {}
            }
        }
        s
    }

    pub fn is_clean(&self) -> bool {
        self.modified + self.staged + self.untracked + self.conflicted == 0
    }

    /// `2 modified · 1 staged · 1 untracked`, or `—` when clean.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.modified > 0 {
            parts.push(format!("{} modified", self.modified));
        }
        if self.staged > 0 {
            parts.push(format!("{} staged", self.staged));
        }
        if self.untracked > 0 {
            parts.push(format!("{} untracked", self.untracked));
        }
        if self.conflicted > 0 {
            parts.push(format!("{} conflicted", self.conflicted));
        }
        if parts.is_empty() { "—".to_string() } else { parts.join(" · ") }
    }
}
