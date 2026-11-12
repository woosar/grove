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
use crate::helpers::{pad, relative_to, short_oid, str_width, wrap_text};

const FIELDS: usize = 5;
/// Existing branches listed under the branch field at most.
const SUGGESTIONS: usize = 6;

/// An existing branch a worktree can be made for: not checked out anywhere.
struct Candidate {
    /// What goes into the branch field (the local name, also for a remote branch).
    name: String,
    remote: bool,
}

/// The "New worktree" form: branch, base, path, and two options. Shows the exact
/// commands under "will run".
pub struct WorktreeForm {
    branch: TextInput,
    base: TextInput,
    path: TextInput,
    path_edited: bool,
    copy: bool,
    open: bool,
    focus: usize,
    candidates: Vec<Candidate>,
    /// Candidates matching the branch field, best first.
    suggestions: Vec<usize>,
    /// Highlighted suggestion; `None` until ctrl-n / ctrl-p.
    pick: Option<usize>,
    matcher: Matcher,
}

impl WorktreeForm {
    pub fn new(branch: Option<String>, ctx: &Ctx) -> Self {
        let base = ctx.snap.current_branch().or(ctx.snap.main.as_deref()).unwrap_or("HEAD").to_string();
        let mut form = WorktreeForm {
            branch: TextInput::new(branch.unwrap_or_default()),
            base: TextInput::new(base),
            path: TextInput::default(),
            path_edited: false,
            copy: true,
            open: true,
            focus: 0,
            candidates: Self::candidates(ctx),
            suggestions: Vec::new(),
            pick: None,
            matcher: Matcher::new(Config::DEFAULT),
        };
        form.suggest_path(ctx);
        form.refilter();
        form
    }

    /// Local branches no worktree has checked out, then remote branches without a local
    /// one, each in the snapshot's (recency) order.
    fn candidates(ctx: &Ctx) -> Vec<Candidate> {
        let snap = ctx.snap;
        let checked_out = |name: &str| snap.worktrees.iter().any(|w| w.branch.as_deref() == Some(name));
        let mut out: Vec<Candidate> =
            snap.local_branches().filter(|b| !checked_out(&b.name)).map(|b| Candidate { name: b.name.clone(), remote: false }).collect();
        for b in snap.remote_only() {
            let name = b.local_name(&snap.remotes);
            if name != "HEAD" && !out.iter().any(|c| c.name == name) {
                out.push(Candidate { name: name.to_string(), remote: true });
            }
        }
        out
    }

