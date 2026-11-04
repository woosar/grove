use std::io::Write;
use std::path::PathBuf;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Action, Ctx, Effect, GroveError, InternalOp, Journal, JournalEntry, JournalFilter, KeyBinding, Modal, Pager, Plan, PromptKind, PromptModal};
use crate::helpers::{day_label, fmt_duration_ms, home_tilde, keep_visible, local_hms, nav, pad, pad_left, short_oid, wrap_text};

/// How many recent days the sessions list shows before "older…".
const DAYS_SHOWN: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Sessions,
    Table,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Day(usize),
    Older,
    AllDays,
    ThisRepo,
    AllRepos,
    ShowGrove,
    ShowRaw,
    ShowReadonly,
}

/// Screen 5: persisted command history with filters, detail, rerun, undo and export.
pub struct JournalScreen {
    focus: Focus,
    side_sel: usize,
    sel: usize,
    offset: usize,
    entries: Vec<JournalEntry>,
    days: Vec<(String, i64)>,
    day: Side,
    all_repos: bool,
    filter: JournalFilter,
    pub needs_reload: bool,
    db_path: Option<PathBuf>,
}

impl Default for JournalScreen {
    fn default() -> Self {
        JournalScreen {
            focus: Focus::Table,
            side_sel: 0,
            sel: 0,
            offset: 0,
            entries: Vec::new(),
            days: Vec::new(),
            day: Side::Day(0),
            all_repos: false,
            filter: JournalFilter::default(),
            needs_reload: true,
            db_path: None,
        }
    }
}

impl JournalScreen {
    fn side_items(&self) -> Vec<Side> {
        let mut items: Vec<Side> = (0..self.days.len().min(DAYS_SHOWN)).map(Side::Day).collect();
        if self.days.len() > DAYS_SHOWN {
            items.push(Side::Older);
        }
        items.extend([Side::AllDays, Side::ThisRepo, Side::AllRepos, Side::ShowGrove, Side::ShowRaw, Side::ShowReadonly]);
        items
    }

    pub fn reload(&mut self, journal: &Journal, repo: &str) {
        let scope = (!self.all_repos).then(|| repo.to_string());
        self.db_path = journal.path.clone();
        self.days = journal.days(scope.as_deref(), self.filter.show_readonly);
        self.filter.repo = scope;
        self.filter.day = None;
        self.filter.before_day = None;
        match self.day {
            Side::Day(i) => match self.days.get(i) {
                Some((d, _)) => self.filter.day = Some(d.clone()),
                None => self.day = Side::AllDays,
            },
            Side::Older => self.filter.before_day = self.days.get(DAYS_SHOWN - 1).map(|(d, _)| d.clone()),
            _ => {}
        }
        self.entries = journal.query(&self.filter);
        self.sel = self.sel.min(self.entries.len().saturating_sub(1));
        self.needs_reload = false;
    }

    pub fn set_search(&mut self, search: String) {
        self.filter.search = search;
        self.needs_reload = true;
    }

    fn selected(&self) -> Option<&JournalEntry> {
        self.entries.get(self.sel)
    }

    fn rerun(e: &JournalEntry) -> Action {
        let mut plan = Plan::git(&e.argv).in_dir(PathBuf::from(&e.worktree));
        plan.show_output = !e.mutating;
        Action::new(KeyBinding::ch('r'), "rerun", plan).explain("same command, same worktree")
    }

    pub fn preview(&self, _ctx: &Ctx) -> Option<Action> {
        if self.focus == Focus::Sessions {
            return None;
        }
        self.selected().map(Self::rerun)
    }

