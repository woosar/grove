use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::{Action, Ctx, Effect};
use crate::helpers::{centered, wrap_text};

/// Long-form help: an action's `long_help`, an error's "why", or a screen's keymap.
pub struct HelpModal {
    pub title: String,
    pub body: String,
    scroll: u16,
}

impl HelpModal {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        HelpModal { title: title.into(), body: body.into(), scroll: 0 }
    }

    pub fn for_action(a: &Action) -> Self {
        let body = if a.long_help.is_empty() { a.explain.clone() } else { a.long_help.to_string() };
        HelpModal::new(format!("? {}", a.label), body)
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> Option<Effect> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Enter => Some(Effect::Close),
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll = self.scroll.saturating_add(1);
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll = self.scroll.saturating_sub(1);
                None
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.scroll = self.scroll.saturating_add(10);
                None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                None
            }
            _ => None,
        }
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let w = area.width.saturating_sub(8).min(96);
        let wrapped = wrap_text(&self.body, w.saturating_sub(4) as usize);
        let mut lines: Vec<Line> = Vec::new();
        for (i, l) in wrapped.into_iter().enumerate() {
            // first line of an action's help is the command synopsis
            let style = if i == 0 && (l.starts_with("git ") || l.contains("(grove only)")) { t.s(t.accent) } else { t.text() };
            lines.push(Line::from(Span::styled(l, style)));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![t.key_dim("esc"), Span::styled(" close   ", t.text()), t.key_dim("j/k"), Span::styled(" scroll", t.text())]));
        let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
        let rect = centered(area, w, h);
        f.render_widget(Clear, rect);
        let block = t.modal(Line::from(self.title.clone()).style(t.s(t.accent)), "", t.accent);
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
        f.render_widget(Paragraph::new(lines).scroll((self.scroll.min(max_scroll), 0)), inner);
    }
}
