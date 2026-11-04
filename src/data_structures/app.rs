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
    InternalOp, Job, JobResult, Journal, JournalEntry, JournalScreen, KeyBinding, LogScreen, Modal, Pager, Plan, PreflightInfo,
    PromptKind, PromptModal, QuickSwitcher, RunOutcome, RunRequest, ScreenId, Snapshot, Theme, Verdict, Worker, WorktreesScreen,
};
use crate::helpers::{
    copy_to_clipboard, home_tilde, norm_path, relative_to, same_path, short_oid, str_width, truecolor_supported, truncate,
};

/// Poll `git status` this often while idle.
const PROBE_EVERY: Duration = Duration::from_secs(5);
/// Full reload this often, to catch ref changes `status` doesn't show.
const REFRESH_EVERY: Duration = Duration::from_secs(30);
const MESSAGE_FOR: Duration = Duration::from_secs(6);

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
    tick: usize,
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
            tick: 0,
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
        result
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            terminal.draw(|f| self.draw(f))?;
            if event::poll(Duration::from_millis(80))? {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
                    Event::FocusGained => self.refresh(),
                    _ => {}
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
        self.tick = self.tick.wrapping_add(1);
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
        }
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
            '1'..='5' => {
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
            '?' => self.open_help(),
            'u' => self.undo(None),
            ']' => self.cycle_tab(1),
            '[' => self.cycle_tab(-1),
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
            ("1–5", "screens"),
            ("tab / shift-tab", "cycle panes"),
            ("j/k, arrows", "move"),
            ("space", "actions for the selected item"),
            ("b", "quick switch (fuzzy)"),
            ("f / p / P", "fetch / pull / push"),
            (":", "raw git prompt (journaled)"),
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
        }
    }

    fn screen_hints(&self) -> Vec<(&'static str, &'static str)> {
        match self.screen {
            ScreenId::Dashboard => self.dashboard.hints(),
            ScreenId::Log => self.log.hints(),
            ScreenId::Branches => self.branches.hints(),
            ScreenId::Worktrees => self.worktrees.hints(),
            ScreenId::Journal => self.journal_view.hints(),
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
        let left_w: usize = left.iter().map(|s| s.width()).sum();
        let full = (area.width as usize) >= left_w + 64;
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
            let spin = g.spinner[self.tick / 2 % g.spinner.len()];
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
            let busy = app.queued > 0 || app.applied_seq < app.refresh_seq;
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
        for key in ['1', '2', '3', '4', '5'] {
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
}