    pub fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("j/k", "move"),
            ("tab", "sessions"),
            ("⏎", "details"),
            ("r", "rerun"),
            ("e", "edit"),
            ("u", "undo (reflog)"),
            ("/", "search"),
            ("f", "failed only"),
            ("E", "export .sh"),
            ("?", "explain"),
        ]
    }

    fn apply_side(&mut self, item: Side) {
        match item {
            Side::Day(_) | Side::Older | Side::AllDays => self.day = item,
            Side::ThisRepo => self.all_repos = false,
            Side::AllRepos => self.all_repos = true,
            Side::ShowGrove => self.filter.show_grove = !self.filter.show_grove,
            Side::ShowRaw => self.filter.show_raw = !self.filter.show_raw,
            Side::ShowReadonly => self.filter.show_readonly = !self.filter.show_readonly,
        }
        self.sel = 0;
        self.needs_reload = true;
    }

    /// Writes the visible entries, oldest first, as a shell script.
    fn export(&self) -> Result<PathBuf, String> {
        let dir = self
            .db_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.join("exports"))
            .ok_or("journal has no data directory")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let path = dir.join(format!("grove-journal-{stamp}.sh"));
        let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let mut out = String::from("#!/bin/sh\n# exported from the grove journal\nset -e\n");
        let mut cwd = String::new();
        for e in self.entries.iter().rev() {
            if e.worktree != cwd {
                cwd = e.worktree.clone();
                out.push_str(&format!("\ncd {}\n", crate::helpers::shell_quote(&cwd)));
            }
            let status = if e.ok() { String::new() } else { format!("  # failed: exit {}", e.exit_code.unwrap_or(-1)) };
            out.push_str(&format!("{}{status}\n", e.command()));
        }
        file.write_all(out.as_bytes()).map_err(|e| e.to_string())?;
        Ok(path)
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> Option<Effect> {
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = if self.focus == Focus::Table { Focus::Sessions } else { Focus::Table };
                return Some(Effect::Nothing);
            }
            KeyCode::Char('/') => {
                return Some(Effect::open(Modal::Prompt(PromptModal::new(PromptKind::JournalSearch, self.filter.search.clone()))));
            }
            KeyCode::Char('f') => {
                self.filter.failed_only = !self.filter.failed_only;
                self.needs_reload = true;
                return Some(Effect::Nothing);
            }
            KeyCode::Char('E') => {
                return Some(Effect::Message(match self.export() {
                    Ok(p) => format!("exported {} entries to {}", self.entries.len(), home_tilde(&p)),
                    Err(e) => format!("export failed: {e}"),
                }));
            }
            _ => {}
        }
        if self.focus == Focus::Sessions {
            let items = self.side_items();
            if let Some(s) = nav(key, self.side_sel, items.len()) {
                self.side_sel = s;
                return Some(Effect::Nothing);
            }
            if matches!(key.code, KeyCode::Enter | KeyCode::Char(' '))
                && let Some(item) = items.get(self.side_sel)
            {
                self.apply_side(*item);
                return Some(Effect::Nothing);
            }
            return None;
        }
        if let Some(s) = nav(key, self.sel, self.entries.len()) {
            self.sel = s;
            return Some(Effect::Nothing);
        }
        let e = self.selected()?.clone();
        match key.code {
            KeyCode::Enter => {
                let mut text = format!("$ {}\n# in {}\n", e.command(), e.worktree);
                if let Some(out) = e.stdout.as_deref().filter(|s| !s.is_empty()) {
                    text.push_str(&format!("\n# stdout\n{out}"));
                }
                if let Some(err) = e.stderr.as_deref().filter(|s| !s.is_empty()) {
                    text.push_str(&format!("\n# stderr\n{err}"));
                }
                Some(Effect::open(Modal::Pager(Pager::new(format!("Entry #{}", e.id), text))))
            }
            KeyCode::Char('r') => Some(Effect::run(Self::rerun(&e), "r")),
            KeyCode::Char('e') => Some(Effect::Internal(InternalOp::EditRaw(
                e.command().strip_prefix("git ").unwrap_or_default().to_string(),
            ))),
            KeyCode::Char('u') => Some(Effect::UndoEntry(Box::new(e))),
            KeyCode::Char('y') => Some(Effect::Internal(InternalOp::Copy(e.command()))),
            _ => None,
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let wide = area.width >= 120;
        let (side, table, detail) = if wide {
            let [s, t, d] = Layout::horizontal([Constraint::Length(28), Constraint::Min(0), Constraint::Percentage(36)])
                .spacing(1)
                .areas(area);
            (Some(s), t, d)
        } else {
            let [t, d] = Layout::vertical([Constraint::Percentage(55), Constraint::Min(0)]).areas(area);
            (None, t, d)
        };
        if let Some(s) = side {
            self.draw_sessions(f, s, ctx);
        } else if self.focus == Focus::Sessions {
            self.focus = Focus::Table;
        }
        self.draw_table(f, table, ctx);
        self.draw_detail(f, detail, ctx);
    }

    fn draw_sessions(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let focused = self.focus == Focus::Sessions;
        let block = t.pane("Sessions", "", focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let items = self.side_items();
        self.side_sel = self.side_sel.min(items.len().saturating_sub(1));
        let mut lines = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let cursor = focused && i == self.side_sel;
            let line = match item {
                Side::Day(d) => {
                    let (day, n) = &self.days[*d];
                    let active = self.day == *item;
                    t.row(
                        vec![
                            Span::styled(pad(&day_label(day), w.saturating_sub(6)), if active { t.s(t.accent) } else { t.text() }),
                            Span::styled(pad_left(&n.to_string(), 6), t.dimmed()),
                        ],
                        w,
                        active || cursor,
                    )
                }
                Side::Older => t.row(vec![Span::styled("older…", if self.day == Side::Older { t.s(t.accent) } else { t.text() })], w, cursor || self.day == Side::Older),
                Side::AllDays => {
                    let active = self.day == Side::AllDays;
                    t.row(vec![Span::styled("all days", if active { t.s(t.accent) } else { t.text() })], w, cursor || active)
                }
                Side::ThisRepo | Side::AllRepos => {
                    if *item == Side::ThisRepo {
                        lines.push(Line::from(Span::styled("─".repeat(w), t.s(t.border))));
                        lines.push(Line::from(Span::styled("scope", t.dimmed())));
                    }
                    let on = (*item == Side::AllRepos) == self.all_repos;
                    let label = if *item == Side::ThisRepo { "this repo" } else { "all repos" };
                    t.row(
                        vec![
                            Span::styled(format!("{} ", if on { g.radio_on } else { g.radio_off }), if on { t.s(t.accent) } else { t.dimmed() }),
                            Span::styled(label, t.text()),
                        ],
                        w,
                        cursor,
                    )
                }
                Side::ShowGrove | Side::ShowRaw | Side::ShowReadonly => {
                    if *item == Side::ShowGrove {
                        lines.push(Line::from(Span::styled("─".repeat(w), t.s(t.border))));
                        lines.push(Line::from(Span::styled("show", t.dimmed())));
                    }
                    let (on, label) = match item {
                        Side::ShowGrove => (self.filter.show_grove, "grove actions"),
                        Side::ShowRaw => (self.filter.show_raw, "raw : commands"),
                        _ => (self.filter.show_readonly, "read-only commands"),
                    };
                    t.row(
                        vec![
                            Span::styled(format!("{} ", if on { g.box_on } else { g.box_off }), if on { t.s(t.accent) } else { t.dimmed() }),
                            Span::styled(label, if on { t.text() } else { t.dimmed() }),
                        ],
                        w,
                        cursor,
                    )
                }
            };
            lines.push(line);
        }
        let footer = ["persisted in journal.db —", "survives restarts"];
        let free = (inner.height as usize).saturating_sub(lines.len() + footer.len());
        lines.extend(std::iter::repeat_n(Line::raw(""), free));
        lines.extend(footer.iter().map(|l| Line::from(Span::styled(*l, t.dimmed()))));
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_table(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let focused = self.focus == Focus::Table;
        let day = match self.day {
            Side::Day(i) => self.days.get(i).map(|(d, _)| day_label(d).to_lowercase()).unwrap_or_else(|| "all days".into()),
            Side::Older => "older".into(),
            _ => "all days".into(),
        };
        let mut hint = String::from("/ search");
        if !self.filter.search.is_empty() {
            hint = format!("/ \"{}\"", self.filter.search);
        }
        if self.filter.failed_only {
            hint.push_str(&format!(" {} failed only", g.sep));
        }
        let block = t.pane(format!("Journal {} {day}", g.sep), &hint, focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let cmd_w = w.saturating_sub(9 + 2 + 7 + 6 + 8);
        let mut lines = vec![
            Line::from(vec![
                Span::styled(pad("time", 11), t.dimmed()),
                Span::styled(pad("via", 7), t.dimmed()),
                Span::styled(pad("command", cmd_w), t.dimmed()),
                Span::styled(pad("exit", 6), t.dimmed()),
                Span::styled(pad_left("took", 7), t.dimmed()),
            ]),
            Line::from(Span::styled("─".repeat(w), t.s(t.border))),
        ];
        let body_h = inner.height.saturating_sub(2) as usize;
        keep_visible(&mut self.offset, self.sel, body_h);
        if self.entries.is_empty() {
            lines.push(Line::from(Span::styled("no entries match", t.dimmed())));
        }
        for (i, e) in self.entries.iter().enumerate().skip(self.offset).take(body_h) {
            let (mark, color) = if e.ok() { (g.check, t.ok) } else { (g.cross, t.err) };
            let exit = e.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "—".into());
            lines.push(t.row(
                vec![
                    Span::styled(pad(&local_hms(e.started_at), 9), t.dimmed()),
                    Span::styled(format!("{mark} "), t.s(color)),
                    Span::styled(pad(&e.via, 7), t.s(t.tag)),
                    Span::styled(pad(&e.command(), cmd_w), if e.mutating { t.text() } else { t.dimmed() }),
                    Span::styled(pad(&exit, 6), t.s(color)),
                    Span::styled(pad_left(&fmt_duration_ms(e.duration_ms), 7), t.dimmed()),
                ],
                w,
                i == self.sel,
            ));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_detail(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let Some(e) = self.selected() else {
            f.render_widget(t.pane("Entry", "", false), area);
            return;
        };
        let block = t.pane(format!("Entry #{}", e.id), &local_hms(e.started_at), false);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let (mark, color) = if e.ok() { (g.check, t.ok) } else { (g.cross, t.err) };
        let mut lines = vec![Line::from(vec![Span::styled(format!("{mark} "), t.s(color)), Span::styled(e.command(), t.s(t.accent))])];
        let field = |name: &str, spans: Vec<Span<'static>>| {
            let mut v = vec![Span::styled(pad(name, 9), t.dimmed())];
            v.extend(spans);
            Line::from(v)
        };
        lines.push(field(
            "where",
            vec![
                Span::styled(home_tilde(std::path::Path::new(&e.worktree)), t.text()),
                Span::styled(format!("  {}", e.branch.clone().unwrap_or_else(|| "(detached)".into())), t.s(t.branch)),
            ],
        ));
        let via = match e.via.as_str() {
            ":" => ": raw prompt".to_string(),
            "remedy" => format!("error-dialog remedy for #{}", e.parent_id.unwrap_or(0)),
            other => format!("key {other}"),
        };
        lines.push(field("via", vec![Span::styled(via, t.text())]));
        let mut result = vec![Span::styled(
            match e.exit_code {
                Some(c) => format!("exit {c}"),
                None => "did not run".into(),
            },
            t.s(color),
        )];
        if !e.ok() {
            let kind = GroveError::classify(&e.argv, e.exit_code, &format!("{}\n{}", e.stderr.as_deref().unwrap_or(""), e.stdout.as_deref().unwrap_or(""))).kind_label();
            result.push(Span::styled(format!(" {} {kind}", g.sep), t.s(t.err)));
        }
        result.push(Span::styled(format!(" {} {}", g.sep, fmt_duration_ms(e.duration_ms)), t.dimmed()));
        lines.push(field("result", result));
        if e.mutating {
            let head = match (&e.head_before, &e.head_after) {
                (Some(a), Some(b)) if a == b => format!("{} {} {}  (unchanged)", short_oid(a), g.arrow, short_oid(b)),
                (Some(a), Some(b)) => format!("{} {} {}", short_oid(a), g.arrow, short_oid(b)),
                _ => "—".into(),
            };
            lines.push(field("HEAD", vec![Span::styled(head, t.dimmed())]));
        }
        lines.push(Line::from(Span::styled("─".repeat(w), t.s(t.border))));
        let (label, output) = match (e.stderr.as_deref().filter(|s| !s.trim().is_empty()), e.stdout.as_deref()) {
            (Some(err), _) => ("stderr", err),
            (None, Some(out)) => ("stdout", out),
            _ => ("output", ""),
        };
        lines.push(Line::from(Span::styled(label, t.dimmed())));
        let out_lines: Vec<&str> = output.lines().collect();
        let max_out = (inner.height as usize).saturating_sub(lines.len() + 12).max(3);
        for l in out_lines.iter().take(max_out) {
            for wl in wrap_text(&l.replace('\t', "    "), w.saturating_sub(2)) {
                lines.push(t.row(vec![Span::styled(format!(" {wl}"), t.text())], w, false).style(ratatui::style::Style::new().bg(t.bg_raised)));
            }
        }
        if out_lines.len() > max_out {
            lines.push(Line::from(Span::styled(format!(" {} {} more lines — {} for all", g.more, out_lines.len() - max_out, g.enter), t.dimmed())));
        }
        if out_lines.is_empty() {
            lines.push(Line::from(Span::styled(" (empty)", t.dimmed())));
        }
        if let Some(note) = &e.note {
            lines.push(Line::raw(""));
            for l in note.lines() {
                let text = l.strip_prefix("grove: ").unwrap_or(l);
                for (i, wl) in wrap_text(text, w.saturating_sub(7)).into_iter().enumerate() {
                    lines.push(Line::from(vec![
                        Span::styled(if i == 0 { "grove: " } else { "       " }, t.s(t.accent)),
                        Span::styled(wl, t.dimmed()),
                    ]));
                }
            }
        }
        lines.push(Line::from(Span::styled("─".repeat(w), t.s(t.border))));
        let undo = match e.undo(ctx.snap.status.oid.as_deref()) {
            Ok((plan, explain)) => format!("{} — {explain}", plan.display(&ctx.snap.workdir)),
            Err(reason) => reason,
        };
        for (key, label, text) in [
            ("r", "rerun", "same command, same worktree".to_string()),
            ("e", "edit", "open in : prompt before running".to_string()),
            ("u", "undo", undo),
            ("y", "copy", "command to clipboard".to_string()),
        ] {
            let chip = if key == "r" { t.key(key) } else { t.key_dim(key) };
            lines.push(Line::from(vec![
                chip,
                Span::styled(format!(" {}", pad(label, 8)), t.text()),
                Span::styled(crate::helpers::truncate(&text, w.saturating_sub(12)), t.dimmed()),
            ]));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}
