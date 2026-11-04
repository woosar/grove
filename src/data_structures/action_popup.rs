use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::{Action, Ctx, Effect, HelpModal, Modal, Verdict};
use crate::helpers::{anchored, nav, pad, str_width, truncate};

/// The `space` tooltip: every action for the selected item with key, command and verdict.
pub struct ActionPopup {
    pub title: String,
    pub info: String,
    pub actions: Vec<Action>,
    pub sel: usize,
    pub anchor: Option<Rect>,
}

impl ActionPopup {
    pub fn new(title: impl Into<String>, info: impl Into<String>, actions: Vec<Action>, anchor: Option<Rect>) -> Self {
        // start on the first action that can actually run
        let sel = actions.iter().position(|a| !a.verdict.is_blocked()).unwrap_or(0);
        ActionPopup { title: title.into(), info: info.into(), actions, sel, anchor }
    }

    pub fn preview(&self) -> Option<Action> {
        self.actions.get(self.sel).cloned()
    }

    fn run(&self, idx: usize, ctx: &Ctx) -> Effect {
        let action = &self.actions[idx];
        if let Verdict::Blocked { alternative: Some(alt), .. } = &action.verdict
            && let Some(other) = self.actions.iter().find(|a| a.key == *alt)
        {
            return Effect::close_then(Effect::run(other.clone(), other.key.label(&ctx.theme.g)));
        }
        Effect::close_then(Effect::run(action.clone(), action.key.label(&ctx.theme.g)))
    }

    pub fn handle_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        if let Some(sel) = nav(key, self.sel, self.actions.len()) {
            self.sel = sel;
            return None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(' ') => Some(Effect::Close),
            KeyCode::Char('?') => self.actions.get(self.sel).map(|a| Effect::open(Modal::Help(HelpModal::for_action(a)))),
            KeyCode::Enter if !self.actions.is_empty() => {
                // an action bound to ⏎ itself (jump, open) wins over the highlighted row
                match self.actions.iter().position(|a| a.key.matches(key)) {
                    Some(i) if i != self.sel && self.actions[self.sel].verdict.is_blocked() => Some(self.run(i, ctx)),
                    _ => Some(self.run(self.sel, ctx)),
                }
            }
            _ => self.actions.iter().position(|a| a.key.matches(key)).map(|i| self.run(i, ctx)),
        }
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let cwd = &ctx.snap.workdir;
        let label_w = self.actions.iter().map(|a| str_width(a.label)).max().unwrap_or(6).max(7) + 1;
        let max_w = area.width.saturating_sub(4).min(110) as usize;
        let inner_w = max_w.saturating_sub(4);
        let mut lines: Vec<Line> = vec![
            Line::from(vec![
                Span::styled(self.title.clone(), t.bold(t.branch)),
                Span::styled(if self.info.is_empty() { String::new() } else { format!(" {} {}", g.sep, self.info) }, t.dimmed()),
            ]),
            Line::from(Span::styled("─".repeat(inner_w), t.s(t.border))),
        ];
        let mut sel_line = 0;
        for (i, a) in self.actions.iter().enumerate() {
            let blocked = a.verdict.is_blocked();
            let selected = i == self.sel;
            if selected {
                sel_line = lines.len();
            }
            let chip = if blocked { t.key_dim(a.key.label(g)) } else if a.destructive { t.key_red(a.key.label(g)) } else { t.key(a.key.label(g)) };
            let label_style = if blocked { t.dimmed() } else { t.text() };
            let mut spans = vec![chip, Span::raw(" "), Span::styled(pad(a.label, label_w), label_style)];
            let used = 4 + label_w;
            let room = inner_w.saturating_sub(used);
            if a.plan.is_internal() {
                spans.push(Span::styled(truncate(&a.explain, room), t.text()));
            } else {
                let cmd = a.plan.display(cwd);
                let cmd_style = if blocked { t.dimmed() } else if a.destructive { t.s(t.err) } else { t.s(t.accent) };
                let cmd_t = truncate(&cmd, room);
                let rest = room.saturating_sub(str_width(&cmd_t) + 1);
                spans.push(Span::styled(cmd_t, cmd_style));
                if !a.note.is_empty() && rest > 4 {
                    spans.push(Span::styled(format!(" {}", truncate(&a.note, rest)), t.dimmed()));
                }
            }
            let mut line = Line::from(spans);
            if selected {
                line = line.style(Style::new().bg(t.sel));
            }
            lines.push(line);
            match &a.verdict {
                Verdict::Blocked { reason, alternative } => {
                    let alt = alternative
                        .and_then(|k| self.actions.iter().find(|o| o.key == k))
                        .map(|o| format!(" — {} {} instead", o.key.label(g), o.label))
                        .unwrap_or_default();
                    lines.push(Line::from(Span::styled(
                        truncate(&format!("    {} blocked: {reason}{alt}", g.warn), inner_w),
                        t.s(t.err),
                    )));
                }
                Verdict::Warn(w) => {
                    lines.push(Line::from(Span::styled(truncate(&format!("    {} {w}", g.warn), inner_w), t.s(t.accent))));
                }
                Verdict::Ok => {}
            }
        }
        lines.push(Line::from(vec![
            t.key("?"),
            Span::raw(" "),
            Span::styled(pad("explain", label_w), t.text()),
            Span::styled("long-form help for the highlighted action", t.dimmed()),
        ]));
        let content_w = lines.iter().map(|l| l.width()).max().unwrap_or(40).min(inner_w);
        let w = (content_w + 4) as u16;
        let h = (lines.len() + 2) as u16;
        let rect = match self.anchor {
            Some(anchor) => anchored(area, anchor, w, h),
            None => crate::helpers::centered(area, w, h),
        };
        f.render_widget(Clear, rect);
        let block = t.modal("", "", t.accent);
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let scroll = sel_line.saturating_sub(inner.height.saturating_sub(2) as usize) as u16;
        f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
    }
}
