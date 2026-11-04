use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Action, ActionPopup, Branch, ChangeArea, Ctx, Effect, FileChange, InternalOp, KeyBinding, Modal, QuickSwitcher, ScreenId, Snapshot, Theme};
use crate::helpers::{fmt_duration_ms, keep_visible, local_hms, nav, pad, pad_left, relative_age, str_width, truncate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Status,
    Branches,
    Worktrees,
    Graph,
    Journal,
}

const PANES: [Pane; 5] = [Pane::Status, Pane::Branches, Pane::Worktrees, Pane::Graph, Pane::Journal];

/// Screen 1: status, recent branches, worktrees, graph and journal tail at a glance.
pub struct DashboardScreen {
    focus: Pane,
    sel: [usize; 5],
    offset: [usize; 5],
    anchor: Option<Rect>,
}

impl Default for DashboardScreen {
    fn default() -> Self {
        DashboardScreen { focus: Pane::Branches, sel: [0; 5], offset: [0; 5], anchor: None }
    }
}

impl DashboardScreen {
    fn idx(p: Pane) -> usize {
        PANES.iter().position(|x| *x == p).unwrap_or(0)
    }

    fn status_items(snap: &Snapshot) -> Vec<&FileChange> {
        let mut out = Vec::new();
        for area in [ChangeArea::Conflicted, ChangeArea::Staged, ChangeArea::Unstaged, ChangeArea::Untracked] {
            out.extend(snap.status.in_area(area));
        }
        out
    }

    fn branches(snap: &Snapshot) -> Vec<&Branch> {
        snap.local_branches().collect()
    }

    fn len(&self, p: Pane, ctx: &Ctx) -> usize {
        match p {
            Pane::Status => Self::status_items(ctx.snap).len(),
            Pane::Branches => Self::branches(ctx.snap).len(),
            Pane::Worktrees => ctx.snap.worktrees.len(),
            Pane::Graph => ctx.snap.recent.len(),
            Pane::Journal => ctx.tail.len(),
        }
    }

    fn sel(&self, p: Pane, ctx: &Ctx) -> usize {
        self.sel[Self::idx(p)].min(self.len(p, ctx).saturating_sub(1))
    }

    /// Title and info line for the popup, plus the actions of the focused item.
    fn item_actions(&self, ctx: &Ctx) -> Option<(String, String, Vec<Action>)> {
        let f = ctx.actions();
        let g = &ctx.theme.g;
        let s = self.sel(self.focus, ctx);
        match self.focus {
            Pane::Status => match Self::status_items(ctx.snap).get(s) {
                Some(fc) => Some((fc.display(), format!("{:?}", fc.area).to_lowercase(), f.file_actions(fc))),
                None => Some(("working tree".into(), "clean".into(), vec![f.commit(), f.stage_all()])),
            },
            Pane::Branches => Self::branches(ctx.snap).get(s).map(|b| {
                let mut info = Vec::new();
                if let (Some((a, _)), Some(m)) = (b.vs_main, &ctx.snap.main)
                    && m != &b.name
                {
                    info.push(format!("{a} ahead of {m}"));
                }
                info.push(format!("last commit {}", crate::helpers::relative_age_long(b.time)));
                (b.name.clone(), info.join(&format!(" {} ", g.sep)), f.branch_actions(b))
            }),
            Pane::Worktrees => ctx.snap.worktrees.get(s).map(|w| {
                (ctx.snap.short_path(&w.path), w.branch.clone().unwrap_or_else(|| "detached".into()), f.worktree_actions(w))
            }),
            Pane::Graph => ctx.snap.recent.get(s).map(|c| (crate::helpers::short_oid(&c.oid).to_string(), c.subject.clone(), f.commit_actions(c))),
            Pane::Journal => None,
        }
    }

