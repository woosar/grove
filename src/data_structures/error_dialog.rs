use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::{Action, Ctx, Effect, ErrorReport, HelpModal, InternalOp, KeyBinding, Modal};
use crate::helpers::{centered, command_line, fmt_duration_ms, pad, str_width, to_os_args, wrap_text};

/// The red modal for a failed command: plain-language headline, what's affected,
/// ranked remedies with exact commands, and the raw output.
pub struct ErrorDialog {
    pub report: ErrorReport,
    sel: usize,
    raw_open: bool,
    scroll: u16,
}

impl ErrorDialog {
    pub fn new(report: ErrorReport) -> Self {
        ErrorDialog { report, sel: 0, raw_open: true, scroll: 0 }
    }

    pub fn preview(&self) -> Option<Action> {
        let r = self.report.remedies.get(self.sel)?;
        Some(Action::new(r.key, "remedy", r.plan.clone()).explain(r.consequence.clone()))
    }

    fn run(&self, idx: usize) -> Effect {
        Effect::close_then(Effect::Remedy { remedy: self.report.remedies[idx].clone(), parent: self.report.entry_id })
    }

    pub fn handle_key(&mut self, key: &KeyEvent, _ctx: &Ctx) -> Option<Effect> {
        let n = self.report.remedies.len();
        match key.code {
            KeyCode::Esc => return Some(Effect::Close),
            KeyCode::Down | KeyCode::Char('j') if n > 0 => self.sel = (self.sel + 1).min(n - 1),
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(5),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(5),
            KeyCode::Tab => self.raw_open = !self.raw_open,
            KeyCode::Enter if n > 0 => {
                // a remedy bound to ⏎ itself only runs via ⏎ when nothing else is highlighted
                return Some(self.run(self.sel));
            }
            KeyCode::Char('?') => {
                return Some(Effect::open(Modal::Help(HelpModal::new("why does git do this?", self.report.error.why()))));
            }
            _ => {
                if let Some(i) = self.report.remedies.iter().position(|r| r.key != KeyBinding::enter() && r.key.matches(key)) {
                    return Some(self.run(i));
                }
                if key.code == KeyCode::Char('y') {
                    let text = format!("$ {}\n{}", command_line(&to_os_args(&self.report.args)), self.report.raw);
                    return Some(Effect::Internal(InternalOp::Copy(text)));
                }
            }
        }
        None
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let r = &self.report;
        let w = area.width.saturating_sub(6).min(140);
        let inner_w = w.saturating_sub(4) as usize;
        let mut lines: Vec<Line> = Vec::new();

        let (pre, name, post) = r.error.headline();
        lines.push(Line::from(vec![
            Span::styled(pre, t.bold(t.fg)),
            Span::styled(name, t.bold(t.branch)),
            Span::styled(post, t.bold(t.fg)),
        ]));
        for l in wrap_text(&r.error.explanation(), inner_w) {
            lines.push(Line::from(Span::styled(l, t.dimmed())));
        }

        if !r.affected.is_empty() {
            lines.push(Line::raw(""));
            let name_w = r.affected.iter().map(|a| str_width(&a.name)).max().unwrap_or(10).min(inner_w / 3) + 2;
            let box_w = inner_w.saturating_sub(2);
            lines.push(Line::from(Span::styled(format!("╭{}╮", "─".repeat(box_w)), t.s(t.border))));
            for a in &r.affected {
                let marker_color = if a.fine { t.dim } else if a.marker == 'U' { t.err } else { t.accent };
                // " M " + name + detail fills the box exactly
                let detail_w = box_w.saturating_sub(name_w + 3);
                let detail_lines = wrap_text(&a.detail, detail_w);
                for (i, d) in detail_lines.iter().enumerate() {
                    let (m, n) = if i == 0 { (a.marker.to_string(), a.name.clone()) } else { (" ".to_string(), String::new()) };
                    let body = format!(" {m} {}{}", pad(&n, name_w), pad(d, detail_w));
                    lines.push(Line::from(vec![
                        Span::styled("│", t.s(t.border)),
                        Span::styled(body[..2.min(body.len())].to_string(), t.s(marker_color)),
                        Span::styled(body.get(2..).unwrap_or("").to_string(), if a.fine { t.dimmed() } else { t.text() }),
                        Span::styled("│", t.s(t.border)),
                    ]));
                }
            }
            lines.push(Line::from(Span::styled(format!("╰{}╯", "─".repeat(box_w)), t.s(t.border))));
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled("Pick a way through", t.dimmed())));
        let label_w = r.remedies.iter().map(|m| str_width(&m.label)).max().unwrap_or(10).min(32) + 3;
        let cmd_w = inner_w.saturating_sub(label_w + 5).max(20);
        for (i, m) in r.remedies.iter().enumerate() {
            let selected = i == self.sel;
            let bg = if selected { Style::new().bg(t.sel) } else { Style::new() };
            let chip = if m.destructive { t.key_red(m.key.label(g)) } else if selected { t.key(m.key.label(g)) } else { t.key_dim(m.key.label(g)) };
            let label_style = if m.destructive { t.s(t.err) } else { t.text() };
            let cmd = m.plan.display(&r.cwd);
            let cmd_color = if m.destructive { t.err } else { t.accent };
            let mut consequence = m.consequence.clone();
            if m.destructive && !m.recoverable {
                consequence = format!("not recoverable: {consequence}");
            }
            // command first, then "— consequence", wrapped under the command column
            let text = if m.plan.is_internal() { consequence.clone() } else { format!("{cmd} {} {consequence}", g.dash) };
            let wrapped = wrap_text(&text, cmd_w);
            let cmd_len = if m.plan.is_internal() { 0 } else { cmd.chars().count() };
            let mut consumed = 0usize;
            for (li, chunk) in wrapped.iter().enumerate() {
                let mut spans = if li == 0 {
                    vec![Span::raw(" "), chip.clone(), Span::raw(" "), Span::styled(pad(&m.label, label_w), label_style)]
                } else {
                    vec![Span::raw(" ".repeat(label_w + 2 + str_width(&m.key.label(g)) + 2))]
                };
                // color the command part accent/red and the consequence dim
                let chunk_len = chunk.chars().count();
                let cmd_part = cmd_len.saturating_sub(consumed).min(chunk_len);
                let (a, b): (String, String) = (chunk.chars().take(cmd_part).collect(), chunk.chars().skip(cmd_part).collect());
                spans.push(Span::styled(a, t.s(cmd_color)));
                let rest_style = if m.destructive { t.s(t.err) } else { t.dimmed() };
                spans.push(Span::styled(b, rest_style));
                consumed += chunk_len + 1;
                let used: usize = spans.iter().map(|s| s.width()).sum();
                spans.push(Span::raw(" ".repeat(inner_w.saturating_sub(used))));
                lines.push(Line::from(spans).style(bg));
            }
            if let Some(warn) = &m.warning {
                let indent = label_w + 2 + str_width(&m.key.label(g)) + 2;
                for l in wrap_text(&format!("{} {warn}", g.warn), cmd_w) {
                    let used = indent + str_width(&l);
                    lines.push(
                        Line::from(vec![
                            Span::raw(" ".repeat(indent)),
                            Span::styled(l, t.s(t.accent)),
                            Span::raw(" ".repeat(inner_w.saturating_sub(used))),
                        ])
                        .style(bg),
                    );
                }
            }
        }

