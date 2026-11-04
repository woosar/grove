use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// A single-line editable text field.
#[derive(Clone, Debug, Default)]
pub struct TextInput {
    pub value: String,
    /// Cursor position in chars.
    cursor: usize,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        TextInput { value, cursor }
    }

    pub fn set(&mut self, value: impl Into<String>) {
        *self = TextInput::new(value);
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.value.char_indices().nth(char_idx).map(|(i, _)| i).unwrap_or(self.value.len())
    }

    /// Applies an editing key; returns whether the value changed. Keys it doesn't use
    /// (enter, esc, tab, arrows up/down) return `false` and change nothing.
    pub fn handle(&mut self, key: &KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if ctrl => {
                let at = self.byte_at(self.cursor);
                self.value.replace_range(..at, "");
                self.cursor = 0;
                true
            }
            KeyCode::Char('a') if ctrl => {
                self.cursor = 0;
                false
            }
            KeyCode::Char('e') if ctrl => {
                self.cursor = self.value.chars().count();
                false
            }
            KeyCode::Char(c) if !ctrl => {
                let at = self.byte_at(self.cursor);
                self.value.insert(at, c);
                self.cursor += 1;
                true
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let at = self.byte_at(self.cursor - 1);
                self.value.remove(at);
                self.cursor -= 1;
                true
            }
            KeyCode::Delete if self.cursor < self.value.chars().count() => {
                let at = self.byte_at(self.cursor);
                self.value.remove(at);
                true
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                false
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.value.chars().count());
                false
            }
            KeyCode::Home => {
                self.cursor = 0;
                false
            }
            KeyCode::End => {
                self.cursor = self.value.chars().count();
                false
            }
            _ => false,
        }
    }

    /// The value with a block cursor, when focused.
    pub fn line<'a>(&self, style: Style, focused: bool) -> Line<'a> {
        if !focused {
            return Line::from(Span::styled(self.value.clone(), style));
        }
        let at = self.byte_at(self.cursor);
        let (before, rest) = self.value.split_at(at);
        let mut chars = rest.chars();
        let under = chars.next().map(|c| c.to_string()).unwrap_or_else(|| " ".to_string());
        Line::from(vec![
            Span::styled(before.to_string(), style),
            Span::styled(under, style.add_modifier(Modifier::REVERSED)),
            Span::styled(chars.as_str().to_string(), style),
        ])
    }
}
