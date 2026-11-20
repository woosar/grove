use std::path::PathBuf;

use super::{StepState, StyledLine};

/// Progress of a running pipeline, streamed to the UI thread.
#[derive(Clone, Debug)]
pub enum PipelineEvent {
    StepStarted { step: usize, dir: PathBuf },
    Output(StyledLine),
    /// The unfinished line written so far; the next output replaces it.
    Partial(StyledLine),
    StepFinished { step: usize, state: StepState },
    Finished,
}
