use std::path::PathBuf;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Action, Ctx, Effect, KeyBinding, TextInput};
use crate::helpers::{keep_visible, pad, relative_to, short_oid, str_width, wrap_text};

/// Branch rows shown at once in "existing branch" mode; the list scrolls past that.
const LIST_ROWS: usize = 8;
const LABEL_W: usize = 10;

/// An existing branch a worktree can be made for: not checked out anywhere.
struct Candidate {
    /// The local branch name (also for a remote branch: what the new local branch is called).
    name: String,
    /// `Some("origin/x")` for a remote branch with no local branch yet.
    remote: Option<String>,
    oid: String,
}

/// Check out a branch that already exists, or create a new one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Existing,
    New,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Mode,
    Branch,
    Base,
    Path,
    Copy,
    Open,
}

/// The "New worktree" form. In "existing branch" mode the branch field filters a list
/// of branches not checked out anywhere and ↑/↓ picks one; in "new branch" mode it is
/// the new branch's name, created from "from". ctrl-t switches. Shows the exact
/// commands under "will run".
pub struct WorktreeForm {
    mode: Mode,
    /// Filter (existing mode) or new branch name (new mode); kept across mode switches.
    branch: TextInput,
    base: TextInput,
    path: TextInput,
    path_edited: bool,
    copy: bool,
    open: bool,
    focus: Field,
    candidates: Vec<Candidate>,
    /// Candidates matching the branch field, best first.
    matches: Vec<usize>,
    /// Selected row in `matches`.
    sel: usize,
    /// First visible row of the list.
    offset: usize,
    matcher: Matcher,
}

impl WorktreeForm {
    /// `branch` prefills the branch field: an existing free branch opens in existing
    /// mode with it selected, any other name in new mode.
    pub fn new(branch: Option<String>, ctx: &Ctx) -> Self {
        let base = ctx.snap.current_branch().or(ctx.snap.main.as_deref()).unwrap_or("HEAD").to_string();
        let candidates = Self::candidates(ctx);
        let prefill = branch.map(|b| b.trim().to_string()).unwrap_or_default();
        let mode = if prefill.is_empty() {
            if candidates.is_empty() { Mode::New } else { Mode::Existing }
        } else if candidates.iter().any(|c| c.name == prefill) {
            Mode::Existing
        } else {
            Mode::New
        };
        let mut form = WorktreeForm {
            mode,
            branch: TextInput::new(prefill),
            base: TextInput::new(base),
            path: TextInput::default(),
            path_edited: false,
            copy: true,
            open: true,
            focus: Field::Branch,
            candidates,
            matches: Vec::new(),
            sel: 0,
            offset: 0,
            matcher: Matcher::new(Config::DEFAULT),
        };
        form.refilter();
        form.suggest_path(ctx);
        form
    }

    /// Local branches no worktree has checked out, then remote branches without a local
    /// one, each in the snapshot's (recency) order.
    fn candidates(ctx: &Ctx) -> Vec<Candidate> {
        let snap = ctx.snap;
        let checked_out = |name: &str| snap.worktrees.iter().any(|w| w.branch.as_deref() == Some(name));
        let mut out: Vec<Candidate> = snap
            .local_branches()
            .filter(|b| !checked_out(&b.name))
            .map(|b| Candidate { name: b.name.clone(), remote: None, oid: b.oid.clone() })
            .collect();
        for b in snap.remote_only() {
            let name = b.local_name(&snap.remotes);
            if name != "HEAD" && !checked_out(name) && !out.iter().any(|c| c.name == name) {
                out.push(Candidate { name: name.to_string(), remote: Some(b.name.clone()), oid: b.oid.clone() });
            }
        }
        out
    }