    fn refilter(&mut self) {
        self.pick = None;
        let query = self.branch.value.trim();
        // a name that is already complete needs no suggestions
        if self.candidates.iter().any(|c| c.name == query) {
            self.suggestions.clear();
            return;
        }
        if query.is_empty() {
            self.suggestions = (0..self.candidates.len()).take(SUGGESTIONS).collect();
            return;
        }
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, usize)> = self
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(i, c)| pattern.score(Utf32Str::new(&c.name, &mut buf), &mut self.matcher).map(|s| (s, i)))
            .collect();
        // best score first; equal scores keep recency order
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.suggestions = scored.into_iter().map(|(_, i)| i).take(SUGGESTIONS).collect();
    }

    /// Moves the highlight through the suggestions; wraps around and starts at the top.
    fn move_pick(&mut self, down: bool) {
        let n = self.suggestions.len();
        if n == 0 {
            return;
        }
        self.pick = Some(match (self.pick, down) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(p), true) => (p + 1) % n,
            (Some(p), false) => (p + n - 1) % n,
        });
    }

    /// Hints for the bottom bar while the form is open.
    pub fn hints(&self) -> Vec<(&'static str, &'static str)> {
        let mut out = vec![("tab", "next field")];
        if self.focus == 0 && !self.suggestions.is_empty() {
            out.push(("ctrl-n/p", "pick branch"));
        }
        out.extend([("space", "toggle"), ("⏎", "create"), ("esc", "cancel")]);
        out
    }

    fn suggest_path(&mut self, ctx: &Ctx) {
        if self.path_edited {
            return;
        }
        let name = self.branch.value.trim();
        let suggested = ctx.snap.suggest_worktree_path(if name.is_empty() { "new" } else { name });
        self.path.set(relative_to(&suggested, &ctx.snap.workdir));
    }

    /// Untracked config files from the list that exist in this checkout.
    fn copy_files(&self, ctx: &Ctx) -> Vec<PathBuf> {
        ctx.copy_files.iter().filter(|f| ctx.snap.workdir.join(f).is_file()).cloned().collect()
    }

    /// `"new"`, `"existing"` or `"remote"`, by what the branch name refers to.
    fn branch_kind(&self, ctx: &Ctx) -> &'static str {
        let name = self.branch.value.trim();
        if ctx.snap.local_branches().any(|b| b.name == name) {
            "existing"
        } else if ctx.snap.branches.iter().any(|b| b.remote && b.local_name(&ctx.snap.remotes) == name) {
            "remote"
        } else {
            "new"
        }
    }

    pub fn action(&self, ctx: &Ctx) -> Result<Action, String> {
        let name = self.branch.value.trim();
        let path_text = self.path.value.trim();
        if name.is_empty() {
            return Err("type a branch name".into());
        }
        if path_text.is_empty() {
            return Err("type a path".into());
        }
        let path = ctx.snap.workdir.join(path_text);
        if path.exists() {
            return Err(format!("{path_text} already exists"));
        }
        let copy = if self.copy { self.copy_files(ctx) } else { Vec::new() };
        let f = ctx.actions();
        let plan = match self.branch_kind(ctx) {
            "existing" => {
                if let Some(wt) = ctx.snap.worktrees.iter().find(|w| w.branch.as_deref() == Some(name)) {
                    return Err(format!("{name} is already checked out in {}", ctx.snap.short_path(&wt.path)));
                }
                f.worktree_add_plan(name, None, &path, false, copy, self.open)
            }
            "remote" => {
                let remote = format!("{}/{name}", ctx.snap.remote());
                f.worktree_add_plan(name, Some(&remote), &path, true, copy, self.open)
            }
            _ => f.worktree_add_plan(name, Some(self.base.value.trim()), &path, true, copy, self.open),
        };
        Ok(Action::new(KeyBinding::enter(), "create", plan)
            .explain("creates a second checkout next to this one; same repo, same history")
            .help("git worktree add\n\nCreates a second checkout next to this one. Same repo, same history — you can build and run both branches side by side without stashing."))
    }

    pub fn handle_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        if self.focus == 0 {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let alt = key.modifiers.contains(KeyModifiers::ALT);
            match key.code {
                KeyCode::Char('n') if ctrl => return self.picked(true),
                KeyCode::Char('p') if ctrl => return self.picked(false),
                KeyCode::Down if alt => return self.picked(true),
                KeyCode::Up if alt => return self.picked(false),
                KeyCode::Enter if self.pick.is_some() => {
                    if let Some(c) = self.pick.and_then(|p| self.suggestions.get(p)).map(|&i| &self.candidates[i]) {
                        self.branch.set(c.name.clone());
                        self.suggest_path(ctx);
                    }
                    self.refilter();
                    return Some(Effect::Nothing);
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % FIELDS,
            KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + FIELDS - 1) % FIELDS,
            KeyCode::Char(' ') if self.focus == 3 => self.copy = !self.copy,
            KeyCode::Char(' ') if self.focus == 4 => self.open = !self.open,
            KeyCode::Enter => {
                return Some(match self.action(ctx) {
                    Ok(a) => Effect::run(a, ctx.theme.g.enter),
                    Err(e) => Effect::Message(e),
                });
            }
            _ => match self.focus {
                0 if self.branch.handle(key) => {
                    self.suggest_path(ctx);
                    self.refilter();
                }
                1 => {
                    self.base.handle(key);
                }
                2 if self.path.handle(key) => self.path_edited = true,
                _ => {}
            },
        }
        Some(Effect::Nothing)
    }

    fn picked(&mut self, down: bool) -> Option<Effect> {
        self.move_pick(down);
        Some(Effect::Nothing)
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let block = t.pane("New worktree", "tab next field", true);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let field_w = w.saturating_sub(10 + 10);
        let input = |label: &str, input: &TextInput, idx: usize, right: String| {
            let focused = self.focus == idx;
            let mut spans = vec![Span::styled(pad(label, 10), t.dimmed())];
            let mut line = input.line(t.text(), focused);
            let used = line.width();
            line.spans.push(Span::raw(" ".repeat(field_w.saturating_sub(used))));
            let bg = if focused { Style::new().bg(t.sel) } else { Style::new().bg(t.bg_raised) };
            spans.extend(line.spans.into_iter().map(|s| s.patch_style(bg)));
            spans.push(Span::styled(format!(" {right}"), if focused { t.s(t.accent) } else { t.dimmed() }));
            Line::from(spans)
        };
        let kind = self.branch_kind(ctx);
        let base_oid = ctx
            .snap
            .branches
            .iter()
            .find(|b| b.name == self.base.value.trim())
            .map(|b| short_oid(&b.oid).to_string())
            .unwrap_or_default();
        let checkbox = |on: bool, idx: usize, text: String, note: String| {
            let mark = if on { g.box_on } else { g.box_off };
            let style = if self.focus == idx { t.s(t.accent) } else { t.text() };
            Line::from(vec![
                Span::raw(" ".repeat(10)),
                Span::styled(format!("{mark} "), t.s(t.accent)),
                Span::styled(text, style),
                Span::styled(format!("  {note}"), t.dimmed()),
            ])
        };
        let files: Vec<String> = self.copy_files(ctx).iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        let mut lines = vec![input("branch", &self.branch, 0, kind.to_string())];
        if self.focus == 0 && !self.suggestions.is_empty() {
            for (row, &i) in self.suggestions.iter().enumerate() {
                let c = &self.candidates[i];
                let picked = self.pick == Some(row);
                lines.push(Line::from(vec![
                    Span::raw(" ".repeat(10)),
                    Span::styled(format!("{} ", if picked { g.pointer } else { " " }), t.s(t.accent)),
                    Span::styled(c.name.clone(), if picked { t.bold(t.branch) } else { t.s(t.branch) }),
                    Span::styled(if c.remote { "  remote" } else { "" }, t.dimmed()),
                ]));
            }
        }
        lines.extend([
            Line::raw(""),
            input("from", &self.base, 1, if kind == "new" { base_oid } else { "—".into() }),
            Line::raw(""),
            input("path", &self.path, 2, if self.path_edited { "edited".into() } else { "auto".into() }),
            Line::raw(""),
            checkbox(self.copy, 3, "copy untracked config".into(), if files.is_empty() { "none found".into() } else { files.join(", ") }),
            checkbox(self.open, 4, "open in a new grove tab when done".into(), String::new()),
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
                lines.push(Line::raw(""));
                for l in wrap_text(
                    "Creates a second checkout next to this one. Same repo, same history — you can build and run both branches side by side without stashing.",
                    w,
                ) {
                    lines.push(Line::from(Span::styled(l, t.dimmed())));
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
