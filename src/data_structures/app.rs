use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use ratatui::crossterm::event::{self, DisableFocusChange, EnableFocusChange, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use super::{
    Action, ActionPopup, BranchesScreen, Cli, ConfirmModal, Ctx, DashboardScreen, Effect, ErrorDialog, GroveError, HelpModal,
    InternalOp, Job, JobResult, Journal, JournalEntry, JournalScreen, KeyBinding, LogScreen, Modal, Pager, Pipeline, PipelineEvent,
    PipelineRun, PipelinesScreen, Plan, PreflightInfo, PromptKind, PromptModal, QuickSwitcher, RunOutcome, RunRequest, ScreenId,
    Snapshot, Theme, Verdict, Worker, WorktreesScreen,
};
use crate::helpers::{
    copy_to_clipboard, home_tilde, norm_path, relative_to, same_path, short_oid, str_width, truecolor_supported, truncate,
};

/// Poll `git status` this often while idle.
const PROBE_EVERY: Duration = Duration::from_secs(5);
/// Full reload this often, to catch ref changes `status` doesn't show.
const REFRESH_EVERY: Duration = Duration::from_secs(30);
const MESSAGE_FOR: Duration = Duration::from_secs(6);
/// Longest wait for input between frames while a pipeline streams output, and otherwise.
const FRAME_BUSY: Duration = Duration::from_millis(16);
const FRAME_IDLE: Duration = Duration::from_millis(80);
/// Input handled before a frame is drawn regardless.
const MAX_EVENTS_PER_FRAME: usize = 64;

/// Borrow the pieces of `App` that screens read, leaving the screens themselves free
/// for `&mut` access.
macro_rules! ctx {
    ($app:expr) => {
        Ctx {
            snap: &$app.snap,
            theme: &$app.theme,
            preflight: &$app.preflight,
            tail: &$app.tail,
            last_used: &$app.last_used,
            copy_files: &$app.copy_files,
            pipelines: &$app.pipelines,
            pipeline_run: $app.pipeline_run.as_ref(),
        }
    };
}

/// App state, event loop, screen routing and the modal stack.
pub struct App {
    theme: Theme,
    journal: Arc<Mutex<Journal>>,
    worker: Worker,
    /// Open worktree tabs (top-level paths); `active` is grove's current context.
    tabs: Vec<PathBuf>,
    active: usize,
    snap: Snapshot,
    screen: ScreenId,
    dashboard: DashboardScreen,
    log: LogScreen,
    branches: BranchesScreen,
    worktrees: WorktreesScreen,
    journal_view: JournalScreen,
    pipelines_view: PipelinesScreen,
    pipelines: Vec<Pipeline>,
    pipeline_run: Option<PipelineRun>,
    modals: Vec<Modal>,
    preflight: HashMap<String, PreflightInfo>,
    preflight_pending: HashSet<String>,
    tail: Vec<JournalEntry>,
    last_used: HashMap<String, i64>,
    copy_files: Vec<PathBuf>,
    /// Command line currently running, for the spinner.
    running: Option<String>,
    queued: usize,
    message: Option<(String, Color, Instant)>,
    refresh_seq: u64,
    applied_seq: u64,
    last_probe: Instant,
    last_refresh: Instant,
    probe_bytes: Vec<u8>,
    probe_pending: bool,
    /// Clock for the spinner, so it turns at the same pace however often frames come.
    started: Instant,
    quit: bool,
}

impl App {
    /// Opens the repository and the journal; fails with a plain-language message when
    /// the path isn't inside a repository.
    pub fn new(cli: Cli) -> Result<App> {
        let cwd = match &cli.path {
            Some(p) => p.clone(),
            None => std::env::current_dir()?,
        };
        if let Err(out) = Snapshot::locate(&cwd) {
            let stderr = out.stderr_text();
            let err = GroveError::classify(&["rev-parse".to_string()], out.status, &stderr);
            let (pre, name, post) = err.headline();
            bail!("{pre}{name}{post}\n{}\n\n{}", err.explanation(), stderr.trim());
        }
        let snap = Snapshot::load(&cwd)?;

        let journal = match cli.journal.clone().or_else(Journal::default_path) {
            Some(path) => Journal::open(&path).or_else(|e| {
                eprintln!("grove: journal unavailable ({e:#}); history won't persist this session");
                Journal::open_in_memory()
            })?,
            None => Journal::open_in_memory()?,
        };
        let journal = Arc::new(Mutex::new(journal));
        let theme = Theme::new(truecolor_supported(), cli.ascii);
        let mut app = App {
            theme,
            worker: Worker::start(journal.clone()),
            journal,
            tabs: vec![snap.workdir.clone()],
            active: 0,
            snap,
            screen: ScreenId::Dashboard,
            dashboard: DashboardScreen::default(),
            log: LogScreen::default(),
            branches: BranchesScreen::default(),
            worktrees: WorktreesScreen::default(),
            journal_view: JournalScreen::default(),
            pipelines_view: PipelinesScreen::default(),
            pipelines: Vec::new(),
            pipeline_run: None,
            modals: Vec::new(),
            preflight: HashMap::new(),
            preflight_pending: HashSet::new(),
            tail: Vec::new(),
            last_used: HashMap::new(),
            copy_files: cli.copy,
            running: None,
            queued: 0,
            message: None,
            refresh_seq: 0,
            applied_seq: 0,
            last_probe: Instant::now(),
            last_refresh: Instant::now(),
            probe_bytes: Vec::new(),
            probe_pending: false,
            started: Instant::now(),
            quit: false,
        };
        app.reload_journal_data();
        Ok(app)
    }

    /// Opens the repository, sets up the terminal, runs until quit, and restores it.
    pub fn launch(cli: Cli) -> Result<()> {
        let mut app = App::new(cli)?;
        let mut terminal = ratatui::try_init()?;
        let _ = execute!(std::io::stdout(), EnableFocusChange);
        let result = app.run(&mut terminal);
        let _ = execute!(std::io::stdout(), DisableFocusChange);
        ratatui::restore();
        app.stop_pipeline();
        result
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            terminal.draw(|f| self.draw(f))?;
            // streaming output wants frequent frames; idle, a slower beat is plenty
            let wait = if self.pipeline_running() { FRAME_BUSY } else { FRAME_IDLE };
            if event::poll(wait)? {
                // handle everything already queued (key repeat) before the next frame,
                // so holding a key doesn't fall behind one redraw per press
                let mut handled = 0;
                loop {
                    match event::read()? {
                        Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
                        Event::FocusGained => self.refresh(),
                        _ => {}
                    }
                    handled += 1;
                    if self.quit || handled >= MAX_EVENTS_PER_FRAME || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
            while let Some(result) = self.worker.try_recv() {
                self.on_result(result);
            }
            self.on_tick();
        }
        Ok(())
    }

    fn repo_key(&self) -> String {
        norm_path(&self.snap.common_dir)
    }

    fn message(&mut self, text: impl Into<String>, color: Color) {
        let text = text.into();
        self.message = (!text.is_empty()).then(|| (text, color, Instant::now()));
    }

    fn reload_journal_data(&mut self) {
        let repo = self.repo_key();
        if let Ok(j) = self.journal.lock() {
            self.tail = j.tail(&repo, 7);
            self.last_used = j.last_used(&repo);
            self.pipelines = j.pipelines(&repo);
        }
        self.journal_view.needs_reload = true;
    }

    fn refresh(&mut self) {
        self.refresh_seq += 1;
        self.last_refresh = Instant::now();
        self.worker.send(Job::Refresh { cwd: self.tabs[self.active].clone(), seq: self.refresh_seq });
    }

    // ---- background ------------------------------------------------------------------

    fn on_tick(&mut self) {
        let idle = self.queued == 0;
        if idle && !self.probe_pending && self.last_probe.elapsed() >= PROBE_EVERY {
            self.probe_pending = true;
            self.last_probe = Instant::now();
            self.worker.send(Job::Probe { cwd: self.snap.workdir.clone() });
        }
        if idle && self.last_refresh.elapsed() >= REFRESH_EVERY {
            self.refresh();
        }
        let wanted = match self.modals.last() {
            Some(m) => m.selected_branch(),
            None => {
                let ctx = ctx!(self);
                match self.screen {
                    ScreenId::Dashboard => self.dashboard.selected_branch(&ctx),
                    ScreenId::Branches => self.branches.selected_branch(&ctx),
                    _ => None,
                }
            }
        };
        if let Some(target) = wanted
            && !self.preflight.contains_key(&target)
            && self.preflight_pending.insert(target.clone())
        {
            let dirty = self.snap.status.dirty_paths().into_iter().cloned().collect();
            self.worker.send(Job::Preflight { cwd: self.snap.workdir.clone(), target, dirty });
        }
        if self.screen == ScreenId::Pipelines
            && let Some(run) = self.pipeline_run.as_mut().filter(|r| r.finished)
        {
            run.seen = true;
        }
        if self.screen == ScreenId::Log {
            for job in self.log.jobs(&self.snap) {
                self.worker.send(job);
            }
        }
        if self.journal_view.needs_reload && self.screen == ScreenId::Journal {
            let repo = self.repo_key();
            if let Ok(j) = self.journal.lock() {
                self.journal_view.reload(&j, &repo);
            }
        }
    }

    fn on_result(&mut self, result: JobResult) {
        match result {
            JobResult::Snapshot { cwd, seq, result } => {
                if seq <= self.applied_seq || !same_path(&cwd, &self.tabs[self.active]) {
                    return;
                }
                self.applied_seq = seq;
                match result {
                    Ok(snap) => {
                        let moved = snap.status.oid != self.snap.status.oid
                            || snap.branches.iter().map(|b| &b.oid).ne(self.snap.branches.iter().map(|b| &b.oid))
                            || !same_path(&snap.workdir, &self.snap.workdir);
                        self.snap = *snap;
                        self.tabs[self.active] = self.snap.workdir.clone();
                        self.preflight.clear();
                        self.preflight_pending.clear();
                        if moved {
                            self.log.invalidate();
                        }
                        self.reload_journal_data();
                    }
                    Err(e) => {
                        // a tab that can't be read (deleted, not a repo) is closed again
                        if !same_path(&self.tabs[self.active], &self.snap.workdir) {
                            self.tabs.remove(self.active);
                            self.active = self.tabs.iter().position(|t| same_path(t, &self.snap.workdir)).unwrap_or(0);
                        }
                        self.message(format!("{} {e}", self.theme.g.cross), self.theme.err);
                    }
                }
            }
            JobResult::Probe { cwd, bytes } => {
                self.probe_pending = false;
                if same_path(&cwd, &self.snap.workdir) && bytes != self.probe_bytes {
                    let first = self.probe_bytes.is_empty();
                    self.probe_bytes = bytes;
                    if !first {
                        self.refresh();
                    }
                }
            }
            JobResult::Preflight { cwd, target, info } => {
                if same_path(&cwd, &self.snap.workdir) {
                    self.preflight_pending.remove(&target);
                    self.preflight.insert(target, info);
                }
            }
            JobResult::LogPage { generation, skip, commits, total } => self.log.on_page(generation, skip, commits, total),
            JobResult::Detail(detail) => self.log.on_detail(*detail),
            JobResult::Ran(outcome) => self.on_ran(*outcome),
            JobResult::Pipeline(event) => self.on_pipeline_event(event),
        }
    }

    fn on_pipeline_event(&mut self, event: PipelineEvent) {
        let finished = matches!(event, PipelineEvent::Finished);
        let Some(run) = self.pipeline_run.as_mut() else { return };
        run.on_event(event);
        if !finished {
            return;
        }
        let g = &self.theme.g;
        let secs = run.elapsed().as_secs();
        let (text, color) = if run.failed() {
            (format!("{} {} failed after {secs}s", g.cross, run.name), self.theme.err)
        } else if run.cancelled() {
            (format!("{} {} cancelled", g.cross, run.name), self.theme.err)
        } else {
            (format!("{} {} done in {secs}s", g.check, run.name), self.theme.ok)
        };
        self.message(text, color);
        // the commands may well have changed the working tree
        self.refresh();
    }

    fn pipeline_running(&self) -> bool {
        self.pipeline_run.as_ref().is_some_and(PipelineRun::is_running)
    }

    fn start_pipeline(&mut self, name: String, steps: Vec<String>) {
        if self.pipeline_running() {
            let color = self.theme.err;
            self.message("a pipeline is already running — x on the Pipelines screen cancels it", color);
            return;
        }
        let run = PipelineRun::new(name, steps, self.snap.workdir.clone());
        self.worker.send(run.job());
        self.pipeline_run = Some(run);
        self.pipelines_view.on_run_started();
        self.screen = ScreenId::Pipelines;
        self.message = None;
    }

    /// Cancels a running pipeline and waits briefly, so quitting leaves no build behind.
    fn stop_pipeline(&mut self) {
        let Some(run) = self.pipeline_run.as_ref().filter(|r| r.is_running()) else { return };
        run.cancel();
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.pipeline_running() && Instant::now() < deadline {
            match self.worker.try_recv() {
                Some(result) => self.on_result(result),
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    /// Runs a journal write and picks up the result; `Err` is shown to the user.
    fn store_pipeline<T>(&mut self, write: impl FnOnce(&Journal, &str) -> Result<T>) -> Option<T> {
        let repo = self.repo_key();
        let result = match self.journal.lock() {
            Ok(j) => write(&j, &repo).map_err(|e| e.to_string()),
            Err(_) => Err("the journal is unavailable".to_string()),
        };
        match result {
            Ok(value) => {
                self.reload_journal_data();
                Some(value)
            }
            Err(e) => {
                let color = self.theme.err;
                self.message(format!("{} couldn't save the pipeline: {e}", self.theme.g.cross), color);
                None
            }
        }
    }

    fn name_taken(&self, name: &str, except: i64) -> bool {
        self.pipelines.iter().any(|p| p.id != except && p.name == name)
    }

    fn on_ran(&mut self, outcome: RunOutcome) {
        self.queued = self.queued.saturating_sub(1);
        if self.queued == 0 {
            self.running = None;
        }
        let g = self.theme.g.clone();
        let command = outcome.plan.display(&outcome.cwd);
        match outcome.failure {
            Some(report) => {
                self.message(format!("{} {} failed", g.cross, truncate(&command, 80)), self.theme.err);
                self.modals.push(Modal::Error(ErrorDialog::new(report)));
            }
            None => {
                if outcome.plan.show_output {
                    if outcome.stdout.trim().is_empty() {
                        self.message(format!("{} {command} — no output", g.check), self.theme.ok);
                    } else {
                        self.modals.push(Modal::Pager(Pager::new(command.clone(), outcome.stdout)));
                    }
                } else {
                    self.message(format!("{} {command}", g.check), self.theme.ok);
                }
                if let Some(op) = outcome.plan.then.clone() {
                    self.apply_internal(op);
                }
            }
        }
        if !outcome.warnings.is_empty() {
            self.message(format!("{} {}", g.warn, outcome.warnings.join("; ")), self.theme.accent);
        }
        self.refresh();
        self.reload_journal_data();
    }

    // ---- input -----------------------------------------------------------------------

    fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if let Some(top) = self.modals.last_mut() {
            let effect = top.handle_key(&key, &ctx!(self));
            if let Some(e) = effect {
                self.apply(e);
            }
            return;
        }
        let effect = {
            let ctx = ctx!(self);
            match self.screen {
                ScreenId::Dashboard => self.dashboard.handle_key(&key, &ctx),
                ScreenId::Log => self.log.handle_key(&key, &ctx),
                ScreenId::Branches => self.branches.handle_key(&key, &ctx),
                ScreenId::Worktrees => self.worktrees.handle_key(&key, &ctx),
                ScreenId::Journal => self.journal_view.handle_key(&key),
                ScreenId::Pipelines => self.pipelines_view.handle_key(&key, &ctx),
            }
        };
        if let Some(e) = effect {
            self.apply(e);
            return;
        }
        if self.screen == ScreenId::Worktrees && self.worktrees.form_open() {
            return;
        }
        self.global_key(key);
    }

    fn global_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Tab && ctrl {
            self.cycle_tab(1);
            return;
        }
        let KeyCode::Char(c) = key.code else {
            if key.code == KeyCode::Esc {
                self.message = None;
            }
            return;
        };
        if ctrl {
            return;
        }
        let f = ctx!(self).actions();
        match c {
            '1'..='6' => {
                self.screen = ScreenId::ALL[(c as u8 - b'1') as usize];
                self.message = None;
            }
            'b' => {
                let m = Modal::Switcher(QuickSwitcher::new(&ctx!(self)));
                self.modals.push(m);
            }
            'f' => self.request_run(f.fetch(), "f".into(), None),
            'p' => self.request_run(f.pull(), "p".into(), None),
            'P' => self.request_run(f.push(), "P".into(), None),
            ':' => self.modals.push(Modal::Prompt(PromptModal::new(PromptKind::Raw, ""))),
            '!' => self.modals.push(Modal::Prompt(PromptModal::new(PromptKind::Shell, ""))),
            '?' => self.open_help(),
            'u' => self.undo(None),
            ']' => self.cycle_tab(1),
            '[' => self.cycle_tab(-1),
            'q' if self.pipeline_running() => {
                let color = self.theme.accent;
                self.message("a pipeline is running — x on the Pipelines screen cancels it, ctrl-c quits and kills it", color);
            }
            'q' => self.quit = true,
            _ => {}
        }
    }

    fn open_help(&mut self) {
        let modal = match self.screen_preview() {
            Some(a) => HelpModal::for_action(&a),
            None => HelpModal::new(format!("? {}", self.screen.title()), self.keymap()),
        };
        self.modals.push(Modal::Help(modal));
    }

    fn keymap(&self) -> String {
        let mut out = String::from("Global\n");
        for (k, v) in [
            ("1–6", "screens"),
            ("tab / shift-tab", "cycle panes"),
            ("j/k, arrows", "move"),
            ("space", "actions for the selected item"),
            ("b", "quick switch (fuzzy)"),
            ("f / p / P", "fetch / pull / push"),
            (":", "raw git prompt (journaled)"),
            ("!", "run a shell command in the worktree root"),
            ("?", "explain the highlighted action"),
            ("u", "undo the last change (reflog)"),
            ("ctrl-tab or ] / [", "cycle worktree tabs"),
            ("esc", "close"),
            ("q", "quit"),
        ] {
            out.push_str(&format!("  {k:<20} {v}\n"));
        }
        out.push_str(&format!("\n{}\n", self.screen.title()));
        for (k, v) in self.screen_hints() {
            out.push_str(&format!("  {k:<20} {v}\n"));
        }
        out
    }

    // ---- effects ---------------------------------------------------------------------

    fn apply(&mut self, effect: Effect) {
        match effect {
            Effect::Nothing => {}
            Effect::Run { action, via } => self.request_run(action, via, None),
            Effect::RunConfirmed { action, via, parent } => self.execute(action.plan, via, action.label.to_string(), parent),
            Effect::Remedy { remedy, parent } => {
                if remedy.destructive {
                    let mut action = Action::new(remedy.key, "remedy", remedy.plan.clone()).explain(remedy.consequence.clone()).destructive();
                    action.confirm = Some(format!("{}?{}", remedy.label, if remedy.recoverable { "" } else { " This can't be undone." }));
                    self.modals.push(Modal::Confirm(ConfirmModal { action, via: "remedy".into(), parent }));
                } else {
                    self.execute(remedy.plan, "remedy".into(), remedy.label, parent);
                }
            }
            Effect::Internal(op) => self.apply_internal(op),
            Effect::Open(m) => self.modals.push(*m),
            Effect::Close => {
                self.modals.pop();
            }
            Effect::CloseThen(e) => {
                self.modals.pop();
                self.apply(*e);
            }
            Effect::Message(m) => {
                let color = self.theme.accent;
                self.message(m, color);
            }
            Effect::SetLogFilter(s) => self.log.set_filter(&s),
            Effect::SetJournalSearch(s) => self.journal_view.set_search(s),
            Effect::UndoEntry(e) => self.undo(Some(*e)),
        }
    }

    /// Checks the verdict and confirmation, then executes.
    fn request_run(&mut self, action: Action, via: String, parent: Option<i64>) {
        let g = &self.theme.g;
        if let Verdict::Blocked { reason, alternative } = &action.verdict {
            let alt = alternative.map(|k: KeyBinding| format!(" — press {} instead", k.label(g))).unwrap_or_default();
            let text = format!("{} {}: blocked — {reason}{alt}", g.warn, action.label);
            let color = self.theme.err;
            self.message(text, color);
            return;
        }
        if action.confirm.is_some() {
            self.modals.push(Modal::Confirm(ConfirmModal { action, via, parent }));
            return;
        }
        self.execute(action.plan, via, action.label.to_string(), parent);
    }

    fn execute(&mut self, plan: Plan, via: String, label: String, parent: Option<i64>) {
        if plan.is_internal() {
            if let Some(op) = plan.then {
                self.apply_internal(op);
            }
            return;
        }
        let cwd = plan.cwd.clone().unwrap_or_else(|| self.snap.workdir.clone());
        self.running = Some(plan.display(&cwd));
        self.message = None;
        self.queued += 1;
        let req = RunRequest { plan, via, label, parent, cwd, snapshot: self.snap.clone() };
        self.worker.send(Job::Run(Box::new(req)));
    }

    fn apply_internal(&mut self, op: InternalOp) {
        match op {
            InternalOp::OpenWorktree(path) => self.open_tab(path),
            InternalOp::GoTo(screen) => {
                self.screen = screen;
                if screen == ScreenId::Dashboard {
                    self.dashboard.focus_status();
                }
            }
            InternalOp::CommitDialog { then_switch } => {
                self.modals.push(Modal::Prompt(PromptModal::new(PromptKind::Commit { then_switch }, "")));
            }
            InternalOp::Copy(text) => {
                copy_to_clipboard(&text);
                let color = self.theme.ok;
                self.message(format!("{} copied to clipboard ({} chars)", self.theme.g.check, text.chars().count()), color);
            }
            InternalOp::Prompt(kind) => {
                let prefill = match &kind {
                    PromptKind::SetUpstream(b) => format!("{}/{b}", self.snap.remote()),
                    PromptKind::RenameBranch(b) => b.clone(),
                    PromptKind::MoveWorktree(p) => relative_to(p, &self.snap.workdir),
                    PromptKind::LogFilter => self.log.filter_input(),
                    PromptKind::RenamePipeline(p) => p.name.clone(),
                    PromptKind::PipelineStep { pipeline, at, replace: true } => pipeline.steps[*at].clone(),
                    _ => String::new(),
                };
                self.modals.push(Modal::Prompt(PromptModal::new(kind, prefill)));
            }
            InternalOp::NewWorktreeForm(branch) => {
                self.screen = ScreenId::Worktrees;
                let ctx = ctx!(self);
                self.worktrees.open_form(branch, &ctx);
            }
            InternalOp::ResetMenu(oid) => {
                let actions = ctx!(self).actions().reset_actions(&oid);
                let title = format!("reset {} to {}", self.snap.head_label(), short_oid(&oid));
                self.modals.push(Modal::Popup(ActionPopup::new(title, "pick a mode", actions, None)));
            }
            InternalOp::EditRaw(cmd) => self.modals.push(Modal::Prompt(PromptModal::new(PromptKind::Raw, cmd))),
            InternalOp::RunPipeline(p) => self.start_pipeline(p.name, p.steps),
            InternalOp::RunShell(line) => self.start_pipeline(format!("! {line}"), vec![line]),
            InternalOp::CancelPipeline => match self.pipeline_run.as_ref().filter(|r| r.is_running()) {
                Some(run) => run.cancel(),
                None => {
                    let color = self.theme.dim;
                    self.message("nothing is running", color);
                }
            },
            InternalOp::CreatePipeline(name) => {
                if self.name_taken(&name, 0) {
                    let color = self.theme.err;
                    self.message(format!("there already is a pipeline called {name}"), color);
                    return;
                }
                if let Some(pipeline) = self.store_pipeline(|j, repo| j.create_pipeline(repo, &name)) {
                    self.screen = ScreenId::Pipelines;
                    self.pipelines_view.select(self.pipelines.len().saturating_sub(1));
                    self.modals.push(Modal::Prompt(PromptModal::new(PromptKind::PipelineStep { pipeline, at: 0, replace: false }, "")));
                }
            }
            InternalOp::SavePipeline { pipeline, next } => {
                if self.name_taken(&pipeline.name, pipeline.id) {
                    let color = self.theme.err;
                    self.message(format!("there already is a pipeline called {}", pipeline.name), color);
                    return;
                }
                if self.store_pipeline(|j, _| j.update_pipeline(&pipeline)).is_some()
                    && let Some(at) = next
                {
                    self.pipelines_view.select_step(at.saturating_sub(1));
                    self.modals.push(Modal::Prompt(PromptModal::new(PromptKind::PipelineStep { pipeline, at, replace: false }, "")));
                }
            }
            InternalOp::DeletePipeline(p) => {
                self.store_pipeline(|j, _| j.delete_pipeline(p.id));
            }
        }
    }

    fn open_tab(&mut self, path: PathBuf) {
        let path = if path.is_relative() { self.snap.workdir.join(path) } else { path };
        let idx = match self.tabs.iter().position(|t| same_path(t, &path)) {
            Some(i) => i,
            None => {
                self.tabs.push(path.clone());
                self.tabs.len() - 1
            }
        };
        if idx == self.active {
            let color = self.theme.dim;
            self.message("already in that worktree", color);
            return;
        }
        self.switch_tab(idx);
    }

    fn switch_tab(&mut self, idx: usize) {
        self.active = idx;
        self.preflight.clear();
        self.preflight_pending.clear();
        self.probe_bytes.clear();
        self.log = LogScreen::default();
        let color = self.theme.accent;
        self.message(format!("opening {}…", home_tilde(&self.tabs[idx])), color);
        self.refresh();
    }

    fn cycle_tab(&mut self, delta: isize) {
        if self.tabs.len() < 2 {
            let color = self.theme.dim;
            self.message("only one worktree tab open — open another from the Worktrees screen", color);
            return;
        }
        let n = self.tabs.len() as isize;
        let idx = ((self.active as isize + delta).rem_euclid(n)) as usize;
        self.switch_tab(idx);
    }

    fn undo(&mut self, entry: Option<JournalEntry>) {
        let repo = self.repo_key();
        let entry = entry.or_else(|| self.journal.lock().ok().and_then(|j| j.latest_mutating(&repo)));
        let Some(e) = entry else {
            let color = self.theme.dim;
            self.message("nothing to undo yet", color);
            return;
        };
        match e.undo(self.snap.status.oid.as_deref()) {
            Ok((plan, explain)) => {
                let plan = if same_path(Path::new(&e.worktree), &self.snap.workdir) { plan } else { plan.in_dir(PathBuf::from(&e.worktree)) };
                let action = Action::new(KeyBinding::ch('u'), "undo", plan)
                    .explain(format!("undoes #{} ({}): {explain}", e.id, e.command()))
                    .confirm(format!("Undo journal entry #{}: {}?", e.id, e.command()));
                self.request_run(action, "u".into(), None);
            }
            Err(reason) => {
                let color = self.theme.dim;
                self.message(format!("undo #{} ({}): {reason}", e.id, e.command()), color);
            }
        }
    }

    // ---- drawing ---------------------------------------------------------------------

    fn screen_preview(&self) -> Option<Action> {
        let ctx = ctx!(self);
        match self.screen {
            ScreenId::Dashboard => self.dashboard.preview(&ctx),
            ScreenId::Log => self.log.preview(&ctx),
            ScreenId::Branches => self.branches.preview(&ctx),
            ScreenId::Worktrees => self.worktrees.preview(&ctx),
            ScreenId::Journal => self.journal_view.preview(&ctx),
            ScreenId::Pipelines => self.pipelines_view.preview(&ctx),
        }
    }

    fn screen_hints(&self) -> Vec<(&'static str, &'static str)> {
        match self.screen {
            ScreenId::Dashboard => self.dashboard.hints(),
            ScreenId::Log => self.log.hints(),
            ScreenId::Branches => self.branches.hints(),
            ScreenId::Worktrees => self.worktrees.hints(),
            ScreenId::Journal => self.journal_view.hints(),
            ScreenId::Pipelines => self.pipelines_view.hints(),
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        f.render_widget(Block::new().style(Style::new().bg(self.theme.bg).fg(self.theme.fg)), area);
        let [header, _, body, footer] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(4), Constraint::Length(3)]).areas(area);
        self.draw_header(f, header);
        {
            let ctx = ctx!(self);
            match self.screen {
                ScreenId::Dashboard => self.dashboard.draw(f, body, &ctx),
                ScreenId::Log => self.log.draw(f, body, &ctx),
                ScreenId::Branches => self.branches.draw(f, body, &ctx),
                ScreenId::Worktrees => self.worktrees.draw(f, body, &ctx),
                ScreenId::Journal => self.journal_view.draw(f, body, &ctx),
                ScreenId::Pipelines => self.pipelines_view.draw(f, body, &ctx),
            }
        }
        let screen_area = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(3));
        if self.modals.iter().any(Modal::dims_background) {
            let shade = self.theme.shade;
            let buf = f.buffer_mut();
            for y in screen_area.top()..screen_area.bottom() {
                for x in screen_area.left()..screen_area.right() {
                    buf[(x, y)].set_fg(shade);
                }
            }
        }
        let ctx = ctx!(self);
        for m in &mut self.modals {
            m.draw(f, screen_area, body, &ctx);
        }
        self.draw_footer(f, footer);
    }

    fn draw_header(&self, f: &mut Frame, area: Rect) {
        let t = &self.theme;
        let g = &t.g;
        let st = &self.snap.status;
        let mut left = vec![
            Span::styled("grove", t.bold(t.accent)),
            Span::raw("  "),
            Span::styled(home_tilde(&self.snap.workdir), t.dimmed()),
        ];
        if self.tabs.len() > 1 {
            left.push(Span::styled(format!(" [{}/{}]", self.active + 1, self.tabs.len()), t.s(t.tag)));
        }
        left.push(Span::raw("  "));
        left.push(Span::styled(format!("{} {}", g.branch, self.snap.head_label()), t.s(t.branch)));
        if st.upstream.is_some() {
            left.push(Span::raw("  "));
            left.push(Span::styled(format!("{}{}", g.up, st.ahead), t.s(if st.ahead > 0 { t.ok } else { t.dim })));
            left.push(Span::raw(" "));
            left.push(Span::styled(format!("{}{}", g.down, st.behind), t.s(if st.behind > 0 { t.accent } else { t.dim })));
        }
        let modified = st.modified_count();
        if modified > 0 {
            left.push(Span::raw("  "));
            left.push(Span::styled(format!("{} {modified} modified", g.dot), t.s(t.accent)));
        }
        let untracked = st.count(super::ChangeArea::Untracked);
        if untracked > 0 {
            left.push(Span::raw("  "));
            left.push(Span::styled(format!("+{untracked} untracked"), t.dimmed()));
        }
        // a build started earlier stays visible from every screen, and so does its failure
        if let Some(run) = &self.pipeline_run {
            if run.is_running() {
                let spin = g.spinner[(self.started.elapsed().as_millis() / 160) as usize % g.spinner.len()];
                left.push(Span::raw("  "));
                left.push(Span::styled(format!("{spin} {}", truncate(&run.name, 24)), t.s(t.accent)));
            } else if run.failed() && !run.seen {
                left.push(Span::raw("  "));
                left.push(Span::styled(format!("{} {}", g.cross, truncate(&run.name, 24)), t.s(t.err)));
            }
        }
        let left_w: usize = left.iter().map(|s| s.width()).sum();
        let full = (area.width as usize) >= left_w + 78;
        let mut right = Vec::new();
        for s in ScreenId::ALL {
            let label = if full || s == self.screen { format!(" {} {} ", s.number(), s.title()) } else { format!(" {} ", s.number()) };
            let style = if s == self.screen { Style::new().bg(t.sel).fg(t.accent) } else { t.dimmed() };
            right.push(Span::styled(label, style));
            right.push(Span::raw(" "));
        }
        let right_w: usize = right.iter().map(|s| s.width()).sum();
        let gap = (area.width as usize).saturating_sub(left_w + right_w);
        left.push(Span::raw(" ".repeat(gap)));
        left.extend(right);
        f.render_widget(Paragraph::new(Line::from(left)), area);
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect) {
        let t = &self.theme;
        let g = &t.g;
        let w = area.width as usize;
        let [rule, preview, hints] = Layout::vertical([Constraint::Length(1); 3]).areas(area);
        f.render_widget(Paragraph::new(Span::styled("─".repeat(w), t.s(t.border))), rule);

        let line = if let Some(cmd) = &self.running {
            let spin = g.spinner[(self.started.elapsed().as_millis() / 160) as usize % g.spinner.len()];
            Line::from(vec![
                Span::styled(format!("{spin} "), t.s(t.accent)),
                Span::styled("running ", t.dimmed()),
                Span::styled(truncate(cmd, w.saturating_sub(12)), t.s(t.accent)),
                Span::styled(" …", t.dimmed()),
            ])
        } else if let Some((text, color, _)) = self.message.as_ref().filter(|m| m.2.elapsed() < MESSAGE_FOR) {
            Line::from(Span::styled(truncate(text, w), Style::new().fg(*color)))
        } else {
            let ctx = ctx!(self);
            let action = match self.modals.last() {
                Some(m) => m.preview(&ctx),
                None => self.screen_preview(),
            };
            match action {
                Some(a) => self.preview_line(&a, w),
                None => Line::from(Span::styled(format!("{} ", g.prompt), t.s(t.accent))),
            }
        };
        f.render_widget(Paragraph::new(line), preview);

        let items = match self.modals.last() {
            Some(m) => m.hints(),
            None => self.screen_hints(),
        };
        let mut shown = Vec::new();
        let mut used = 0;
        for item in items {
            let width = str_width(item.0) + str_width(item.1) + 6;
            if used + width > w {
                break;
            }
            used += width;
            shown.push(item);
        }
        f.render_widget(Paragraph::new(t.hints(&shown)), hints);
    }

    /// `› git switch x — explanation`, with the verdict appended when it isn't Ok.
    fn preview_line(&self, a: &Action, width: usize) -> Line<'static> {
        let t = &self.theme;
        let g = &t.g;
        let mut spans = vec![Span::styled(format!("{} ", g.prompt), t.s(t.accent))];
        let explain = if a.plan.is_internal() {
            spans.push(Span::styled(a.plan.display(&self.snap.workdir), t.text()));
            format!(" {} no git command runs; {}", g.dash, a.explain)
        } else {
            let cmd = a.plan.display(&self.snap.workdir);
            spans.push(Span::styled(cmd, if a.destructive { t.s(t.err) } else { t.s(t.accent) }));
            format!(" {} {}", g.dash, a.explain)
        };
        let verdict = match &a.verdict {
            Verdict::Ok => None,
            Verdict::Warn(w) => Some((format!("  {} {w}", g.warn), t.accent)),
            Verdict::Blocked { reason, .. } => Some((format!("  {} blocked: {reason}", g.warn), t.err)),
        };
        let used: usize = spans.iter().map(|s| s.width()).sum();
        let verdict_w = verdict.as_ref().map_or(0, |v| str_width(&v.0));
        let room = width.saturating_sub(used + verdict_w);
        if !a.explain.is_empty() {
            spans.push(Span::styled(truncate(&explain, room), t.dimmed()));
        }
        if let Some((text, color)) = verdict {
            spans.push(Span::styled(text, Style::new().fg(color)));
        }
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyEventState;

    use super::super::JournalFilter;
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "grove")
            .env("GIT_AUTHOR_EMAIL", "grove@example.com")
            .env("GIT_COMMITTER_NAME", "grove")
            .env("GIT_COMMITTER_EMAIL", "grove@example.com")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// main + two feature branches, one checked out in a second worktree, and an
    /// uncommitted edit to a file that differs on main.
    fn scenario(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("grove-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "core.autocrlf", "false"]);
        git(&repo, &["config", "user.name", "grove"]);
        git(&repo, &["config", "user.email", "grove@example.com"]);
        std::fs::write(repo.join("Cargo.toml"), "[package]\nversion = \"0.2.9\"\n").unwrap();
        std::fs::write(repo.join("store.rs"), "fn store() {}\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "init: skeleton"]);
        git(&repo, &["switch", "-qc", "feat/log-graph"]);
        std::fs::write(repo.join("lanes.rs"), "fn lanes() {}\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "graph: allocate lanes"]);
        git(&repo, &["switch", "-q", "main"]);
        std::fs::write(repo.join("Cargo.toml"), "[package]\nversion = \"0.3.0\"\n").unwrap();
        git(&repo, &["commit", "-qam", "release: v0.3.0"]);
        git(&repo, &["switch", "-qc", "feat/journal", "HEAD~1"]);
        std::fs::write(repo.join("store.rs"), "fn store() { persist() }\n").unwrap();
        git(&repo, &["commit", "-qam", "journal: persist"]);
        git(&repo, &["worktree", "add", "-q", "../repo.wt/log-graph", "feat/log-graph"]);
        // an uncommitted edit to a file that main changed
        std::fs::write(repo.join("Cargo.toml"), "[package]\nversion = \"0.2.9\"\n[dependencies]\nrusqlite = \"1\"\n").unwrap();
        (root, repo)
    }

    fn app_for(repo: &Path, root: &Path) -> App {
        App::new(Cli { path: Some(repo.to_path_buf()), ascii: false, copy: vec![], journal: Some(root.join("journal.db")) })
            .unwrap()
    }

    fn press(app: &mut App, code: KeyCode) {
        let modifiers = match code {
            KeyCode::Char(c) if c.is_ascii_uppercase() => KeyModifiers::SHIFT,
            _ => KeyModifiers::NONE,
        };
        app.on_key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    }

    fn typing(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    /// Pumps worker results until nothing has arrived for a while and nothing runs.
    fn settle(app: &mut App) {
        let mut quiet = 0;
        for _ in 0..500 {
            let mut got = false;
            while let Some(r) = app.worker.try_recv() {
                app.on_result(r);
                got = true;
            }
            app.on_tick();
            let busy = app.queued > 0 || app.applied_seq < app.refresh_seq || app.pipeline_running();
            quiet = if got || busy { 0 } else { quiet + 1 };
            if quiet > 15 {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn render(app: &mut App) -> String {
        render_at(app, 160, 42)
    }

    fn render_at(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        if std::env::var_os("GROVE_PRINT").is_some() {
            println!("{out}");
        }
        out
    }

    fn journal(app: &App, readonly: bool) -> Vec<JournalEntry> {
        app.journal.lock().unwrap().query(&JournalFilter { show_readonly: readonly, ..Default::default() })
    }

    #[test]
    fn renders_every_screen_from_a_real_repo() {
        let (root, repo) = scenario("screens");
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        for (key, needle) in [
            ('1', "Branches · recent"),
            ('2', "Log · --all --topo-order"),
            ('3', "Branches · local 3"),
            ('4', "all share one .git"),
            ('5', "Sessions"),
            ('6', "Pipelines · 0"),
        ] {
            press(&mut app, KeyCode::Char(key));
            settle(&mut app);
            let screen = render(&mut app);
            assert!(screen.contains(needle), "screen {key} lacks {needle:?}:\n{screen}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn small_terminals_stack_panes_without_panicking() {
        let (root, repo) = scenario("small");
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        for key in ['1', '2', '3', '4', '5', '6'] {
            press(&mut app, KeyCode::Char(key));
            settle(&mut app);
            for (w, h) in [(120, 35), (90, 28), (40, 12)] {
                render_at(&mut app, w, h);
            }
        }
        press(&mut app, KeyCode::Char('b'));
        render_at(&mut app, 40, 12);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn switching_over_local_edits_opens_the_error_dialog() {
        let (root, repo) = scenario("overwrite");
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        press(&mut app, KeyCode::Char('b'));
        typing(&mut app, "main");
        settle(&mut app);
        // the exact command is previewed before anything runs
        assert!(render(&mut app).contains("› git switch main"));
        press(&mut app, KeyCode::Enter);
        settle(&mut app);
        let Some(Modal::Error(dialog)) = app.modals.last() else { panic!("no error dialog:\n{}", render(&mut app)) };
        let labels: Vec<&str> = dialog.report.remedies.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["open main in a worktree", "stash · switch · re-apply", "commit first, then switch", "discard my edits & switch"]);
        assert_eq!(dialog.report.affected[0].name, "Cargo.toml");
        assert!(render(&mut app).contains("Can't switch to main"));
        // the same argv is in the journal afterwards
        let entries = journal(&app, false);
        assert_eq!(entries[0].argv, ["switch", "main"]);
        assert_eq!(entries[0].exit_code, Some(1));

        // the safest remedy runs, is linked to the failure, and opens the new worktree
        press(&mut app, KeyCode::Char('W'));
        settle(&mut app);
        assert_eq!(app.tabs.len(), 2, "{}", render(&mut app));
        let entries = journal(&app, false);
        assert_eq!(entries[0].via, "remedy");
        assert_eq!(entries[0].parent_id, Some(entries[1].id));
        assert!(entries[1].note.as_deref().unwrap_or("").contains("you chose"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn switching_to_a_branch_in_another_worktree_never_reaches_git() {
        let (root, repo) = scenario("blocked");
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        press(&mut app, KeyCode::Char('b'));
        typing(&mut app, "log-graph");
        settle(&mut app);
        let screen = render(&mut app);
        assert!(screen.contains("blocked: already checked out in ../repo.wt/log-graph"), "{screen}");
        press(&mut app, KeyCode::Enter);
        settle(&mut app);
        let entries = journal(&app, true);
        assert!(entries.is_empty(), "git ran: {:?}", entries.iter().map(|e| e.command()).collect::<Vec<_>>());
        assert_eq!(app.tabs.len(), 2, "jumped to the worktree instead");
        assert!(app.snap.workdir.ends_with("log-graph"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// `n`, a name, one line per step, an empty line to finish.
    fn record(app: &mut App, name: &str, steps: &[&str]) {
        press(app, KeyCode::Char('6'));
        press(app, KeyCode::Char('n'));
        typing(app, name);
        press(app, KeyCode::Enter);
        for step in steps {
            typing(app, step);
            press(app, KeyCode::Enter);
        }
        press(app, KeyCode::Enter);
        assert!(app.modals.is_empty(), "recording didn't end:\n{}", render(app));
    }

    #[test]
    fn a_recorded_pipeline_runs_from_the_root_of_whichever_worktree_is_active() {
        let (root, repo) = scenario("pipeline");
        let wt = root.join("repo.wt").join("log-graph");
        for checkout in [&repo, &wt] {
            std::fs::create_dir_all(checkout.join("src/app")).unwrap();
            std::fs::create_dir_all(checkout.join("web")).unwrap();
        }
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        record(&mut app, "rebuild", &["cd src/app", "echo built>built.txt", "cd", "cd web", "echo installed>installed.txt"]);

        // saved, for every worktree of the repo, across restarts
        drop(app);
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        assert_eq!(app.pipelines[0].steps.len(), 5);

        press(&mut app, KeyCode::Char('6'));
        press(&mut app, KeyCode::Enter);
        settle(&mut app);
        assert!(repo.join("src/app/built.txt").exists(), "{}", render(&mut app));
        assert!(repo.join("web/installed.txt").exists());
        assert!(render(&mut app).contains("done"));

        // same pipeline from the other worktree: relative paths land there instead
        app.apply_internal(InternalOp::OpenWorktree(wt.clone()));
        settle(&mut app);
        assert!(same_path(&app.snap.workdir, &wt));
        assert_eq!(app.pipelines.len(), 1, "pipelines are shared between worktrees");
        press(&mut app, KeyCode::Enter);
        settle(&mut app);
        assert!(wt.join("src/app/built.txt").exists(), "{}", render(&mut app));
        assert!(wt.join("web/installed.txt").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    /// Real tools, ignored by default: `cargo test real_dotnet -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs dotnet and pnpm, and network access"]
    fn real_dotnet_and_pnpm_pipeline() {
        let (root, repo) = scenario("pipeline-real");
        std::fs::create_dir_all(repo.join("web")).unwrap();
        std::fs::write(repo.join("web/package.json"), "{\"name\":\"t\",\"version\":\"1.0.0\"}\n").unwrap();
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        record(
            &mut app,
            "rebuild",
            &[
                "dotnet new console -o src/App --force",
                "cd src/App",
                "dotnet clean",
                "dotnet build",
                "cd",
                "cd web",
                "pnpm install",
            ],
        );
        press(&mut app, KeyCode::Enter);
        let started = Instant::now();
        let mut waited = 0;
        while app.pipeline_running() && waited < 15_000 {
            while let Some(r) = app.worker.try_recv() {
                app.on_result(r);
            }
            std::thread::sleep(Duration::from_millis(20));
            waited += 1;
        }
        settle(&mut app);
        let screen = render_at(&mut app, 140, 40);
        println!("{screen}\ntook {:?}", started.elapsed());
        let run = app.pipeline_run.as_ref().unwrap();
        assert!(!run.failed() && !run.is_running(), "{screen}");
        assert!(repo.join("src/App/bin").is_dir(), "built in the solution dir");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_failing_step_stops_the_pipeline_and_stays_visible() {
        let (root, repo) = scenario("pipeline-fail");
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        record(&mut app, "bad", &["echo first>first.txt", "exit 3", "echo never>never.txt"]);
        press(&mut app, KeyCode::Enter);
        // leave the screen right away: the failure must still show when it ends
        press(&mut app, KeyCode::Char('1'));
        settle(&mut app);
        assert!(repo.join("first.txt").exists());
        assert!(!repo.join("never.txt").exists(), "ran past the failure");
        let header = render(&mut app);
        assert!(header.lines().next().unwrap().contains("✗ bad"), "{header}");
        press(&mut app, KeyCode::Char('6'));
        settle(&mut app);
        let screen = render(&mut app);
        assert!(screen.contains("exit 3"), "{screen}");
        assert!(!screen.lines().next().unwrap().contains("✗ bad"), "still flagged after being seen:\n{screen}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_directory_fails_the_cd_step() {
        let (root, repo) = scenario("pipeline-cd");
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        record(&mut app, "lost", &["cd no/such/dir", "echo here>here.txt"]);
        press(&mut app, KeyCode::Enter);
        settle(&mut app);
        assert!(render(&mut app).contains("no such directory"));
        assert!(!repo.join("here.txt").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_one_off_command_runs_in_the_worktree_root_from_any_screen() {
        let (root, repo) = scenario("pipeline-oneoff");
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        press(&mut app, KeyCode::Char('3'));
        press(&mut app, KeyCode::Char('!'));
        typing(&mut app, "echo hi>hi.txt&& echo visible");
        press(&mut app, KeyCode::Enter);
        settle(&mut app);
        assert_eq!(app.screen, ScreenId::Pipelines);
        assert!(repo.join("hi.txt").exists());
        assert!(render(&mut app).contains("visible"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn steps_can_be_edited_reordered_and_removed() {
        let (root, repo) = scenario("pipeline-edit");
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        record(&mut app, "tidy", &["one", "two", "three"]);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('J'));
        assert_eq!(app.pipelines[0].steps, ["two", "one", "three"]);
        press(&mut app, KeyCode::Enter);
        typing(&mut app, "!");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.pipelines[0].steps, ["two", "one!", "three"]);
        assert!(app.modals.is_empty(), "editing asks for one line only");
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.pipelines[0].steps, ["two", "three"]);

        // names stay unique, and deleting asks first
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('n'));
        typing(&mut app, "tidy");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.pipelines.len(), 1);
        press(&mut app, KeyCode::Char('d'));
        assert!(matches!(app.modals.last(), Some(Modal::Confirm(_))));
        press(&mut app, KeyCode::Char('Y'));
        assert!(app.pipelines.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_running_pipeline_blocks_q_and_can_be_cancelled() {
        let (root, repo) = scenario("pipeline-cancel");
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        let sleep = if cfg!(windows) { "ping -n 30 127.0.0.1" } else { "sleep 30" };
        record(&mut app, "slow", &[sleep, "echo never>never.txt"]);
        press(&mut app, KeyCode::Enter);
        assert!(app.pipeline_running());
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.quit, "q must not kill a running build");
        // starting a second one is refused
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('x'));
        settle(&mut app);
        assert!(!app.pipeline_running());
        assert!(!repo.join("never.txt").exists());
        assert!(render(&mut app).contains("cancelled"));

        // quitting (ctrl-c) takes a running build down with it
        press(&mut app, KeyCode::Enter);
        assert!(app.pipeline_running());
        app.stop_pipeline();
        assert!(!app.pipeline_running());
        assert!(!repo.join("never.txt").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn journal_survives_restart_and_undo_reverts_a_commit() {
        let (root, repo) = scenario("undo");
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        let before = app.snap.status.oid.clone();
        press(&mut app, KeyCode::Char(':'));
        typing(&mut app, "commit -am \"wip: deps\"");
        press(&mut app, KeyCode::Enter);
        settle(&mut app);
        app.modals.clear();
        assert_ne!(app.snap.status.oid, before);
        drop(app);

        let mut app = app_for(&repo, &root);
        settle(&mut app);
        let entries = journal(&app, false);
        assert_eq!(entries[0].argv, ["commit", "-am", "wip: deps"]);
        assert_eq!(entries[0].via, ":");
        press(&mut app, KeyCode::Char('u'));
        let Some(Modal::Confirm(c)) = app.modals.last() else { panic!("no confirm:\n{}", render(&mut app)) };
        assert!(c.action.plan.display(&repo).starts_with("git reset --keep "));
        press(&mut app, KeyCode::Char('y'));
        settle(&mut app);
        let after = app.snap.status.oid.clone();
        assert_eq!(after, before, "{}", render(&mut app));
        // the commit's changes go with it (it stays in the reflog); nothing else was dirty
        assert!(app.snap.status.changes.is_empty());
        assert_eq!(journal(&app, false)[0].via, "u");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_dirty_worktree_can_be_force_removed_after_confirming() {
        let (root, repo) = scenario("force-rm");
        let wt = root.join("repo.wt").join("log-graph");
        std::fs::write(wt.join("scratch.txt"), "not committed anywhere
").unwrap();
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        press(&mut app, KeyCode::Char('4'));
        let at = app.snap.worktrees.iter().position(|w| w.path.ends_with("log-graph")).unwrap();
        for _ in 0..at {
            press(&mut app, KeyCode::Char('j'));
        }
        // plain remove refuses up front; force asks first, with what is lost
        let screen = render(&mut app);
        assert!(screen.contains("has uncommitted work"), "{screen}");
        press(&mut app, KeyCode::Char('X'));
        let Some(Modal::Confirm(c)) = app.modals.last() else { panic!("no confirm:
{}", render(&mut app)) };
        assert_eq!(c.action.plan.display(&repo), "git worktree remove --force ../repo.wt/log-graph");
        assert!(c.action.confirm.as_deref().unwrap_or("").contains("1 untracked"), "{:?}", c.action.confirm);
        // destructive: a plain y isn't enough
        press(&mut app, KeyCode::Char('y'));
        assert!(matches!(app.modals.last(), Some(Modal::Confirm(_))));
        press(&mut app, KeyCode::Char('Y'));
        settle(&mut app);
        assert!(!wt.exists(), "{}", render(&mut app));
        assert_eq!(app.snap.worktrees.len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_worktree_form_suggests_branches_that_are_not_checked_out() {
        let (root, repo) = scenario("wt-pick");
        let mut app = app_for(&repo, &root);
        settle(&mut app);
        press(&mut app, KeyCode::Char('4'));
        press(&mut app, KeyCode::Char('n'));
        let screen = render(&mut app);
        // main is free; feat/journal is checked out here, feat/log-graph in the other worktree
        // suggestion rows sit under the field, indented past its label
        assert!(screen.contains("│             main"), "{screen}");
        assert!(!screen.contains("│             feat/"), "{screen}");
        typing(&mut app, "mai");
        let ctrl_n = KeyEvent { code: KeyCode::Char('n'), modifiers: KeyModifiers::CONTROL, kind: KeyEventKind::Press, state: KeyEventState::NONE };
        app.on_key(ctrl_n);
        press(&mut app, KeyCode::Enter);
        // the field now holds the branch; the next Enter checks it out in a new worktree
        let screen = render(&mut app);
        assert!(screen.contains("existing"), "{screen}");
        press(&mut app, KeyCode::Enter);
        settle(&mut app);
        assert_eq!(app.tabs.len(), 2, "{}", render(&mut app));
        assert_eq!(app.snap.status.branch.as_deref(), Some("main"));
        let _ = std::fs::remove_dir_all(root);
    }

}
