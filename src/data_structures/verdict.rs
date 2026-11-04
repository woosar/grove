use super::KeyBinding;

/// Outcome of an action's preflight checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Warn(String),
    Blocked { reason: String, alternative: Option<KeyBinding> },
}

impl Verdict {
    pub fn blocked(reason: impl Into<String>) -> Self {
        Verdict::Blocked { reason: reason.into(), alternative: None }
    }

    pub fn is_blocked(&self) -> bool {
        matches!(self, Verdict::Blocked { .. })
    }
}
