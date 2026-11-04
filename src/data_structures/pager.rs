use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::{Ctx, Effect, InternalOp};

/// Full-screen scrollable output of a read-only command (`git show`, `git log …`).
pub struct Pager {
    pub title: String,
    pub text: String,
    scroll: usize,
    height: usize,
}

impl Pager {
    pub fn new(title: impl Into<String>, text: impl Into<String>) -> Self {
        Pager { title: title.into(), text: text.into(), scroll: 0, height: 20 }
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> Option<Effect> {
        let lines = self.text.lines().count();
        let max = lines.saturating_sub(self.height);
        let page = self.height.saturating_sub(2).max(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => return Some(Effect::Close),
            KeyCode::Char('y') => return Some(Effect::Internal(InternalOp::Copy(self.text.clone()))),
            KeyCode::Char('j') | KeyCode::Down => self.scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char(' ') | KeyCode::PageDown => self.scroll += page,
            KeyCode::PageUp | KeyCode::Char('b') => self.scroll = self.scroll.saturating_sub(page),
            KeyCode::Char('g') | KeyCode::Home => self.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => self.scroll = max,
            _ => {}
        }
        self.scroll = self.scroll.min(max);
        None
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        f.render_widget(Clear, area);
        let total = self.text.lines().count();
        let hint = format!("{}/{} {} q close {} y copy", (self.scroll + 1).min(total), total, t.g.sep, t.g.sep);
        let block = t.pane(Line::from(self.title.clone()), &hint, true);
        let inner = block.inner(area);
        self.height = inner.height as usize;
        f.render_widget(block, area);
        let lines: Vec<Line> = self
            .text
            .lines()
            .skip(self.scroll)
            .take(inner.height as usize)
            .map(|l| {
                let style = if l.starts_with("+++") || l.starts_with("---") {
                    t.bold(t.fg)
                } else if l.starts_with('+') {
                    t.s(t.ok)
                } else if l.starts_with('-') {
                    t.s(t.err)
                } else if l.starts_with("@@") {
                    t.s(t.branch)
                } else if l.starts_with("commit ") || l.starts_with("diff --git") {
                    t.s(t.accent).add_modifier(Modifier::BOLD)
                } else {
                    t.text()
                };
                Line::from(Span::styled(l.replace('\t', "    "), style))
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
}
