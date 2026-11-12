use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Action, ActionPopup, Ctx, Effect, Modal, Verdict, Worktree, WorktreeForm};
use crate::helpers::{home_tilde, keep_visible, nav, norm_path, pad, relative_age_long, short_oid, str_width, truncate, wrap_text};

/// Screen 4: all worktrees, the selected one's state and actions, and the new-worktree form.
#[derive(Default)]
pub struct WorktreesScreen {
    sel: usize,
    offset: usize,
    form: Option<WorktreeForm>,
    anchor: Option<Rect>,
}

impl WorktreesScreen {
    pub fn open_form(&mut self, branch: Option<String>, ctx: &Ctx) {
        self.form = Some(WorktreeForm::new(branch, ctx));
    }

    pub fn form_open(&self) -> bool {
        self.form.is_some()
    }

    fn selected<'a>(&self, ctx: &Ctx<'a>) -> Option<&'a Worktree> {
        ctx.snap.worktrees.get(self.sel.min(ctx.snap.worktrees.len().saturating_sub(1)))
    }

    pub fn preview(&self, ctx: &Ctx) -> Option<Action> {
        if let Some(form) = &self.form {
            return form.action(ctx).ok();
        }
        let actions = ctx.actions().worktree_actions(self.selected(ctx)?);
        actions.first().cloned()
    }

    pub fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if let Some(form) = &self.form {
            return form.hints();
        }
        vec![
            ("n", "new"),
            ("⏎", "open"),
            ("x", "remove"),
            ("X", "force remove"),
            ("p", "prune"),
            ("L", "lock / unlock"),
            ("m", "move"),
            ("ctrl-tab", "next worktree tab"),
            ("?", "explain"),
        ]
    }

    pub fn handle_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        if let Some(form) = &mut self.form {
            if key.code == KeyCode::Esc {
                self.form = None;
                return Some(Effect::Nothing);
            }
            return form.handle_key(key, ctx);
        }
        if let Some(s) = nav(key, self.sel, ctx.snap.worktrees.len()) {
            self.sel = s;
            return Some(Effect::Nothing);
        }
        if key.code == KeyCode::Char('n') {
            self.open_form(None, ctx);
            return Some(Effect::Nothing);
        }
        let wt = self.selected(ctx)?;
        let actions = ctx.actions().worktree_actions(wt);
        match key.code {
            KeyCode::Char(' ') => Some(Effect::open(Modal::Popup(ActionPopup::new(
                ctx.snap.short_path(&wt.path),
                wt.branch.clone().unwrap_or_else(|| "detached".into()),
                actions,
                self.anchor,
            )))),
            _ => {
                let a = actions.into_iter().find(|a| a.key.matches(key))?;
                let via = a.key.label(&ctx.theme.g);
                Some(Effect::run(a, via))
            }
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let n = ctx.snap.worktrees.len();
        self.sel = self.sel.min(n.saturating_sub(1));
        // at least 5 rows, at most 2/5 of the screen — the cap wins on tiny terminals
        let table_h = ((n + 4) as u16).max(5).min(area.height * 2 / 5);
        let [top, bottom] = Layout::vertical([Constraint::Length(table_h), Constraint::Min(5)]).spacing(1).areas(area);
        self.draw_table(f, top, ctx);
        if bottom.width >= 100 {
            let [left, right] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).spacing(1).areas(bottom);
            self.draw_detail(f, left, ctx);
            self.draw_form(f, right, ctx);
        } else if self.form.is_some() {
            self.draw_form(f, bottom, ctx);
        } else {
            self.draw_detail(f, bottom, ctx);
        }
    }

    fn state(wt: &Worktree, ctx: &Ctx) -> (String, ratatui::style::Color, String) {
        let t = ctx.theme;
        let g = &t.g;
        if wt.prunable.is_some() {
            (format!("{} prunable", g.cross), t.err, "directory missing".into())
        } else if let Some(reason) = &wt.locked {
            let detail = if reason.is_empty() { "locked".to_string() } else { format!("locked: \"{reason}\"") };
            (format!("{} locked", g.diamond), t.tag, detail)
        } else if wt.is_dirty() {
            (format!("{} dirty", g.dot), t.accent, wt.dirty.map(|d| d.describe()).unwrap_or_default())
        } else if wt.bare {
            ("bare".into(), t.dim, "—".into())
        } else {
            ("clean".into(), t.ok, "—".into())
        }
    }

    fn draw_table(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let snap = ctx.snap;
        let title = format!("Worktrees {} {}", g.sep, snap.worktrees.len());
        let hint = format!("all share one .git {} {}", g.sep, home_tilde(&snap.common_dir));
        let block = t.pane(title, &hint, self.form.is_none());
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let path_w = (w / 4).clamp(14, 40);
        let branch_w = (w / 6).clamp(10, 28);
        let head_w = 10;
        let state_w = 13;
        let used_w = 10;
        let change_w = w.saturating_sub(2 + path_w + branch_w + head_w + state_w + used_w);
        let mut lines = vec![
            Line::from(vec![
                Span::raw("  "),
                Span::styled(pad("path", path_w), t.dimmed()),
                Span::styled(pad("branch", branch_w), t.dimmed()),
                Span::styled(pad("HEAD", head_w), t.dimmed()),
                Span::styled(pad("state", state_w), t.dimmed()),
                Span::styled(pad("changes", change_w), t.dimmed()),
                Span::styled(pad("last used", used_w), t.dimmed()),
            ]),
            Line::from(Span::styled("─".repeat(w), t.s(t.border))),
        ];
        let body_h = inner.height.saturating_sub(2) as usize;
        keep_visible(&mut self.offset, self.sel, body_h);
        for (i, wt) in snap.worktrees.iter().enumerate().skip(self.offset).take(body_h) {
            let here = crate::helpers::same_path(&wt.path, &snap.workdir);
            let selected = i == self.sel;
            let mark = if here { g.star } else if selected { g.pointer } else { " " };
            let (state, color, changes) = Self::state(wt, ctx);
            let last = if here {
                "now".to_string()
            } else {
                ctx.last_used.get(&norm_path(&wt.path)).map(|ms| relative_age_long(ms / 1000)).unwrap_or_else(|| "—".into())
            };
            lines.push(t.row(
                vec![
                    Span::styled(format!("{mark} "), t.s(t.accent)),
                    Span::styled(pad(&snap.short_path(&wt.path), path_w - 1), t.text()),
                    Span::raw(" "),
                    Span::styled(pad(wt.branch.as_deref().unwrap_or("(detached)"), branch_w), t.s(t.branch)),
                    Span::styled(pad(short_oid(&wt.head), head_w), t.dimmed()),
                    Span::styled(pad(&state, state_w), t.s(color)),
                    Span::styled(pad(&changes, change_w), t.dimmed()),
                    Span::styled(pad(&last, used_w), t.dimmed()),
                ],
                w,
                selected && self.form.is_none(),
            ));
        }
        self.anchor = Some(Rect::new(inner.x, inner.y + 2 + (self.sel - self.offset) as u16, inner.width, 1));
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_detail(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let Some(wt) = self.selected(ctx) else {
            f.render_widget(t.pane("", "", false), area);
            return;
        };
        let block = t.pane(ctx.snap.short_path(&wt.path), "", false);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let branch = wt.branch.clone().unwrap_or_else(|| "(detached)".into());
        let (state, color, _) = Self::state(wt, ctx);
        let (summary, story) = if wt.prunable.is_some() {
            (
                "the directory is gone but git still has it registered".to_string(),
                format!("Probably deleted with a file manager instead of git worktree remove. While registered, its branch {branch} counts as \"checked out\" and you can't switch to it anywhere else."),
            )
        } else if let Some(reason) = &wt.locked {
            (
                if reason.is_empty() { "protected from prune and remove".to_string() } else { format!("\"{reason}\"") },
                "A locked worktree is protected from git worktree prune and remove — e.g. one on a removable drive or in use by a CI job. Unlock it when it's no longer needed.".to_string(),
            )
        } else if wt.is_dirty() {
            (
                wt.dirty.map(|d| d.describe()).unwrap_or_default(),
                "Uncommitted work lives only in this directory. Commit or stash it there before removing the worktree.".to_string(),
            )
        } else if wt.is_main {
            ("the main worktree".to_string(), "The original checkout; it holds the shared .git directory and can't be removed or moved.".to_string())
        } else {
            ("nothing uncommitted".to_string(), format!("Safe to remove: only the checkout goes; {branch} and its commits stay."))
        };
        let mut lines = vec![Line::from(vec![
            Span::styled(state, t.s(color)),
            Span::styled(format!(" {} {summary}", g.dash), t.dimmed()),
        ])];
        lines.push(Line::raw(""));
        for l in wrap_text(&story, w) {
            lines.push(Line::from(Span::styled(l, t.text())));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(pad("branch", 10), t.dimmed()),
            Span::styled(branch.clone(), t.s(t.branch)),
            Span::styled(if wt.is_main { String::new() } else { format!(" {} stays, only the worktree entry goes", g.sep) }, t.dimmed()),
        ]));
        if !wt.is_main {
            let name = wt.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(pad("admin dir", 10), t.dimmed()),
                Span::styled(format!(".git/worktrees/{name}"), t.text()),
            ]));
        }
        lines.push(Line::from(Span::styled("─".repeat(w), t.s(t.border))));
        for a in ctx.actions().worktree_actions(wt) {
            let blocked = a.verdict.is_blocked();
            let chip = if blocked { t.key_dim(a.key.label(g)) } else { t.key(a.key.label(g)) };
            let text = if a.plan.is_internal() { a.explain.clone() } else { a.plan.display(&ctx.snap.workdir) };
            let mut spans = vec![chip, Span::styled(format!(" {}", pad(a.label, 9)), t.text())];
            spans.push(Span::styled(text.clone(), if blocked || a.plan.is_internal() { t.dimmed() } else { t.s(t.accent) }));
            let room = w.saturating_sub(13 + str_width(&text));
            let (note, style) = match &a.verdict {
                Verdict::Blocked { reason, .. } => (reason.clone(), t.s(t.err)),
                _ => (a.note.clone(), t.dimmed()),
            };
            if room > 6 && !note.is_empty() {
                spans.push(Span::styled(format!("  {}", truncate(&note, room - 2)), style));
            }
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_form(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        match &self.form {
            Some(form) => form.draw(f, area, ctx),
            None => {
                let t = ctx.theme;
                let block = t.pane("New worktree", "n", false);
                let inner = block.inner(area);
                f.render_widget(block, area);
                let mut lines = vec![Line::from(vec![t.key_dim("n"), Span::styled(" new worktree", t.text())]), Line::raw("")];
                for l in wrap_text(
                    "A worktree is a second checkout of this repository in another directory. Use one to look at or build another branch without stashing or switching here.",
                    inner.width as usize,
                ) {
                    lines.push(Line::from(Span::styled(l, t.dimmed())));
                }
                f.render_widget(Paragraph::new(lines), inner);
            }
        }
    }
}
