use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Action, ActionPopup, Commit, CommitDetail, Ctx, Effect, GraphRow, Job, KeyBinding, LaneAllocator, LogFilter, Modal, Plan, PromptKind, PromptModal, Snapshot};
use crate::helpers::{keep_visible, local_date_long, nav, pad, pad_left, relative_age_long, short_oid, str_width, wrap_text};

/// Screen 2: the full log with graph, filter, commit detail and file preview.
pub struct LogScreen {
    commits: Vec<Commit>,
    graph: Vec<GraphRow>,
    lanes: LaneAllocator,
    total: usize,
    sel: usize,
    offset: usize,
    file_sel: usize,
    files_focused: bool,
    filter: LogFilter,
    all_branches: bool,
    generation: u64,
    requested: Option<usize>,
    exhausted: bool,
    stale: bool,
    /// Oid to reselect after a reload.
    keep_oid: Option<String>,
    detail: Option<CommitDetail>,
    detail_requested: Option<String>,
    anchor: Option<Rect>,
    page: usize,
}

impl Default for LogScreen {
    fn default() -> Self {
        LogScreen {
            commits: Vec::new(),
            graph: Vec::new(),
            lanes: LaneAllocator::new(),
            total: 0,
            sel: 0,
            offset: 0,
            file_sel: 0,
            files_focused: false,
            filter: LogFilter::default(),
            all_branches: true,
            generation: 0,
            requested: None,
            exhausted: false,
            stale: true,
            keep_oid: None,
            detail: None,
            detail_requested: None,
            anchor: None,
            page: 20,
        }
    }
}

impl LogScreen {
    /// Reload from scratch on the next tick (repo changed, filter changed).
    pub fn invalidate(&mut self) {
        self.stale = true;
        self.keep_oid = self.commits.get(self.sel).map(|c| c.oid.clone());
    }

    pub fn filter_input(&self) -> String {
        self.filter.to_input()
    }

    pub fn set_filter(&mut self, input: &str) {
        self.filter = LogFilter::parse(input);
        self.sel = 0;
        self.invalidate();
        self.keep_oid = None;
    }

    /// Background work this screen needs right now: log pages and commit detail.
    pub fn jobs(&mut self, snap: &Snapshot) -> Vec<Job> {
        let mut jobs = Vec::new();
        let cwd = snap.workdir.clone();
        if self.stale {
            self.stale = false;
            self.generation += 1;
            self.commits.clear();
            self.graph.clear();
            self.lanes = LaneAllocator::new();
            self.exhausted = false;
            self.requested = None;
            self.detail = None;
            self.detail_requested = None;
        }
        if snap.status.oid.is_none() {
            self.exhausted = true;
            return jobs;
        }
        let near_end = self.sel + self.page * 2 >= self.commits.len();
        if !self.exhausted && self.requested.is_none() && (self.commits.is_empty() || near_end) {
            let skip = self.commits.len();
            self.requested = Some(skip);
            jobs.push(Job::LogPage {
                cwd: cwd.clone(),
                rev_args: self.filter.rev_args(self.all_branches),
                skip,
                generation: self.generation,
                remotes: snap.remotes.clone(),
            });
        }
        if let Some(c) = self.commits.get(self.sel)
            && self.detail.as_ref().is_none_or(|d| d.oid != c.oid)
            && self.detail_requested.as_deref() != Some(&c.oid)
        {
            self.detail_requested = Some(c.oid.clone());
            jobs.push(Job::Detail { cwd, oid: c.oid.clone(), remotes: snap.remotes.clone() });
        }
        jobs
    }