        lines.push(Line::raw(""));
        let toggle = if self.raw_open { g.tri_down } else { g.tri_right };
        lines.push(Line::from(vec![
            Span::styled(format!("{toggle} raw git output "), t.dimmed()),
            Span::styled(format!("(tab {})", if self.raw_open { "hides" } else { "shows" }), t.dimmed().add_modifier(Modifier::ITALIC)),
        ]));
        if self.raw_open {
            // keep git's indentation (file lists are tab-indented) on every wrapped line
            let raw_lines = r.raw.lines().flat_map(|l| {
                let l = l.replace('\t', "        ");
                let indent = l.len() - l.trim_start().len();
                wrap_text(l.trim_start(), inner_w.saturating_sub(2 + indent))
                    .into_iter()
                    .map(move |w| format!("{}{w}", " ".repeat(indent)))
            });
            for l in raw_lines {
                let used = str_width(&l) + 2;
                lines.push(Line::from(vec![
                    Span::styled(format!("  {l}"), t.dimmed()),
                    Span::raw(" ".repeat(inner_w.saturating_sub(used))),
                ]).style(Style::new().bg(t.bg)));
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            t.key(g.enter),
            Span::styled(" run selected   ", t.text()),
            t.key_dim("?"),
            Span::styled(" why does git do this   ", t.text()),
            t.key_dim("y"),
            Span::styled(" copy error   ", t.text()),
            t.key_dim("esc"),
            Span::styled(" dismiss", t.text()),
        ]));

        let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
        let rect = centered(area, w, h);
        f.render_widget(Clear, rect);
        let exit = match r.exit {
            Some(code) => format!("exit {code}"),
            None => "did not run".to_string(),
        };
        let title = format!(
            "{} {} {} {exit} {} {}",
            g.cross,
            command_line(&to_os_args(&r.args)),
            g.sep,
            g.sep,
            fmt_duration_ms(r.duration_ms)
        );
        let hint = r.entry_id.map(|id| format!("journal #{id}")).unwrap_or_default();
        let block = t.modal(Line::from(title).style(t.s(t.err)), &hint, t.err);
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let max_scroll = (lines.len() as u16).saturating_sub(inner.height);
        f.render_widget(Paragraph::new(lines).scroll((self.scroll.min(max_scroll), 0)), inner);
    }
}
