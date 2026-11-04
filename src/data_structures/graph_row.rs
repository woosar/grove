/// The graph column of one log row: two cells per lane (glyph, connector), each tagged
/// with the lane whose color it takes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphRow {
    pub cells: Vec<(char, usize)>,
    pub commit_lane: usize,
}

impl GraphRow {
    #[cfg(test)]
    pub fn text(&self) -> String {
        self.cells.iter().map(|(c, _)| *c).collect()
    }

    pub fn width(&self) -> usize {
        self.cells.len()
    }
}
