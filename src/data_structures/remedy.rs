use super::{KeyBinding, Plan};

/// One way out of a failed command, ranked safest first in the error dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remedy {
    pub key: KeyBinding,
    pub label: String,
    pub plan: Plan,
    /// One line: what happens to your work if you pick this.
    pub consequence: String,
    /// A specific risk with this remedy right now (e.g. "pop will likely conflict").
    pub warning: Option<String>,
    pub destructive: bool,
    pub recoverable: bool,
}

impl Remedy {
    pub fn new(key: char, label: impl Into<String>, plan: Plan, consequence: impl Into<String>) -> Self {
        Remedy {
            key: KeyBinding::ch(key),
            label: label.into(),
            plan,
            consequence: consequence.into(),
            warning: None,
            destructive: false,
            recoverable: true,
        }
    }

    pub fn enter(label: impl Into<String>, plan: Plan, consequence: impl Into<String>) -> Self {
        Remedy { key: KeyBinding::enter(), ..Remedy::new(' ', label, plan, consequence) }
    }

    pub fn warning(mut self, text: Option<String>) -> Self {
        self.warning = text;
        self
    }

    /// Marks it red. `recoverable: false` adds "not recoverable" to the dialog.
    pub fn destructive(mut self, recoverable: bool) -> Self {
        self.destructive = true;
        self.recoverable = recoverable;
        self
    }
}
