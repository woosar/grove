use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::{Action, Ctx, Effect};
use crate::helpers::{centered, wrap_text};

/// "Are you sure?" for actions that ask first. Destructive ones are red and need
/// an uppercase `Y`.
pub struct ConfirmModal {
    pub action: Action,
    pub via: String,
    pub parent: Option<i64>,
}

impl ConfirmModal {
    pub fn handle_key(&mut self, key: &KeyEvent) -> Option<Effect> {
        let confirmed = match key.code {
            KeyCode::Char('Y') => true,
            KeyCode::Char('y') | KeyCode::Enter => !self.action.destructive,
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => return Some(Effect::Close),
            _ => false,
        };
        confirmed.then(|| {
            Effect::close_then(Effect::RunConfirmed { action: self.action.clone(), via: self.via.clone(), parent: self.parent })
        })
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let red = self.action.destructive;
        let edge = if red { t.err } else { t.accent };
        let w = area.width.saturating_sub(8).min(90);
        let inner_w = w.saturating_sub(4) as usize;
        let question = self.action.confirm.clone().unwrap_or_else(|| format!("Run {}?", self.action.label));
        let mut lines: Vec<Line> = wrap_text(&question, inner_w)
            .into_iter()
            .map(|l| Line::from(Span::styled(l, t.bold(t.fg))))
            .collect();
        lines.push(Line::raw(""));
        for l in wrap_text(&self.action.plan.display(&ctx.snap.workdir), inner_w) {
            lines.push(Line::from(Span::styled(l, t.s(edge))));
        }
        for l in wrap_text(&self.action.explain, inner_w) {
            lines.push(Line::from(Span::styled(l, t.dimmed())));
        }
        lines.push(Line::raw(""));
        let yes = if red { t.key_red("Y") } else { t.key("y") };
        lines.push(Line::from(vec![
            yes,
            Span::styled(if red { " yes, do it (shift-y)   " } else { " run   " }, t.text()),
            t.key_dim("esc"),
            Span::styled(" cancel", t.text()),
        ]));
        let rect = centered(area, w, lines.len() as u16 + 2);
        f.render_widget(Clear, rect);
        let title = if red { format!("{} not recoverable", t.g.warn) } else { "confirm".to_string() };
        let block = t.modal(Line::from(title).style(t.s(edge)), "", edge);
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        f.render_widget(Paragraph::new(lines), inner);
    }
}
