use super::GraphRow;

/// Lane allocation over topo-ordered commits. Each lane holds the oid it expects next;
/// freed lanes are reused lowest-first so wide histories stay narrow.
#[derive(Clone, Debug, Default)]
pub struct LaneAllocator {
    lanes: Vec<Option<String>>,
}

impl LaneAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    fn alloc(&mut self) -> usize {
        match self.lanes.iter().position(Option::is_none) {
            Some(i) => i,
            None => {
                self.lanes.push(None);
                self.lanes.len() - 1
            }
        }
    }

    pub fn push(&mut self, oid: &str, parents: &[String]) -> GraphRow {
        let active_before: Vec<bool> = self.lanes.iter().map(Option::is_some).collect();
        let was_active = |i: usize| active_before.get(i).copied().unwrap_or(false);

        let col = match self.lanes.iter().position(|l| l.as_deref() == Some(oid)) {
            Some(i) => i,
            None => self.alloc(),
        };
        // other lanes that were waiting for this commit end here (merge-in edges)
        let merging: Vec<usize> = (0..self.lanes.len())
            .filter(|&i| i != col && self.lanes[i].as_deref() == Some(oid))
            .collect();
        for &i in &merging {
            self.lanes[i] = None;
        }
        self.lanes[col] = parents.first().cloned();
        // further parents either join a lane already waiting for them, or open one
        let mut forks: Vec<(usize, bool)> = Vec::new();
        for p in parents.iter().skip(1) {
            match self.lanes.iter().position(|l| l.as_deref() == Some(p.as_str())) {
                Some(i) => forks.push((i, false)),
                None => {
                    let i = self.alloc();
                    self.lanes[i] = Some(p.clone());
                    forks.push((i, true));
                }
            }
        }

        let width = [
            self.lanes.len(),
            col + 1,
            merging.iter().max().map_or(0, |m| m + 1),
            forks.iter().map(|f| f.0 + 1).max().unwrap_or(0),
        ]
        .into_iter()
        .max()
        .unwrap_or(1);
        let mut cells = vec![(' ', 0usize); width * 2];
        for (i, lane) in self.lanes.iter().enumerate() {
            if lane.is_some() && was_active(i) {
                cells[i * 2] = ('│', i);
            }
        }
        let mut edges: Vec<usize> = Vec::new();
        for &i in &merging {
            let reopened = forks.iter().any(|&(f, new)| f == i && new);
            cells[i * 2] = match (i > col, reopened) {
                (true, false) => ('╯', i),
                (false, false) => ('╰', i),
                (true, true) => ('┤', i),
                (false, true) => ('├', i),
            };
            edges.push(i);
        }
        for &(i, new) in &forks {
            if merging.contains(&i) {
                continue;
            }
            cells[i * 2] = match (i > col, new) {
                (true, true) => ('╮', i),
                (false, true) => ('╭', i),
                (true, false) => ('┤', i),
                (false, false) => ('├', i),
            };
            edges.push(i);
        }
        for &i in &edges {
            let (lo, hi) = (col.min(i) * 2, col.max(i) * 2);
            for cell in cells.iter_mut().take(hi).skip(lo + 1) {
                if cell.0 == ' ' {
                    *cell = ('─', i);
                }
            }
        }
        cells[col * 2] = ('●', col);

        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }
        while cells.len() > 1 && cells.last().is_some_and(|c| c.0 == ' ') {
            cells.pop();
        }
        GraphRow { cells, commit_lane: col }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_structures::Commit;

    fn render(commits: &[(&str, &[&str])]) -> Vec<String> {
        let mut lanes = LaneAllocator::new();
        commits
            .iter()
            .map(|(oid, parents)| {
                let parents: Vec<String> = parents.iter().map(|p| p.to_string()).collect();
                lanes.push(oid, &parents).text()
            })
            .collect()
    }

    #[test]
    fn linear_history_is_one_lane() {
        assert_eq!(render(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]), vec!["●", "●", "●"]);
    }

    #[test]
    fn branch_and_merge_back() {
        // the mockup shape: a side branch forks off and merges back in
        let rows = render(&[
            ("h", &["g"]),
            ("x2", &["x1"]),
            ("x1", &["g"]),
            ("g", &["m"]),
            ("m", &["e", "y"]),
            ("y", &["e"]),
            ("e", &[]),
        ]);
        assert_eq!(rows, vec!["●", "│ ●", "│ ●", "●─╯", "●─╮", "│ ●", "●─╯"]);
    }

    #[test]
    fn freed_lanes_are_reused() {
        let rows = render(&[
            ("a", &["base"]),
            ("b", &["base"]),
            ("c", &["base"]),
            ("base", &["root"]),
            ("d", &["root2"]),
            ("root", &[]),
        ]);
        assert_eq!(rows[3], "●─╯─╯");
        // after `base` the two side lanes are free again, so `d` gets lane 1, not lane 3
        assert_eq!(rows[4], "│ ●");
    }

    #[test]
    fn real_log_renders() {
        let commits = Commit::parse_log(include_bytes!("../../tests/fixtures/log.bin"), &[]);
        let mut lanes = LaneAllocator::new();
        let rows: Vec<String> = commits.iter().map(|c| lanes.push(&c.oid, &c.parents).text()).collect();
        assert_eq!(rows[0], "●");
        assert_eq!(rows[1], "●─╮");
        assert!(rows.iter().all(|r| r.contains('●')));
    }
}