    pub fn on_page(&mut self, generation: u64, skip: usize, commits: Vec<Commit>, total: Option<usize>) {
        if generation != self.generation || skip != self.commits.len() {
            return;
        }
        self.requested = None;
        if commits.len() < 200 {
            self.exhausted = true;
        }
        for c in &commits {
            self.graph.push(self.lanes.push(&c.oid, &c.parents));
        }
        self.commits.extend(commits);
        if let Some(t) = total {
            self.total = t;
        }
        if let Some(oid) = self.keep_oid.take() {
            if let Some(i) = self.commits.iter().position(|c| c.oid == oid) {
                self.sel = i;
            } else if !self.exhausted {
                self.keep_oid = Some(oid);
            }
        }
        self.sel = self.sel.min(self.commits.len().saturating_sub(1));
    }

    pub fn on_detail(&mut self, detail: CommitDetail) {
        if self.commits.get(self.sel).is_some_and(|c| c.oid == detail.oid) {
            self.file_sel = self.file_sel.min(detail.files.len().saturating_sub(1));
            self.detail = Some(detail);
        }
    }

    fn selected(&self) -> Option<&Commit> {
        self.commits.get(self.sel)
    }

    fn file_diff(&self) -> Option<Action> {
        let d = self.detail.as_ref()?;
        let file = d.files.get(self.file_sel)?;
        Some(
            Action::new(
                KeyBinding::enter(),
                "diff",
                Plan::git(&["show", "--format=", "--diff-merges=first-parent", short_oid(&d.oid), "--", &file.path]).with_output(),
            )
            .explain(format!("shows what {} changed in {}", short_oid(&d.oid), file.path)),
        )
    }

    pub fn preview(&self, ctx: &Ctx) -> Option<Action> {
        if self.files_focused {
            return self.file_diff();
        }
        ctx.actions().commit_actions(self.selected()?).into_iter().find(|a| !a.verdict.is_blocked())
    }

