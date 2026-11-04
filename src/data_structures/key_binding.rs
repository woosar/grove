use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Glyphs;

/// A key an action is bound to. Shift is implied by the character (`F`, `R`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub ctrl: bool,
}

impl KeyBinding {
    pub const fn ch(c: char) -> Self {
        KeyBinding { code: KeyCode::Char(c), ctrl: false }
    }

    pub const fn ctrl(c: char) -> Self {
        KeyBinding { code: KeyCode::Char(c), ctrl: true }
    }

    pub const fn enter() -> Self {
        KeyBinding { code: KeyCode::Enter, ctrl: false }
    }

    pub fn matches(&self, key: &KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl != self.ctrl {
            return false;
        }
        match (self.code, key.code) {
            (KeyCode::Char(a), KeyCode::Char(b)) => {
                if self.ctrl { a.eq_ignore_ascii_case(&b) } else { a == b }
            }
            (a, b) => a == b,
        }
    }

    pub fn label(&self, g: &Glyphs) -> String {
        let base = match self.code {
            KeyCode::Enter => g.enter.to_string(),
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Esc => "esc".to_string(),
            KeyCode::Tab => "tab".to_string(),
            other => format!("{other:?}").to_lowercase(),
        };
        if self.ctrl { format!("ctrl-{base}") } else { base }
    }
}
