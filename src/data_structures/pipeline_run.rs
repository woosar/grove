use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::{Job, PipelineEvent, StepState};
use crate::helpers::relative_to;

/// Output lines kept for scrolling; a long build drops its oldest lines first.
const MAX_LINES: usize = 20_000;

/// The current (or last) pipeline run: per-step progress and captured output.
pub struct PipelineRun {
    pub name: String,
    pub steps: Vec<String>,
    pub states: Vec<StepState>,
    /// The worktree it started in.
    pub root: PathBuf,
    /// `(is a step header, text)`
    pub output: VecDeque<(bool, String)>,
    /// Lines received over the whole run, including dropped ones.
    pub total_lines: usize,
    pub finished: bool,
    /// The user has seen how it ended; the header stops pointing at it.
    pub seen: bool,
    cancel: Arc<AtomicBool>,
    started: Instant,
    took: Option<Duration>,
}

impl PipelineRun {
    pub fn new(name: String, steps: Vec<String>, root: PathBuf) -> Self {
        PipelineRun {
            name,
            states: vec![StepState::Pending; steps.len()],
            steps,
            root,
            output: VecDeque::new(),
            total_lines: 0,
            finished: false,
            seen: false,
            cancel: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
            took: None,
        }
    }

    pub fn job(&self) -> Job {
        Job::Pipeline { steps: self.steps.clone(), root: self.root.clone(), cancel: self.cancel.clone() }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_running(&self) -> bool {
        !self.finished
    }

    pub fn failed(&self) -> bool {
        self.states.iter().any(|s| matches!(s, StepState::Failed(_)))
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn elapsed(&self) -> Duration {
        self.took.unwrap_or_else(|| self.started.elapsed())
    }

    /// Steps finished successfully so far.
    pub fn done_count(&self) -> usize {
        self.states.iter().filter(|s| **s == StepState::Done).count()
    }

    fn push(&mut self, header: bool, text: String) {
        if self.output.len() == MAX_LINES {
            self.output.pop_front();
        }
        self.output.push_back((header, text));
        self.total_lines += 1;
    }

    pub fn on_event(&mut self, event: PipelineEvent) {
        match event {
            PipelineEvent::StepStarted { step, dir } => {
                self.states[step] = StepState::Running;
                let at = relative_to(&dir, &self.root);
                let at = if at == "." { String::new() } else { format!("{at} ") };
                self.push(true, format!("{at}$ {}", self.steps[step]));
            }
            PipelineEvent::Output(line) => self.push(false, line),
            PipelineEvent::StepFinished { step, state } => {
                self.states[step] = state;
                match state {
                    StepState::Failed(Some(code)) => self.push(true, format!("exit {code}")),
                    StepState::Cancelled => self.push(true, "cancelled".to_string()),
                    _ => {}
                }
            }
            PipelineEvent::Finished => {
                self.finished = true;
                self.took = Some(self.started.elapsed());
            }
        }
    }
}