    pub fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("j/k", "move"),
            ("⏎", "diff"),
            ("c", "checkout"),
            ("p", "cherry-pick"),
            ("R", "revert"),
            ("x", "reset to here"),
            ("t", "tag"),
            ("/", "filter"),
            ("b", "branches"),
            ("tab", "files"),
            ("?", "explain"),
        ]
    }

    pub fn handle_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        let g = &ctx.theme.g;
        if self.files_focused {
            let n = self.detail.as_ref().map_or(0, |d| d.files.len());
            if let Some(s) = nav(key, self.file_sel, n) {
                self.file_sel = s;
                return Some(Effect::Nothing);
            }
            match key.code {
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc => {
                    self.files_focused = false;
                    return Some(Effect::Nothing);
                }
                KeyCode::Enter => return self.file_diff().map(|a| Effect::run(a, g.enter)),
                _ => {}
            }
        }
        if let Some(s) = nav(key, self.sel, self.commits.len()) {
            if self.sel != s {
                self.file_sel = 0;
            }
            self.sel = s;
            return Some(Effect::Nothing);
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.files_focused = self.detail.as_ref().is_some_and(|d| !d.files.is_empty());
                return Some(Effect::Nothing);
            }
            KeyCode::Char('/') => {
                return Some(Effect::open(Modal::Prompt(PromptModal::new(PromptKind::LogFilter, self.filter.to_input()))));
            }
            KeyCode::Char('b') => {
                self.all_branches = !self.all_branches;
                self.invalidate();
                return Some(Effect::Nothing);
            }
            _ => {}
        }
        let c = self.selected()?;
        let actions = ctx.actions().commit_actions(c);
        match key.code {
            KeyCode::Char(' ') => Some(Effect::open(Modal::Popup(ActionPopup::new(
                short_oid(&c.oid).to_string(),
                c.subject.clone(),
                actions,
                self.anchor,
            )))),
            _ => {
                let a = actions.into_iter().find(|a| a.key.matches(key))?;
                let via = a.key.label(g);
                Some(Effect::run(a, via))
            }
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let (list, side) = if area.width >= 120 {
            let [l, r] = Layout::horizontal([Constraint::Min(0), Constraint::Percentage(36)]).spacing(1).areas(area);
            (l, r)
        } else {
            let [l, r] = Layout::vertical([Constraint::Percentage(55), Constraint::Min(0)]).areas(area);
            (l, r)
        };
        self.draw_list(f, list, ctx);
        let detail_h = self.detail.as_ref().map_or(10, |d| {
            let body = wrap_text(&d.body, side.width.saturating_sub(4) as usize).len().min(8);
            (10 + body + if d.body.is_empty() { 0 } else { 1 }) as u16
        });
        let [commit, files] = Layout::vertical([Constraint::Length(detail_h.min(side.height / 2 + 4)), Constraint::Min(3)]).areas(side);
        self.draw_commit(f, commit, ctx);
        self.draw_files(f, files, ctx);
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let focused = !self.files_focused;
        let scope = if self.all_branches { "--all --topo-order" } else { "HEAD --topo-order" };
        let count = if self.commits.is_empty() && self.requested.is_some() {
            "loading…".to_string()
        } else {
            format!("{} of {}", (self.sel + 1).min(self.commits.len()), self.total.max(self.commits.len()))
        };
        let block = t.pane(format!("Log {} {scope}", g.sep), &count, focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let [filter_row, rows_area, footer] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(inner);

        let filter_text = if self.filter.is_empty() { "path:any  author:any  since:any".to_string() } else { self.filter.to_input() };
        let right = format!("b branches: {} {}", if self.all_branches { "all" } else { "current" }, g.tri_down);
        let room = w.saturating_sub(str_width(&right) + 3);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("/ ", t.s(t.accent)),
                Span::styled(pad(&filter_text, room), if self.filter.is_empty() { t.dimmed() } else { t.text() }),
                Span::styled(format!(" {right}"), t.dimmed()),
            ])),
            filter_row,
        );

        let h = rows_area.height as usize;
        self.page = h.max(1);
        keep_visible(&mut self.offset, self.sel, h);
        let graph_w = self.graph.iter().skip(self.offset).take(h).map(|r| r.width()).max().unwrap_or(1).min(w / 3).max(1);
        let rows: Vec<Line> = self
            .commits
            .iter()
            .zip(&self.graph)
            .enumerate()
            .skip(self.offset)
            .take(h)
            .map(|(i, (c, row))| t.commit_line(row, c, graph_w, w, i == self.sel, w >= 70))
            .collect();
        if rows.is_empty() && self.exhausted {
            f.render_widget(Paragraph::new(Span::styled("no commits match", t.dimmed())), rows_area);
        } else {
            f.render_widget(Paragraph::new(rows), rows_area);
        }
        self.anchor = (!self.commits.is_empty())
            .then(|| Rect::new(rows_area.x, rows_area.y + (self.sel - self.offset) as u16, rows_area.width, 1));

        let below = self.total.max(self.commits.len()).saturating_sub(self.offset + h);
        let footer_text = if below > 0 { format!("{} {below} older commits {} G jump to root", g.more, g.sep) } else { String::new() };
        f.render_widget(Paragraph::new(Span::styled(footer_text, t.dimmed())), footer);
    }

    fn draw_commit(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let block = t.pane("Commit", "y copy hash", false);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let Some(d) = &self.detail else {
            let msg = if self.commits.is_empty() { "" } else { "loading…" };
            f.render_widget(Paragraph::new(Span::styled(msg, t.dimmed())), inner);
            return;
        };
        let w = inner.width as usize;
        let field = |name: &str, value: Vec<Span<'static>>| {
            let mut spans = vec![Span::styled(pad(name, 9), t.dimmed())];
            spans.extend(value);
            Line::from(spans)
        };
        let mut lines = vec![
            field("hash", vec![Span::styled(d.oid.clone(), t.s(t.accent))]),
            field("author", vec![Span::styled(format!("{} <{}>", d.author, d.email), t.text())]),
            field(
                "date",
                vec![Span::styled(local_date_long(d.time), t.text()), Span::styled(format!("  ({})", relative_age_long(d.time)), t.dimmed())],
            ),
        ];
        let mut refs = Vec::new();
        for r in &d.refs {
            refs.push(t.chip(r.name.clone(), t.ref_color(r.kind)));
            refs.push(Span::raw(" "));
        }
        if refs.is_empty() {
            refs.push(Span::styled("—", t.dimmed()));
        }
        lines.push(field("refs", refs));
        let parents: Vec<String> = d.parents.iter().map(|p| short_oid(p).to_string()).collect();
        lines.push(field(
            if parents.len() > 1 { "parents" } else { "parent" },
            vec![Span::styled(if parents.is_empty() { "— (root)".into() } else { parents.join(" ") }, t.text())],
        ));
        let main = ctx.snap.main.clone();
        let local_on: Vec<&String> = d.contained_in.iter().filter(|b| !ctx.snap.remotes.iter().any(|r| b.starts_with(&format!("{r}/")))).collect();
        let mut on = vec![Span::styled(
            match local_on.len() {
                0 => "no local branch".to_string(),
                1 => format!("{} only", local_on[0]),
                n if n <= 3 => local_on.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
                n => format!("{n} branches"),
            },
            t.text(),
        )];
        if let Some(m) = main
            && !d.contained_in.contains(&m)
        {
            on.push(Span::styled(format!("  {} not in {m}", g.sep), t.dimmed()));
        }
        lines.push(field("on", on));
        lines.push(Line::from(Span::styled("─".repeat(w), t.s(t.border))));
        lines.push(Line::from(Span::styled(d.subject.clone(), t.bold(t.fg))));
        if !d.body.is_empty() {
            lines.push(Line::raw(""));
            for l in wrap_text(&d.body, w) {
                lines.push(Line::from(Span::styled(l, t.dimmed())));
            }
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_files(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let n = self.detail.as_ref().map_or(0, |d| d.files.len());
        let block = t.pane(format!("Files {} {n}", t.g.sep), &format!("{} diff", t.g.enter), self.files_focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let Some(d) = &self.detail else { return };
        let w = inner.width as usize;
        let mut lines = Vec::new();
        let list_h = (inner.height as usize / 2).max(3);
        let start = self.file_sel.saturating_sub(list_h.saturating_sub(1));
        for (i, file) in d.files.iter().enumerate().skip(start).take(list_h) {
            let color = match file.status {
                'A' => t.ok,
                'D' => t.err,
                'R' | 'C' => t.tag,
                _ => t.accent,
            };
            let (a, r) = (
                file.added.map_or("bin".to_string(), |a| format!("+{a}")),
                file.removed.map_or(String::new(), |r| format!("−{r}")),
            );
            let path_w = w.saturating_sub(2 + 12);
            lines.push(t.row(
                vec![
                    Span::styled(format!("{} ", file.status), t.s(color)),
                    Span::styled(pad(&file.path, path_w), t.text()),
                    Span::styled(pad_left(&a, 6), t.s(t.ok)),
                    Span::styled(pad_left(&r, 6), t.s(t.err)),
                ],
                w,
                i == self.file_sel,
            ));
        }
        if let Some(file) = d.files.get(self.file_sel) {
            lines.push(Line::raw(""));
            for l in d.first_hunk(&file.path) {
                let style = if l.starts_with('+') {
                    t.s(t.ok)
                } else if l.starts_with('-') {
                    t.s(t.err)
                } else if l.starts_with("@@") {
                    t.dimmed().add_modifier(Modifier::BOLD)
                } else {
                    t.text()
                };
                let (sign, rest) = if l.starts_with("@@") { (String::new(), l.clone()) } else { (l.chars().take(1).collect(), l.chars().skip(1).collect()) };
                lines.push(Line::from(vec![Span::styled(pad(&sign, 2), style), Span::styled(rest.replace('\t', "    "), style)]));
            }
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}
