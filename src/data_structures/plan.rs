use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{InternalOp, Step};
use crate::helpers::to_os_args;

/// What an action will run: git steps in order (stop on first failure), then an
/// optional internal follow-up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub then: Option<InternalOp>,
    /// Show stdout in a pager afterwards (read-only commands like `git show`).
    pub show_output: bool,
    /// Where to run; `None` means the active worktree.
    pub cwd: Option<PathBuf>,
}

impl Plan {
    pub fn git<S: AsRef<str>>(args: &[S]) -> Plan {
        Plan::git_os(to_os_args(args))
    }

    pub fn git_os(args: Vec<OsString>) -> Plan {
        Plan { steps: vec![Step::Git(args)], ..Default::default() }
    }

    pub fn internal(op: InternalOp) -> Plan {
        Plan { then: Some(op), ..Default::default() }
    }

    pub fn and<S: AsRef<str>>(self, args: &[S]) -> Plan {
        self.and_os(to_os_args(args))
    }

    pub fn and_os(mut self, args: Vec<OsString>) -> Plan {
        self.steps.push(Step::Git(args));
        self
    }

    pub fn and_step(mut self, step: Step) -> Plan {
        self.steps.push(step);
        self
    }

    pub fn then(mut self, op: InternalOp) -> Plan {
        self.then = Some(op);
        self
    }

    pub fn with_output(mut self) -> Plan {
        self.show_output = true;
        self
    }

    pub fn in_dir(mut self, cwd: PathBuf) -> Plan {
        self.cwd = Some(cwd);
        self
    }

    pub fn is_internal(&self) -> bool {
        self.steps.is_empty()
    }

    /// The shell-ish rendering shown in the preview line, e.g.
    /// `git stash push -m x && git switch main`. Internal plans read `grove: …`.
    pub fn display(&self, cwd: &Path) -> String {
        if self.steps.is_empty() {
            return match &self.then {
                Some(op) => format!("grove: {}", op.describe(cwd)),
                None => "grove: nothing to do".to_string(),
            };
        }
        self.steps.iter().map(|s| s.display(cwd)).collect::<Vec<_>>().join(" && ")
    }
}
