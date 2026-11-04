/// What the Journal screen's table shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalFilter {
    /// `None` = all repos.
    pub repo: Option<String>,
    /// Local day `YYYY-MM-DD`; `None` = any day.
    pub day: Option<String>,
    /// Only entries before this local day (the "older…" bucket).
    pub before_day: Option<String>,
    pub show_grove: bool,
    pub show_raw: bool,
    pub show_readonly: bool,
    pub failed_only: bool,
    pub search: String,
    pub limit: usize,
}

impl Default for JournalFilter {
    fn default() -> Self {
        JournalFilter {
            repo: None,
            day: None,
            before_day: None,
            show_grove: true,
            show_raw: true,
            show_readonly: false,
            failed_only: false,
            search: String::new(),
            limit: 2000,
        }
    }
}