    fn fields(&self) -> &'static [Field] {
        match self.mode {
            Mode::Existing => &[Field::Mode, Field::Branch, Field::Path, Field::Copy, Field::Open],
            Mode::New => &[Field::Mode, Field::Branch, Field::Base, Field::Path, Field::Copy, Field::Open],
        }
    }

    fn move_focus(&mut self, forward: bool) {
        let fields = self.fields();
        let n = fields.len();
        let at = fields.iter().position(|&f| f == self.focus).unwrap_or(0);
        self.focus = fields[if forward { (at + 1) % n } else { (at + n - 1) % n }];
    }

    fn toggle_mode(&mut self, ctx: &Ctx) {
        self.mode = match self.mode {
            Mode::Existing => Mode::New,
            Mode::New => Mode::Existing,
        };
        if !self.fields().contains(&self.focus) {
            self.focus = Field::Branch;
        }
        self.refilter();
        self.suggest_path(ctx);
    }

    /// Re-ranks the candidates against the branch field and selects the best match
    /// (an exact name match always comes first).
    fn refilter(&mut self) {
        self.sel = 0;
        self.offset = 0;
        let query = self.branch.value.trim();
        if query.is_empty() {
            self.matches = (0..self.candidates.len()).collect();
            return;
        }
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(bool, u32, usize)> = self
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                pattern.score(Utf32Str::new(&c.name, &mut buf), &mut self.matcher).map(|s| (c.name == query, s, i))
            })
            .collect();
        // exact first, then best score; equal scores keep recency order
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        self.matches = scored.into_iter().map(|(_, _, i)| i).collect();
    }

    /// Moves the list selection; wraps around.
    fn move_sel(&mut self, down: bool, ctx: &Ctx) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        self.sel = if down { (self.sel + 1) % n } else { (self.sel + n - 1) % n };
        keep_visible(&mut self.offset, self.sel, LIST_ROWS);
        self.suggest_path(ctx);
    }

    fn selected(&self) -> Option<&Candidate> {
        self.matches.get(self.sel).map(|&i| &self.candidates[i])
    }

    /// The branch the worktree will have checked out, by mode.
    fn target_name(&self) -> &str {
        match self.mode {
            Mode::Existing => self.selected().map(|c| c.name.as_str()).unwrap_or(""),
            Mode::New => self.branch.value.trim(),
        }
    }

    /// Hints for the bottom bar while the form is open.
    pub fn hints(&self) -> Vec<(&'static str, &'static str)> {
        let mut out = vec![("tab", "next field")];
        match self.mode {
            Mode::Existing => {
                out.push(("↑/↓", "pick branch"));
                out.push(("ctrl-t", "new branch instead"));
            }
            Mode::New => out.push(("ctrl-t", "existing branch instead")),
        }
        out.extend([("space", "toggle"), ("⏎", "create"), ("esc", "cancel")]);
        out
    }

    fn suggest_path(&mut self, ctx: &Ctx) {
        if self.path_edited {
            return;
        }
        let name = self.target_name();
        let suggested = ctx.snap.suggest_worktree_path(if name.is_empty() { "new" } else { name });
        self.path.set(relative_to(&suggested, &ctx.snap.workdir));
    }

    /// Untracked config files from the list that exist in this checkout.
    fn copy_files(&self, ctx: &Ctx) -> Vec<PathBuf> {
        ctx.copy_files.iter().filter(|f| ctx.snap.workdir.join(f).is_file()).cloned().collect()
    }

    pub fn action(&self, ctx: &Ctx) -> Result<Action, String> {
        let path_text = self.path.value.trim();
        let f = ctx.actions();
        let (name, base, new_branch) = match self.mode {
            Mode::Existing => {
                let query = self.branch.value.trim();
                let Some(c) = self.selected() else {
                    return Err(if self.candidates.is_empty() {
                        "every branch is checked out somewhere — ctrl-t to create a new one".into()
                    } else {
                        format!("no free branch matches \"{query}\" — ctrl-t to create it as a new branch")
                    });
                };
                (c.name.as_str(), c.remote.as_deref(), c.remote.is_some())
            }
            Mode::New => {
                let name = self.branch.value.trim();
                if name.is_empty() {
                    return Err("type a name for the new branch".into());
                }
                if let Some(wt) = ctx.snap.worktrees.iter().find(|w| w.branch.as_deref() == Some(name)) {
                    return Err(format!("{name} exists and is checked out in {}", ctx.snap.short_path(&wt.path)));
                }
                if ctx.snap.local_branches().any(|b| b.name == name) || self.candidates.iter().any(|c| c.name == name) {
                    return Err(format!("{name} already exists — ctrl-t to pick it as an existing branch"));
                }
                let base = self.base.value.trim();
                if base.is_empty() {
                    return Err("type what to branch from".into());
                }
                (name, Some(base), true)
            }
        };
        if path_text.is_empty() {
            return Err("type a path".into());
        }
        let path = ctx.snap.workdir.join(path_text);
        if path.exists() {
            return Err(format!("{path_text} already exists"));
        }
        let copy = if self.copy { self.copy_files(ctx) } else { Vec::new() };
        let plan = f.worktree_add_plan(name, base, &path, new_branch, copy, self.open);
        Ok(Action::new(KeyBinding::enter(), "create", plan)
            .explain("creates a second checkout next to this one; same repo, same history")
            .help("git worktree add\n\nCreates a second checkout next to this one. Same repo, same history — you can build and run both branches side by side without stashing."))
    }

    pub fn handle_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let picking = self.mode == Mode::Existing && self.focus == Field::Branch;
        match key.code {
            KeyCode::Char('t') if ctrl => self.toggle_mode(ctx),
            KeyCode::Char('n') if ctrl && self.mode == Mode::Existing => self.move_sel(true, ctx),
            KeyCode::Char('p') if ctrl && self.mode == Mode::Existing => self.move_sel(false, ctx),
            KeyCode::Down if picking => self.move_sel(true, ctx),
            KeyCode::Up if picking => self.move_sel(false, ctx),
            KeyCode::Tab | KeyCode::Down => self.move_focus(true),
            KeyCode::BackTab | KeyCode::Up => self.move_focus(false),
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right if self.focus == Field::Mode => self.toggle_mode(ctx),
            KeyCode::Char(' ') if self.focus == Field::Copy => self.copy = !self.copy,
            KeyCode::Char(' ') if self.focus == Field::Open => self.open = !self.open,
            KeyCode::Enter => {
                return Some(match self.action(ctx) {
                    Ok(a) => Effect::run(a, ctx.theme.g.enter),
                    Err(e) => Effect::Message(e),
                });
            }
            _ => match self.focus {
                Field::Branch if self.branch.handle(key) => {
                    self.refilter();
                    self.suggest_path(ctx);
                }
                Field::Base => {
                    self.base.handle(key);
                }
                Field::Path if self.path.handle(key) => self.path_edited = true,
                _ => {}
            },
        }
        Some(Effect::Nothing)
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let block = t.pane("New worktree", "ctrl-t existing / new", true);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let field_w = w.saturating_sub(LABEL_W + 12);
        let input = |label: &str, input: &TextInput, field: Field, right: String| {
            let focused = self.focus == field;
            let mut spans = vec![Span::styled(pad(label, LABEL_W), t.dimmed())];
            let mut line = input.line(t.text(), focused);
            let used = line.width();
            line.spans.push(Span::raw(" ".repeat(field_w.saturating_sub(used))));
            let bg = if focused { Style::new().bg(t.sel) } else { Style::new().bg(t.bg_raised) };
            spans.extend(line.spans.into_iter().map(|s| s.patch_style(bg)));
            spans.push(Span::styled(format!(" {right}"), if focused { t.s(t.accent) } else { t.dimmed() }));
            Line::from(spans)
        };
        let checkbox = |on: bool, field: Field, text: String, note: String| {
            let mark = if on { g.box_on } else { g.box_off };
            let style = if self.focus == field { t.s(t.accent) } else { t.text() };
            Line::from(vec![
                Span::raw(" ".repeat(LABEL_W)),
                Span::styled(format!("{mark} "), t.s(t.accent)),
                Span::styled(text, style),
                Span::styled(format!("  {note}"), t.dimmed()),
            ])
        };
        let radio = |m: Mode, text: &str| {
            let on = self.mode == m;
            let style = match (on, self.focus == Field::Mode) {
                (true, true) => t.bold(t.accent),
                (true, false) => t.bold(t.fg),
                _ => t.dimmed(),
            };
            vec![Span::styled(format!("{} ", if on { g.radio_on } else { g.radio_off }), t.s(t.accent)), Span::styled(text.to_string(), style)]
        };
        let mut mode_row = vec![Span::styled(pad("checkout", LABEL_W), t.dimmed())];
        mode_row.extend(radio(Mode::Existing, "existing branch"));
        mode_row.push(Span::raw("   "));
        mode_row.extend(radio(Mode::New, "new branch"));
        mode_row.push(Span::styled("   ctrl-t", if self.focus == Field::Mode { t.s(t.accent) } else { t.dimmed() }));
        let mut lines = vec![Line::from(mode_row), Line::raw("")];

        match self.mode {
            Mode::Existing => {
                let count = format!("{}/{}", self.matches.len(), self.candidates.len());
                lines.push(input("branch", &self.branch, Field::Branch, count));
                if self.candidates.is_empty() {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(LABEL_W)),
                        Span::styled("every branch is checked out in some worktree", t.dimmed()),
                    ]));
                } else if self.matches.is_empty() {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(LABEL_W)),
                        Span::styled("no free branch matches — ctrl-t to create it", t.dimmed()),
                    ]));
                }
                let end = (self.offset + LIST_ROWS).min(self.matches.len());
                for row in self.offset..end {
                    let c = &self.candidates[self.matches[row]];
                    let picked = row == self.sel;
                    let mut spans = vec![
                        Span::raw(" ".repeat(LABEL_W)),
                        Span::styled(format!("{} ", if picked { g.pointer } else { " " }), t.s(t.accent)),
                        Span::styled(c.name.clone(), if picked { t.bold(t.branch) } else { t.s(t.branch) }),
                        Span::styled(format!("  {}", short_oid(&c.oid)), t.dimmed()),
                    ];
                    if let Some(r) = &c.remote {
                        spans.push(Span::styled(format!("  from {r}"), t.dimmed()));
                    }
                    lines.push(Line::from(spans));
                }
                if self.matches.len() > LIST_ROWS {
                    let more = format!("{}–{} of {}", self.offset + 1, end, self.matches.len());
                    lines.push(Line::from(vec![Span::raw(" ".repeat(LABEL_W + 2)), Span::styled(more, t.dimmed())]));
                }
            }
            Mode::New => {
                lines.push(input("branch", &self.branch, Field::Branch, "new".into()));
                lines.push(Line::raw(""));
                let base_oid = ctx
                    .snap
                    .branches
                    .iter()
                    .find(|b| b.name == self.base.value.trim())
                    .map(|b| short_oid(&b.oid).to_string())
                    .unwrap_or_default();
                lines.push(input("from", &self.base, Field::Base, base_oid));
            }
        }

        let files: Vec<String> = self.copy_files(ctx).iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        lines.extend([
            Line::raw(""),
            input("path", &self.path, Field::Path, if self.path_edited { "edited".into() } else { "auto".into() }),
            Line::raw(""),
            checkbox(self.copy, Field::Copy, "copy untracked config".into(), if files.is_empty() { "none found".into() } else { files.join(", ") }),
            checkbox(self.open, Field::Open, "open in a new grove tab when done".into(), String::new()),
            Line::from(Span::styled("─".repeat(w), t.s(t.border))),
            Line::from(Span::styled("will run", t.dimmed())),
        ]);
        match self.action(ctx) {
            Ok(a) => {
                for step in &a.plan.steps {
                    for l in wrap_text(&step.display(&ctx.snap.workdir), w) {
                        lines.push(Line::from(Span::styled(l, t.s(t.accent))));
                    }
                }
            }
            Err(e) => lines.push(Line::from(Span::styled(e, t.s(t.err)))),
        }
        let footer_y = inner.height.saturating_sub(1) as usize;
        while lines.len() < footer_y {
            lines.push(Line::raw(""));
        }
        let keys = Line::from(vec![t.key(g.enter), Span::styled(" create   ", t.text()), t.key_dim("esc"), Span::styled(" cancel", t.text())]);
        if str_width(&keys.to_string()) <= w {
            lines.push(keys);
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}
