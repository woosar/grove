use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::{Action, Branch, Ctx, Effect, InternalOp, TextInput, Verdict};
use crate::helpers::{centered, pad, pad_left, relative_age, str_width, truncate, wrap_text};

const MAX_RESULTS: usize = 12;

/// `b` from anywhere: fuzzy branch switcher over local and remote branches.
pub struct QuickSwitcher {
    input: TextInput,
    candidates: Vec<Branch>,
    /// (candidate index, matched char indices), best first.
    results: Vec<(usize, Vec<u32>)>,
    /// Index into results; `results.len()` is the "create branch" row.
    sel: usize,
    matcher: Matcher,
}

impl QuickSwitcher {
    pub fn new(ctx: &Ctx) -> Self {
        let mut candidates: Vec<Branch> = ctx.snap.local_branches().cloned().collect();
        candidates.extend(ctx.snap.branches.iter().filter(|b| b.remote).cloned());
        let mut s = QuickSwitcher {
            input: TextInput::default(),
            candidates,
            results: Vec::new(),
            sel: 0,
            matcher: Matcher::new(Config::DEFAULT),
        };
        s.refilter();
        // start on the most recent branch that isn't the current one
        s.sel = s.results.iter().position(|(i, _)| !s.candidates[*i].is_head).unwrap_or(0);
        s
    }

