/// How far one pipeline step got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Running,
    Done,
    /// The exit code; `None` when it never started or was killed by a signal.
    Failed(Option<i32>),
    Cancelled,
}
