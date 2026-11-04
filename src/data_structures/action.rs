use super::{KeyBinding, Plan, Verdict};

/// Anything a key does: a label, what will run, why, and whether it should.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub key: KeyBinding,
    pub label: &'static str,
    /// One line, shown in tooltips and the preview line.
    pub explain: String,
    /// Shown on `?`.
    pub long_help: &'static str,
    /// Short dim annotation after the command in popups (`→ into current`).
    pub note: String,
    pub plan: Plan,
    pub verdict: Verdict,
    /// Ask before running, with this question.
    pub confirm: Option<String>,
    /// Discards work; rendered red and confirmed in red.
    pub destructive: bool,
}

impl Action {
    pub fn new(key: KeyBinding, label: &'static str, plan: Plan) -> Self {
        Action {
            key,
            label,
            explain: String::new(),
            long_help: "",
            note: String::new(),
            plan,
            verdict: Verdict::Ok,
            confirm: None,
            destructive: false,
        }
    }

    pub fn explain(mut self, text: impl Into<String>) -> Self {
        self.explain = text.into();
        self
    }

    pub fn help(mut self, text: &'static str) -> Self {
        self.long_help = text;
        self
    }

    pub fn note(mut self, text: impl Into<String>) -> Self {
        self.note = text.into();
        self
    }

    pub fn verdict(mut self, verdict: Verdict) -> Self {
        self.verdict = verdict;
        self
    }

    pub fn confirm(mut self, question: impl Into<String>) -> Self {
        self.confirm = Some(question.into());
        self
    }

    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }
}