    fn refilter(&mut self) {
        let query = self.input.value.trim().to_string();
        self.results.clear();
        if query.is_empty() {
            self.results = (0..self.candidates.len()).map(|i| (i, Vec::new())).collect();
        } else {
            let pattern = Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart);
            let mut buf = Vec::new();
            let mut scored: Vec<(u32, usize, Vec<u32>)> = Vec::new();
            for (i, b) in self.candidates.iter().enumerate() {
                let mut indices = Vec::new();
                if let Some(score) = pattern.indices(Utf32Str::new(&b.name, &mut buf), &mut self.matcher, &mut indices) {
                    indices.sort_unstable();
                    indices.dedup();
                    scored.push((score, i, indices));
                }
            }
            // best score first; equal scores keep recency order
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.results = scored.into_iter().map(|(_, i, idx)| (i, idx)).collect();
        }
        self.results.truncate(MAX_RESULTS);
        self.sel = self.sel.min(self.results.len());
    }

    fn on_create_row(&self) -> bool {
        self.sel >= self.results.len()
    }

    fn selected(&self) -> Option<&Branch> {
        self.results.get(self.sel).map(|(i, _)| &self.candidates[*i])
    }

    pub fn selected_branch(&self) -> Option<String> {
        self.selected().map(|b| b.name.clone())
    }

    fn primary(&self, ctx: &Ctx) -> Action {
        let f = ctx.actions();
        match self.selected() {
            Some(b) => f.switch(b),
            None => {
                let name = self.input.value.trim();
                let mut a = f.create_branch(name);
                if name.is_empty() {
                    a.verdict = Verdict::blocked("type a name to create a branch");
                } else if ctx.snap.local_branches().any(|b| b.name == name) {
                    a.verdict = Verdict::blocked(format!("{name} already exists"));
                }
                a
            }
        }
    }

    pub fn preview(&self, ctx: &Ctx) -> Option<Action> {
        Some(self.primary(ctx))
    }

    pub fn handle_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let rows = self.results.len() + 1;
        let g = &ctx.theme.g;
        let f = ctx.actions();
        match key.code {
            KeyCode::Esc => Some(Effect::Close),
            KeyCode::Down | KeyCode::Tab => {
                self.sel = (self.sel + 1) % rows;
                None
            }
            KeyCode::Char('j' | 'n') if ctrl => {
                self.sel = (self.sel + 1) % rows;
                None
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.sel = (self.sel + rows - 1) % rows;
                None
            }
            KeyCode::Char('k' | 'p') if ctrl => {
                self.sel = (self.sel + rows - 1) % rows;
                None
            }
            KeyCode::Enter => {
                let action = self.primary(ctx);
                if let (Verdict::Blocked { alternative: Some(_), .. }, Some(b)) = (&action.verdict, self.selected())
                    && let Some(wt) = ctx.snap.other_worktree_with(&b.name)
                {
                    return Some(Effect::close_then(Effect::run(f.jump(wt), g.enter)));
                }
                Some(Effect::close_then(Effect::run(action, g.enter)))
            }
            KeyCode::Char('w') if ctrl => match self.selected() {
                Some(b) => Some(Effect::close_then(Effect::run(f.branch_worktree(b), "ctrl-w"))),
                None => Some(Effect::close_then(Effect::Internal(InternalOp::NewWorktreeForm(Some(
                    self.input.value.trim().to_string(),
                ))))),
            },
            KeyCode::Char('t') if ctrl => match self.selected() {
                Some(b) if b.remote => Some(Effect::close_then(Effect::run(f.switch(b), "ctrl-t"))),
                _ => Some(Effect::Message("ctrl-t tracks a remote branch — pick one (origin/…)".into())),
            },
            KeyCode::Char('r') if ctrl => self.selected().map(|b| Effect::close_then(Effect::run(f.switch_and_rebase(b), "ctrl-r"))),
            _ => {
                if self.input.handle(key) {
                    self.sel = 0;
                    self.refilter();
                }
                None
            }
        }
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let w = area.width.saturating_sub(8).min(110);
        let inner_w = w.saturating_sub(4) as usize;
        let mut lines: Vec<Line> = Vec::new();

        let count = format!("{} match{}", self.results.len(), if self.results.len() == 1 { "" } else { "es" });
        let mut input_line = self.input.line(t.text(), true);
        input_line.spans.insert(0, Span::styled(format!("{} ", g.prompt), t.s(t.accent)));
        let used = input_line.width();
        input_line.spans.push(Span::raw(" ".repeat(inner_w.saturating_sub(used + str_width(&count)))));
        input_line.spans.push(Span::styled(count, t.dimmed()));
        lines.push(input_line.style(Style::new().bg(t.bg)));
        lines.push(Line::raw(""));

        let name_w = inner_w.saturating_sub(4 + 12 + 6 + 4);
        for (row, (i, matched)) in self.results.iter().enumerate() {
            let b = &self.candidates[*i];
            let selected = row == self.sel;
            let mut spans = vec![Span::styled(if selected { format!(" {} ", g.pointer) } else { "   ".to_string() }, t.s(t.accent))];
            let name_color = if b.is_head { t.accent } else if b.remote { t.dim } else { t.fg };
            let shown = truncate(&b.name, name_w);
            for (ci, ch) in shown.chars().enumerate() {
                let style = if matched.contains(&(ci as u32)) { Style::new().bg(t.accent).fg(t.bg) } else { t.s(name_color) };
                spans.push(Span::styled(ch.to_string(), style));
            }
            spans.push(Span::raw(" ".repeat(name_w.saturating_sub(str_width(&shown)) + 1)));
            let (ab, ab_style) = if b.remote {
                ("remote".to_string(), t.dimmed())
            } else if let Some((a, bh)) = b.vs_main {
                let color = if bh > 20 { t.err } else if bh > 0 { t.accent } else { t.ok };
                (format!("{}{a} {}{bh}", g.up, g.down), t.s(color))
            } else {
                (String::new(), t.dimmed())
            };
            spans.push(Span::styled(pad(&ab, 12), ab_style));
            spans.push(Span::styled(pad_left(&relative_age(b.time), 5), t.dimmed()));
            let wt = if b.worktree.is_some() && !b.is_head { " wt" } else { "" };
            spans.push(Span::styled(pad(wt, 4), t.s(t.tag)));
            let mut line = Line::from(spans);
            if selected {
                line = line.style(Style::new().bg(t.sel));
            }
            lines.push(line);
        }
        let query = self.input.value.trim();
        let mut create = Line::from(vec![
            Span::styled(" + ", t.s(t.ok)),
            Span::styled("create branch ", t.text()),
            Span::styled(if query.is_empty() { "<name>".to_string() } else { query.to_string() }, t.s(t.accent)),
            Span::styled(format!(" from {}", ctx.snap.head_label()), t.dimmed()),
        ]);
        if self.on_create_row() {
            create = create.style(Style::new().bg(t.sel));
        }
        lines.push(create);
        lines.push(Line::from(Span::styled("─".repeat(inner_w), t.s(t.border))));

        let action = self.primary(ctx);
        lines.push(Line::from(Span::styled(truncate(&action.plan.display(&ctx.snap.workdir), inner_w), t.s(t.accent))));
        let detail = match self.selected() {
            Some(b) => ctx.actions().switch_detail(b),
            None => action.explain.clone(),
        };
        for l in wrap_text(&detail, inner_w) {
            lines.push(Line::from(Span::styled(l, t.dimmed())));
        }
        let mut far_behind = false;
        match &action.verdict {
            Verdict::Blocked { reason, alternative } => {
                let alt = if alternative.is_some() { format!(" — {} jumps there instead", g.enter) } else { String::new() };
                lines.push(Line::from(Span::styled(format!("{} blocked: {reason}{alt}", g.warn), t.s(t.err))));
            }
            Verdict::Warn(w) => {
                far_behind = w.contains("behind");
                let mut spans = vec![Span::styled(format!("{} {w}", g.warn), t.s(t.accent))];
                if far_behind {
                    spans.push(Span::styled(format!(" {} ", g.dash), t.dimmed()));
                    spans.push(t.key_dim("ctrl-r"));
                    spans.push(Span::styled(" switch + rebase onto main in one go", t.text()));
                }
                lines.push(Line::from(spans));
            }
            Verdict::Ok => {}
        }
        lines.push(Line::raw(""));
        let mut keys = vec![
            t.key(g.enter),
            Span::styled(" switch   ", t.text()),
            t.key_dim("ctrl-w"),
            Span::styled(" open in new worktree   ", t.text()),
            t.key_dim("ctrl-t"),
            Span::styled(" track remote   ", t.text()),
        ];
        if !far_behind {
            keys.push(t.key_dim("ctrl-r"));
            keys.push(Span::styled(" switch + rebase   ", t.text()));
        }
        keys.push(t.key_dim("esc"));
        keys.push(Span::styled(" close", t.text()));
        lines.push(Line::from(keys));

        let h = lines.len() as u16 + 2;
        let mut rect = centered(area, w, h);
        rect.y = area.y + area.height.saturating_sub(h) / 4;
        f.render_widget(Clear, rect);
        let block = t.modal(Line::from("Switch branch").style(t.s(t.accent)), &format!("fuzzy {} local + remote", g.sep), t.accent);
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        f.render_widget(Paragraph::new(lines), inner);
    }
}
