use std::path::PathBuf;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::{Action, ChangeArea, Ctx, Effect, RepoStatus, Verdict};
use crate::helpers::{centered, keep_visible, nav, pad, pad_left, str_width, truncate, wrap_text};

/// How much of a path is (or will be) staged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    All,
    /// Staged and unstaged changes both; left alone unless toggled.
    Partial,
    None,
}

/// One path of the dialog, with all its changes.
struct Row {
    path: PathBuf,
    /// The old path of a staged rename; unstaging takes both back out.
    orig: Option<PathBuf>,
    /// Status letters: index side, worktree side (`?` untracked, `U` conflicted).
    index: char,
    worktree: char,
    area: ChangeArea,
    counts: String,
    initial: Mark,
    mark: Mark,
}

impl Row {
    fn display(&self) -> String {
        let path = self.path.to_string_lossy().replace('\\', "/");
        match &self.orig {
            Some(o) => format!("{} → {path}", o.to_string_lossy().replace('\\', "/")),
            None => path,
        }
    }
}

fn arg(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// `a` on the Log screen: every changed file with a checkbox. Checked files get staged,
/// unchecked staged ones unstaged; `c` stages and then asks for a commit message.
pub struct StageModal {
    rows: Vec<Row>,
    sel: usize,
    offset: usize,
}

impl StageModal {
    /// `None` when there is nothing to stage or unstage.
    pub fn new(status: &RepoStatus) -> Option<Self> {
        let mut rows: Vec<Row> = Vec::new();
        for c in &status.changes {
            let i = match rows.iter().position(|r| r.path == c.path) {
                Some(i) => i,
                None => {
                    rows.push(Row {
                        path: c.path.clone(),
                        orig: None,
                        index: ' ',
                        worktree: ' ',
                        area: c.area,
                        counts: String::new(),
                        initial: Mark::None,
                        mark: Mark::None,
                    });
                    rows.len() - 1
                }
            };
            let row = &mut rows[i];
            match c.area {
                ChangeArea::Staged => {
                    row.index = c.code;
                    row.orig = c.orig_path.clone();
                }
                ChangeArea::Unstaged | ChangeArea::Untracked | ChangeArea::Conflicted => {
                    row.worktree = c.code;
                    row.area = c.area;
                }
            }
            if row.counts.is_empty() {
                row.counts = c.line_counts();
            }
        }
        if rows.is_empty() {
            return None;
        }
        for r in &mut rows {
            r.initial = match (r.index != ' ', r.worktree != ' ') {
                (true, false) => Mark::All,
                (true, true) => Mark::Partial,
                _ => Mark::None,
            };
            r.mark = r.initial;
        }
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        Some(StageModal { rows, sel: 0, offset: 0 })
    }

    fn toggle(&mut self) {
        if let Some(r) = self.rows.get_mut(self.sel) {
            r.mark = if r.mark == Mark::All { Mark::None } else { Mark::All };
        }
    }

    /// Checks everything, or unchecks everything when all is checked already.
    fn toggle_all(&mut self) {
        let to = if self.rows.iter().all(|r| r.mark == Mark::All) { Mark::None } else { Mark::All };
        for r in &mut self.rows {
            r.mark = to;
        }
    }

    fn action(&self, ctx: &Ctx, commit: bool) -> Action {
        let mut add = Vec::new();
        let mut unstage = Vec::new();
        for r in self.rows.iter().filter(|r| r.mark != r.initial) {
            match r.mark {
                Mark::All => add.push(arg(&r.path)),
                Mark::None => {
                    unstage.extend(r.orig.as_deref().map(arg));
                    unstage.push(arg(&r.path));
                }
                Mark::Partial => {}
            }
        }
        let staged_after = self.rows.iter().filter(|r| r.mark != Mark::None).count();
        ctx.actions().stage_selection(&add, &unstage, staged_after, commit)
    }

    fn submit(&self, ctx: &Ctx, commit: bool) -> Effect {
        let action = self.action(ctx, commit);
        if let Verdict::Blocked { reason, .. } = &action.verdict {
            return Effect::Message(reason.clone());
        }
        let via = action.key.label(&ctx.theme.g);
        Effect::close_then(Effect::run(action, via))
    }

    pub fn preview(&self, ctx: &Ctx) -> Option<Action> {
        Some(self.action(ctx, false))
    }

    pub fn handle_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        if let Some(s) = nav(key, self.sel, self.rows.len()) {
            self.sel = s;
            return None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Some(Effect::Close),
            KeyCode::Char(' ') => {
                self.toggle();
                self.sel = (self.sel + 1).min(self.rows.len() - 1);
                None
            }
            KeyCode::Char('a') => {
                self.toggle_all();
                None
            }
            KeyCode::Enter => Some(self.submit(ctx, false)),
            KeyCode::Char('c') => Some(self.submit(ctx, true)),
            _ => None,
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let w = area.width.saturating_sub(8).min(100);
        let inner_w = w.saturating_sub(4) as usize;
        let action = self.action(ctx, false);
        let cmd = if action.plan.steps.is_empty() { String::new() } else { action.plan.display(&ctx.snap.workdir) };
        let cmd_lines = wrap_text(&cmd, inner_w);
        // header, rule, rows, blank, command, explain, blank, keys
        let chrome = 7 + cmd_lines.len().max(1);
        let max_rows = (area.height as usize).saturating_sub(chrome + 2).max(3);
        let list_h = self.rows.len().min(max_rows);
        keep_visible(&mut self.offset, self.sel, list_h);

        let checked = self.rows.iter().filter(|r| r.mark == Mark::All).count();
        let mut lines = vec![
            Line::from(vec![
                Span::styled(format!("{checked} of {} checked", self.rows.len()), t.text()),
                Span::styled(format!("  {} green: staged now  {} amber: not staged", g.sep, g.sep), t.dimmed()),
            ]),
            Line::from(Span::styled("─".repeat(inner_w), t.s(t.border))),
        ];
        let counts_w = 12;
        let path_w = inner_w.saturating_sub(str_width(g.box_on) + 5 + counts_w);
        for (i, r) in self.rows.iter().enumerate().skip(self.offset).take(list_h) {
            let mark = match r.mark {
                Mark::All => g.box_on,
                Mark::None => g.box_off,
                Mark::Partial => if g.ascii { "[~]" } else { "◩" },
            };
            let changed = r.mark != r.initial;
            let worktree_color = match r.area {
                ChangeArea::Conflicted => t.err,
                ChangeArea::Untracked => t.dim,
                _ => t.accent,
            };
            lines.push(t.row(
                vec![
                    Span::styled(format!("{mark} "), t.s(if changed { t.accent } else { t.fg })),
                    Span::styled(r.index.to_string(), t.s(t.ok)),
                    Span::styled(format!("{} ", r.worktree), t.s(worktree_color)),
                    Span::styled(pad(&truncate(&r.display(), path_w.saturating_sub(1)), path_w), t.text()),
                    Span::styled(pad_left(&r.counts, counts_w), t.dimmed()),
                ],
                inner_w,
                i == self.sel,
            ));
        }
        if self.rows.len() > list_h {
            lines.push(Line::from(Span::styled(format!("{} {} more — j/k scrolls", g.more, self.rows.len() - list_h), t.dimmed())));
        }
        lines.push(Line::raw(""));
        if cmd.is_empty() {
            lines.push(Line::from(Span::styled("nothing changes yet — space checks a file, a checks all", t.dimmed())));
        } else {
            for l in cmd_lines {
                lines.push(Line::from(Span::styled(l, t.s(t.accent))));
            }
            lines.push(Line::from(Span::styled(truncate(&action.explain, inner_w), t.dimmed())));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            t.key("space"),
            Span::styled(" toggle  ", t.text()),
            t.key("a"),
            Span::styled(" all  ", t.text()),
            t.key(g.enter),
            Span::styled(" stage  ", t.text()),
            t.key("c"),
            Span::styled(" stage + commit  ", t.text()),
            t.key_dim("esc"),
            Span::styled(" close", t.text()),
        ]));
        let rect = centered(area, w, lines.len() as u16 + 2);
        f.render_widget(Clear, rect);
        let block = t.modal(Line::from("Stage files").style(t.s(t.accent)), "", t.accent);
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        f.render_widget(Paragraph::new(lines), inner);
    }
}