    /// Keys that act directly on the focused item; everything else stays global.
    fn direct_keys(&self) -> &'static [char] {
        match self.focus {
            Pane::Status => &['a', 'c', 'A', 'X'],
            Pane::Branches => &['s', 'm', 'r', 'd', 'w', 'R'],
            Pane::Worktrees => &['x', 'L', 'm'],
            Pane::Graph => &['y', 't'],
            Pane::Journal => &[],
        }
    }

    fn primary(actions: &[Action]) -> Option<&Action> {
        actions
            .iter()
            .find(|a| a.key == KeyBinding::enter() && !a.verdict.is_blocked())
            .or(actions.first())
    }

    pub fn preview(&self, ctx: &Ctx) -> Option<Action> {
        let (_, _, actions) = self.item_actions(ctx)?;
        Self::primary(&actions).cloned()
    }

    pub fn selected_branch(&self, ctx: &Ctx) -> Option<String> {
        (self.focus == Pane::Branches)
            .then(|| Self::branches(ctx.snap).get(self.sel(Pane::Branches, ctx)).map(|b| b.name.clone()))
            .flatten()
    }

    pub fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("tab", "pane"),
            ("j/k", "move"),
            ("space", "actions"),
            ("b", "quick switch"),
            ("f", "fetch"),
            ("p", "pull"),
            ("P", "push"),
            (":", "raw git"),
            ("?", "help"),
            ("q", "quit"),
        ]
    }

    pub fn focus_status(&mut self) {
        self.focus = Pane::Status;
    }

    pub fn handle_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        let g = &ctx.theme.g;
        let i = Self::idx(self.focus);
        match key.code {
            KeyCode::Tab => {
                self.focus = PANES[(i + 1) % PANES.len()];
                return Some(Effect::Nothing);
            }
            KeyCode::BackTab => {
                self.focus = PANES[(i + PANES.len() - 1) % PANES.len()];
                return Some(Effect::Nothing);
            }
            _ => {}
        }
        if let Some(s) = nav(key, self.sel(self.focus, ctx), self.len(self.focus, ctx)) {
            self.sel[i] = s;
            return Some(Effect::Nothing);
        }
        if self.focus == Pane::Journal {
            return matches!(key.code, KeyCode::Enter).then_some(Effect::Internal(InternalOp::GoTo(ScreenId::Journal)));
        }
        match key.code {
            KeyCode::Char('/') if self.focus == Pane::Branches => return Some(Effect::open(Modal::Switcher(QuickSwitcher::new(ctx)))),
            KeyCode::Char('n') if self.focus == Pane::Branches => return Some(Effect::run(ctx.actions().new_branch(), "n")),
            _ => {}
        }
        let (title, info, actions) = self.item_actions(ctx)?;
        match key.code {
            KeyCode::Char(' ') => Some(Effect::open(Modal::Popup(ActionPopup::new(title, info, actions, self.anchor)))),
            KeyCode::Enter => {
                let a = Self::primary(&actions)?.clone();
                let via = a.key.label(g);
                Some(Effect::run(a, via))
            }
            KeyCode::Char(c) if self.direct_keys().contains(&c) => {
                let a = actions.into_iter().find(|a| a.key.matches(key))?;
                Some(Effect::run(a, c.to_string()))
            }
            _ => None,
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let snap = ctx.snap;
        let sections = [ChangeArea::Conflicted, ChangeArea::Staged, ChangeArea::Unstaged, ChangeArea::Untracked]
            .iter()
            .filter(|a| snap.status.count(**a) > 0)
            .count();
        let status_h = (Self::status_items(snap).len() + sections + 2).clamp(3, 12) as u16;
        let wt_h = (snap.worktrees.len() + 2).clamp(3, 8) as u16;
        let (status, branches, worktrees, graph, journal);
        if area.width >= 120 {
            let left_w = (area.width * 38 / 100).clamp(50, 70);
            let [left, right] = Layout::horizontal([Constraint::Length(left_w), Constraint::Min(0)]).spacing(1).areas(area);
            [status, branches, worktrees] =
                Layout::vertical([Constraint::Length(status_h), Constraint::Min(4), Constraint::Length(wt_h)]).areas(left);
            [graph, journal] = Layout::vertical([Constraint::Min(5), Constraint::Length(9)]).areas(right);
        } else {
            [status, branches, worktrees, graph, journal] = Layout::vertical([
                Constraint::Length(status_h.min(8)),
                Constraint::Min(4),
                Constraint::Length(wt_h.min(5)),
                Constraint::Min(5),
                Constraint::Length(6),
            ])
            .areas(area);
        }
        self.anchor = None;
        self.draw_status(f, status, ctx);
        self.draw_branches(f, branches, ctx);
        self.draw_worktrees(f, worktrees, ctx);
        self.draw_graph(f, graph, ctx);
        self.draw_journal(f, journal, ctx);
    }

    /// Renders `rows` with the pane's scroll offset kept around the selection.
    fn render_list(&mut self, f: &mut Frame, inner: Rect, pane: Pane, rows: Vec<Line<'static>>, sel_row: Option<usize>) {
        let i = Self::idx(pane);
        let h = inner.height as usize;
        if let Some(s) = sel_row {
            keep_visible(&mut self.offset[i], s, h);
            if self.focus == pane {
                let y = inner.y + (s - self.offset[i]) as u16;
                self.anchor = Some(Rect::new(inner.x, y, inner.width, 1));
            }
        }
        let visible: Vec<Line> = rows.into_iter().skip(self.offset[i]).take(h).collect();
        f.render_widget(Paragraph::new(visible), inner);
    }

    fn draw_status(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let focused = self.focus == Pane::Status;
        let block = t.pane("Status", &format!("a stage {} c commit", t.g.sep), focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let items = Self::status_items(ctx.snap);
        if items.is_empty() {
            f.render_widget(Paragraph::new(Line::from(Span::styled("clean — nothing to commit", t.dimmed()))), inner);
            return;
        }
        let sel = self.sel(Pane::Status, ctx);
        let mut rows = Vec::new();
        let mut sel_row = None;
        let mut idx = 0;
        for (area_kind, title, color) in [
            (ChangeArea::Conflicted, "Conflicted", t.err),
            (ChangeArea::Staged, "Staged", t.ok),
            (ChangeArea::Unstaged, "Unstaged", t.accent),
            (ChangeArea::Untracked, "Untracked", t.dim),
        ] {
            let n = ctx.snap.status.count(area_kind);
            if n == 0 {
                continue;
            }
            rows.push(Line::from(Span::styled(format!("{title} ({n})"), t.s(color))));
            for fc in ctx.snap.status.in_area(area_kind) {
                let selected = focused && idx == sel;
                if idx == sel {
                    sel_row = Some(rows.len());
                }
                let counts = fc.line_counts();
                let path_w = w.saturating_sub(5 + str_width(&counts) + 1);
                rows.push(t.row(
                    vec![
                        Span::styled(format!("  {} ", fc.code), t.s(color)),
                        Span::styled(pad(&fc.display(), path_w), t.text()),
                        Span::styled(format!(" {counts}"), t.dimmed()),
                    ],
                    w,
                    selected,
                ));
                idx += 1;
            }
        }
        self.render_list(f, inner, Pane::Status, rows, sel_row);
    }

    fn ab_label(b: &Branch, snap: &Snapshot, t: &Theme) -> (String, ratatui::style::Color) {
        let g = &t.g;
        if snap.main.as_deref() == Some(&b.name) {
            return match &b.upstream {
                Some(_) if b.ahead == 0 && b.behind == 0 => ("synced".into(), t.dim),
                Some(_) => (format!("{}{} {}{}", g.up, b.ahead, g.down, b.behind), t.accent),
                None => ("local".into(), t.dim),
            };
        }
        match b.vs_main {
            Some((0, _)) if snap.merged.contains(&b.name) => ("merged".into(), t.dim),
            Some((a, behind)) => {
                let color = if behind > 20 { t.err } else if behind > 0 { t.accent } else { t.ok };
                (format!("{}{a} {}{behind}", g.up, g.down), color)
            }
            None => (String::new(), t.dim),
        }
    }

    fn draw_branches(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let focused = self.focus == Pane::Branches;
        let block = t.pane(format!("Branches {} recent", g.sep), &format!("/ find {} n new", g.sep), focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let sel = self.sel(Pane::Branches, ctx);
        let rows: Vec<Line<'static>> = Self::branches(ctx.snap)
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let selected = focused && i == sel;
                let mark = if b.is_head { g.star } else if selected { g.pointer } else { " " };
                let name_color = if b.is_head { t.accent } else if selected { t.branch } else { t.fg };
                let (ab, ab_color) = Self::ab_label(b, ctx.snap, t);
                let wt = if b.worktree.is_some() && !b.is_head { "wt" } else { "" };
                let name_w = w.saturating_sub(2 + 10 + 5 + 4);
                t.row(
                    vec![
                        Span::styled(format!("{mark} "), t.s(t.accent)),
                        Span::styled(pad(&b.name, name_w), t.s(name_color)),
                        Span::styled(pad_left(&ab, 9), t.s(ab_color)),
                        Span::styled(pad_left(&relative_age(b.time), 5), t.dimmed()),
                        Span::styled(format!("  {}", pad(wt, 2)), t.s(t.tag)),
                    ],
                    w,
                    selected,
                )
            })
            .collect();
        let sel_row = (!rows.is_empty()).then_some(sel);
        self.render_list(f, inner, Pane::Branches, rows, sel_row);
    }

    fn draw_worktrees(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let focused = self.focus == Pane::Worktrees;
        let block = t.pane("Worktrees", "4 manage", focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let sel = self.sel(Pane::Worktrees, ctx);
        let rows: Vec<Line<'static>> = ctx
            .snap
            .worktrees
            .iter()
            .enumerate()
            .map(|(i, wt)| {
                let (state, color) = if wt.prunable.is_some() {
                    ("prune".to_string(), t.err)
                } else if wt.locked.is_some() {
                    ("locked".to_string(), t.tag)
                } else if wt.is_dirty() {
                    (format!("{} dirty", t.g.dot), t.accent)
                } else {
                    ("clean".to_string(), t.ok)
                };
                let branch_w = (w / 3).min(24);
                let path_w = w.saturating_sub(branch_w + 10);
                let branch = wt.branch.clone().unwrap_or_else(|| format!("({})", crate::helpers::short_oid(&wt.head)));
                t.row(
                    vec![
                        Span::styled(pad(&ctx.snap.short_path(&wt.path), path_w), t.text()),
                        Span::raw(" "),
                        Span::styled(pad(&branch, branch_w), t.s(t.branch)),
                        Span::styled(format!(" {}", pad(&state, 8)), t.s(color)),
                    ],
                    w,
                    focused && i == sel,
                )
            })
            .collect();
        let sel_row = (!rows.is_empty()).then_some(sel);
        self.render_list(f, inner, Pane::Worktrees, rows, sel_row);
    }

    fn draw_graph(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let focused = self.focus == Pane::Graph;
        let block = t.pane(format!("Graph {} all branches", t.g.sep), "2 full log", focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let graph_w = ctx.snap.recent_graph.iter().map(|r| r.width()).max().unwrap_or(1).min(w / 4).max(1);
        let sel = self.sel(Pane::Graph, ctx);
        let rows: Vec<Line<'static>> = ctx
            .snap
            .recent
            .iter()
            .zip(&ctx.snap.recent_graph)
            .enumerate()
            .map(|(i, (c, row))| t.commit_line(row, c, graph_w, w, focused && i == sel, false))
            .collect();
        if rows.is_empty() {
            f.render_widget(Paragraph::new(Span::styled("no commits yet", t.dimmed())), inner);
            return;
        }
        self.render_list(f, inner, Pane::Graph, rows, Some(sel));
    }

    fn draw_journal(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let focused = self.focus == Pane::Journal;
        let block = t.pane(format!("Journal {} tail", g.sep), &format!("5 open {} u undo last", g.sep), focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        if ctx.tail.is_empty() {
            f.render_widget(
                Paragraph::new(Span::styled("nothing yet — every git command grove runs shows up here", t.dimmed())),
                inner,
            );
            return;
        }
        let sel = self.sel(Pane::Journal, ctx);
        let rows: Vec<Line<'static>> = ctx
            .tail
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let (mark, color) = if e.ok() { (g.check, t.ok) } else { (g.cross, t.err) };
                let exit = if e.ok() { String::new() } else { e.exit_code.map(|c| format!("exit {c}")).unwrap_or_else(|| "failed".into()) };
                let cmd_w = w.saturating_sub(9 + 2 + 8 + 7);
                t.row(
                    vec![
                        Span::styled(format!("{} ", local_hms(e.started_at)), t.dimmed()),
                        Span::styled(format!("{mark} "), t.s(color)),
                        Span::styled(pad(&truncate(&e.command(), cmd_w), cmd_w), t.text()),
                        Span::styled(pad_left(&exit, 8), t.s(color)),
                        Span::styled(pad_left(&fmt_duration_ms(e.duration_ms), 7), t.dimmed()),
                    ],
                    w,
                    focused && i == sel,
                )
            })
            .collect();
        self.render_list(f, inner, Pane::Journal, rows, Some(sel));
    }
}
