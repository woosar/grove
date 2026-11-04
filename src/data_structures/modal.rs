use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use super::{Action, ActionPopup, ConfirmModal, Ctx, Effect, ErrorDialog, HelpModal, Pager, PromptModal, QuickSwitcher};

/// Anything drawn on top of a screen. The app keeps a stack; the top one gets keys.
pub enum Modal {
    Popup(ActionPopup),
    Switcher(QuickSwitcher),
    Error(ErrorDialog),
    Help(HelpModal),
    Prompt(PromptModal),
    Confirm(ConfirmModal),
    Pager(Pager),
}

impl Modal {
    pub fn handle_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        match self {
            Modal::Popup(m) => m.handle_key(key, ctx),
            Modal::Switcher(m) => m.handle_key(key, ctx),
            Modal::Error(m) => m.handle_key(key, ctx),
            Modal::Help(m) => m.handle_key(key),
            Modal::Prompt(m) => m.handle_key(key, ctx),
            Modal::Confirm(m) => m.handle_key(key),
            Modal::Pager(m) => m.handle_key(key),
        }
    }

    /// `screen` is the whole terminal minus the footer; `body` the area under the header.
    pub fn draw(&mut self, f: &mut Frame, screen: Rect, body: Rect, ctx: &Ctx) {
        match self {
            Modal::Popup(m) => m.draw(f, body, ctx),
            Modal::Switcher(m) => m.draw(f, body, ctx),
            Modal::Error(m) => m.draw(f, screen, ctx),
            Modal::Help(m) => m.draw(f, screen, ctx),
            Modal::Prompt(m) => m.draw(f, body, ctx),
            Modal::Confirm(m) => m.draw(f, body, ctx),
            Modal::Pager(m) => m.draw(f, body, ctx),
        }
    }

    /// What the footer's command preview shows while this modal is on top.
    pub fn preview(&self, ctx: &Ctx) -> Option<Action> {
        match self {
            Modal::Popup(m) => m.preview(),
            Modal::Switcher(m) => m.preview(ctx),
            Modal::Error(m) => m.preview(),
            Modal::Prompt(m) => m.preview(ctx),
            Modal::Confirm(m) => Some(m.action.clone()),
            Modal::Help(_) | Modal::Pager(_) => None,
        }
    }

    pub fn selected_branch(&self) -> Option<String> {
        match self {
            Modal::Switcher(m) => m.selected_branch(),
            _ => None,
        }
    }

    /// The error dialog dims everything behind it.
    pub fn dims_background(&self) -> bool {
        matches!(self, Modal::Error(_))
    }

    pub fn hints(&self) -> Vec<(&'static str, &'static str)> {
        match self {
            Modal::Popup(_) => vec![("j/k", "move"), ("⏎", "run"), ("?", "explain"), ("esc", "close")],
            Modal::Switcher(_) => vec![("⏎", "switch"), ("ctrl-w", "worktree"), ("ctrl-t", "track"), ("esc", "close")],
            Modal::Error(_) => vec![("j/k", "remedy"), ("⏎", "run"), ("?", "why"), ("tab", "raw output"), ("esc", "dismiss")],
            Modal::Help(_) => vec![("j/k", "scroll"), ("esc", "close")],
            Modal::Prompt(_) => vec![("⏎", "ok"), ("esc", "cancel")],
            Modal::Confirm(m) if m.action.destructive => vec![("Y", "confirm"), ("esc", "cancel")],
            Modal::Confirm(_) => vec![("y", "confirm"), ("esc", "cancel")],
            Modal::Pager(_) => vec![("j/k", "scroll"), ("space", "page"), ("y", "copy"), ("q", "close")],
        }
    }
}
