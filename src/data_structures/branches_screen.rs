use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Action, ActionPopup, Branch, Ctx, Effect, KeyBinding, Modal, QuickSwitcher, Snapshot};
use crate::helpers::{keep_visible, nav, pad, pad_left, relative_age, relative_age_long};

/// Screen 3: every branch with upstream, ahead/behind, last commit and worktree.
#[derive(Default)]
pub struct BranchesScreen {
    sel: usize,
    offset: usize,
    by_name: bool,
    anchor: Option<Rect>,
}

impl BranchesScreen {
    /// Locals first, then remote branches without a local counterpart.
    fn rows<'a>(&self, snap: &'a Snapshot) -> (Vec<&'a Branch>, usize) {
        let mut locals: Vec<&Branch> = snap.local_branches().collect();
        let mut remotes: Vec<&Branch> = snap.remote_only().collect();
        if self.by_name {
            locals.sort_by(|a, b| a.name.cmp(&b.name));
            remotes.sort_by(|a, b| a.name.cmp(&b.name));
        }
        let n = locals.len();
        locals.extend(remotes);
        (locals, n)
    }

    fn selected<'a>(&self, snap: &'a Snapshot) -> Option<&'a Branch> {
        let (rows, _) = self.rows(snap);
        rows.get(self.sel.min(rows.len().saturating_sub(1))).copied()
    }

    pub fn selected_branch(&self, ctx: &Ctx) -> Option<String> {
        self.selected(ctx.snap).filter(|b| !b.remote).map(|b| b.name.clone())
    }

    fn primary(actions: &[Action]) -> Option<&Action> {
        actions
            .iter()
            .find(|a| a.key == KeyBinding::enter() && !a.verdict.is_blocked())
            .or(actions.first())
    }

    pub fn preview(&self, ctx: &Ctx) -> Option<Action> {
        let b = self.selected(ctx.snap)?;
        Self::primary(&ctx.actions().branch_actions(b)).cloned()
    }

    pub fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("b", "quick switch"),
            ("-", "previous branch"),
            ("n", "new"),
            ("R", "rename"),
            ("d", "delete"),
            ("u", "set upstream"),
            ("D", "prune merged"),
            ("o", "sort"),
            ("?", "explain"),
        ]
    }

    pub fn handle_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        let f = ctx.actions();
        let g = &ctx.theme.g;
        let (rows, _) = self.rows(ctx.snap);
        if let Some(s) = nav(key, self.sel, rows.len()) {
            self.sel = s;
            return Some(Effect::Nothing);
        }
        match key.code {
            KeyCode::Char('-') => return Some(Effect::run(f.previous_branch(), "-")),
            KeyCode::Char('n') => return Some(Effect::run(f.new_branch(), "n")),
            KeyCode::Char('D') => return Some(Effect::run(f.prune_merged(), "D")),
            KeyCode::Char('/') => return Some(Effect::open(Modal::Switcher(QuickSwitcher::new(ctx)))),
            KeyCode::Char('o') => {
                self.by_name = !self.by_name;
                return Some(Effect::Nothing);
            }
            _ => {}
        }
        let b = self.selected(ctx.snap)?;
        let actions = f.branch_actions(b);
        match key.code {
            KeyCode::Char(' ') => {
                let info = b.upstream.clone().map(|u| format!("tracks {u}")).unwrap_or_else(|| "no upstream".into());
                Some(Effect::open(Modal::Popup(ActionPopup::new(b.name.clone(), info, actions, self.anchor))))
            }
            KeyCode::Enter => {
                let a = Self::primary(&actions)?.clone();
                let via = a.key.label(g);
                Some(Effect::run(a, via))
            }
            KeyCode::Char(c) if "smrduRw".contains(c) => {
                let a = actions.into_iter().find(|a| a.key.matches(key))?;
                Some(Effect::run(a, c.to_string()))
            }
            _ => None,
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let snap = ctx.snap;
        let (rows, n_local) = self.rows(snap);
        self.sel = self.sel.min(rows.len().saturating_sub(1));
        let title = format!("Branches {} local {} {} remote {}", g.sep, n_local, g.sep, snap.branches.iter().filter(|b| b.remote).count());
        let sort = format!("sort: {} {}", if self.by_name { "name" } else { "recent" }, g.tri_down);
        let block = t.pane(title, &sort, true);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;

        // column widths: marker, name, upstream, ↑↓, subject (flex), age, worktree
        let name_w = (w / 5).clamp(14, 34);
        let up_w = (w / 6).clamp(10, 28);
        let ab_w = 9;
        let age_w = 6;
        let wt_w = if w > 110 { (w / 6).min(30) } else { 0 };
        let subj_w = w.saturating_sub(2 + name_w + up_w + ab_w + age_w + wt_w + 2);
        let header = Line::from(vec![
            Span::raw("  "),
            Span::styled(pad("name", name_w), t.dimmed()),
            Span::styled(pad("upstream", up_w), t.dimmed()),
            Span::styled(pad(&format!("{}{}", g.up, g.down), ab_w), t.dimmed()),
            Span::styled(pad("last commit", subj_w), t.dimmed()),
            Span::styled(pad_left("age", age_w), t.dimmed()),
            Span::styled(format!("  {}", pad("worktree", wt_w)), t.dimmed()),
        ]);
        let mut lines = vec![header, Line::from(Span::styled("─".repeat(w), t.s(t.border)))];
        let body_h = inner.height.saturating_sub(2) as usize;

        let mut table: Vec<Line> = Vec::new();
        let mut sel_line = 0;
        for (i, b) in rows.iter().enumerate() {
            if i == n_local && i < rows.len() {
                table.push(Line::raw(""));
                table.push(Line::from(Span::styled("Remote only", t.dimmed())));
            }
            let selected = i == self.sel;
            if selected {
                sel_line = table.len();
            }
            let mark = if b.is_head { g.star } else if selected { g.pointer } else { " " };
            let name_color = if b.is_head { t.accent } else if b.remote { t.dim } else if b.worktree.is_some() { t.branch } else { t.fg };
            let (upstream, up_color) = match (&b.upstream, b.upstream_gone) {
                (Some(_), true) => ("gone (deleted on remote)".to_string(), t.err),
                (Some(u), false) => (u.clone(), t.dim),
                (None, _) if b.remote => ("—".to_string(), t.dim),
                (None, _) => ("—".to_string(), t.dim),
            };
            let (ab, ab_color) = if b.upstream.is_some() && !b.upstream_gone {
                let color = if b.behind > 0 && b.ahead > 0 { t.err } else if b.behind > 0 { t.accent } else if b.ahead > 0 { t.ok } else { t.dim };
                (format!("{}{} {}{}", g.up, b.ahead, g.down, b.behind), color)
            } else {
                (String::new(), t.dim)
            };
            let wt = b.worktree.as_ref().map(|p| snap.short_path(p)).unwrap_or_default();
            table.push(t.row(
                vec![
                    Span::styled(format!("{mark} "), t.s(t.accent)),
                    Span::styled(pad(&b.name, name_w), t.s(name_color)),
                    Span::styled(pad(&upstream, up_w), t.s(up_color)),
                    Span::styled(pad(&ab, ab_w), t.s(ab_color)),
                    Span::styled(pad(&b.subject, subj_w), if b.remote { t.dimmed() } else { t.text() }),
                    Span::styled(pad_left(&relative_age(b.time), age_w), t.dimmed()),
                    Span::styled(format!("  {}", pad(&wt, wt_w)), t.s(t.tag)),
                ],
                w,
                selected,
            ));
        }
        keep_visible(&mut self.offset, sel_line, body_h);
        self.anchor = Some(Rect::new(inner.x, inner.y + 2 + (sel_line - self.offset) as u16, inner.width, 1));
        lines.extend(table.into_iter().skip(self.offset).take(body_h));
        if rows.is_empty() {
            lines.push(Line::from(Span::styled("no branches yet", t.dimmed())));
        } else if let Some(b) = rows.get(self.sel) {
            // fill the remaining space with a short description of the selection
            let spare = body_h.saturating_sub(lines.len() - 2);
            if spare > 3 {
                let wt = b.worktree.as_ref().map(|p| format!(" {} checked out in {}", g.sep, snap.short_path(p))).unwrap_or_default();
                lines.push(Line::raw(""));
                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", b.name), t.bold(t.branch)),
                    Span::styled(format!("{} last commit {}{wt}", g.sep, relative_age_long(b.time)), t.dimmed()),
                ]));
            }
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}
