use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding};

use super::{Commit, Glyphs, GraphRow, RefKind};
use crate::helpers::{color, pad, pad_left, relative_age, short_oid, str_width};

/// Palette and glyphs (§9). Dark ground, amber accent, cyan for branch names.
#[derive(Clone, Debug)]
pub struct Theme {
    pub bg: Color,
    pub bg_raised: Color,
    pub sel: Color,
    pub border: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub branch: Color,
    pub ok: Color,
    pub err: Color,
    pub tag: Color,
    /// Background of the dimmed screen behind the error dialog.
    pub shade: Color,
    pub g: Glyphs,
}

impl Theme {
    pub fn new(truecolor: bool, ascii: bool) -> Self {
        let c = |hex| color(hex, truecolor);
        Theme {
            bg: c(0x0e1013),
            bg_raised: c(0x14181d),
            sel: c(0x1f2630),
            border: c(0x2c323c),
            fg: c(0xd4d7dc),
            dim: c(0x8a919c),
            accent: c(0xe0a85a),
            branch: c(0x6fc3df),
            ok: c(0x8fcf7a),
            err: c(0xef7a6e),
            tag: c(0xb79cf0),
            shade: c(0x50555c),
            g: if ascii { Glyphs::ascii() } else { Glyphs::unicode() },
        }
    }

    pub fn s(&self, fg: Color) -> Style {
        Style::new().fg(fg)
    }

    pub fn text(&self) -> Style {
        Style::new().fg(self.fg)
    }

    pub fn dimmed(&self) -> Style {
        Style::new().fg(self.dim)
    }

    pub fn bold(&self, fg: Color) -> Style {
        Style::new().fg(fg).add_modifier(Modifier::BOLD)
    }

    /// Graph lanes rotate through the palette: amber, cyan, violet, green, red.
    pub fn lane(&self, lane: usize) -> Color {
        [self.accent, self.branch, self.tag, self.ok, self.err][lane % 5]
    }

    pub fn ref_color(&self, kind: RefKind) -> Color {
        match kind {
            RefKind::Head => self.accent,
            RefKind::Local => self.branch,
            RefKind::Remote => self.dim,
            RefKind::Tag => self.tag,
        }
    }

    fn border_set(&self) -> Option<border::Set<'static>> {
        self.g.ascii.then_some(border::Set {
            top_left: "+",
            top_right: "+",
            bottom_left: "+",
            bottom_right: "+",
            vertical_left: "|",
            vertical_right: "|",
            horizontal_top: "-",
            horizontal_bottom: "-",
        })
    }

    /// A titled pane: rounded border, title inset top-left, key hint inset top-right.
    /// Focus turns border and title amber.
    pub fn pane<'a>(&self, title: impl Into<Line<'a>>, hint: &str, focused: bool) -> Block<'a> {
        let edge = if focused { self.accent } else { self.border };
        let title_color = if focused { self.accent } else { self.dim };
        let mut title: Line = title.into();
        title.spans.insert(0, Span::raw(" "));
        title.spans.push(Span::raw(" "));
        let title = title.style(Style::new().fg(title_color));
        let mut block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(edge))
            .title(title)
            .padding(Padding::horizontal(1))
            .style(Style::new().bg(self.bg).fg(self.fg));
        if let Some(set) = self.border_set() {
            block = block.border_set(set);
        }
        if !hint.is_empty() {
            block = block.title_top(Line::from(format!(" {hint} ")).style(self.dimmed()).right_aligned());
        }
        block
    }

    /// Popup/modal frame: raised background, border in `edge`.
    pub fn modal<'a>(&self, title: impl Into<Line<'a>>, hint: &str, edge: Color) -> Block<'a> {
        let mut title: Line = title.into();
        title.spans.insert(0, Span::raw(" "));
        title.spans.push(Span::raw(" "));
        let mut block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(edge))
            .title(title)
            .padding(Padding::horizontal(1))
            .style(Style::new().bg(self.bg_raised).fg(self.fg));
        if let Some(set) = self.border_set() {
            block = block.border_set(set);
        }
        if !hint.is_empty() {
            block = block.title_top(Line::from(format!(" {hint} ")).style(self.dimmed()).right_aligned());
        }
        block
    }

    /// Primary key chip: accent background, dark bold text.
    pub fn key<'a>(&self, label: impl Into<String>) -> Span<'a> {
        Span::styled(format!(" {} ", label.into()), Style::new().bg(self.accent).fg(self.bg).add_modifier(Modifier::BOLD))
    }

    /// Red chip for destructive remedies.
    pub fn key_red<'a>(&self, label: impl Into<String>) -> Span<'a> {
        Span::styled(format!(" {} ", label.into()), Style::new().bg(self.err).fg(self.bg).add_modifier(Modifier::BOLD))
    }

    /// Secondary key chip used in hint rows.
    pub fn key_dim<'a>(&self, label: impl Into<String>) -> Span<'a> {
        Span::styled(format!(" {} ", label.into()), Style::new().bg(self.border).fg(self.fg))
    }

    /// `[tab] pane  [j/k] move …`
    pub fn hints<'a>(&self, items: &[(&str, &str)]) -> Line<'a> {
        let mut spans = Vec::new();
        for (i, (key, label)) in items.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("   "));
            }
            let key = if *key == "⏎" { self.g.enter } else { key };
            spans.push(self.key_dim(key.to_string()));
            spans.push(Span::styled(format!(" {label}"), self.text()));
        }
        Line::from(spans)
    }

    /// A list row padded to `width`, on the selection background when selected.
    pub fn row<'a>(&self, mut spans: Vec<Span<'a>>, width: usize, selected: bool) -> Line<'a> {
        let used: usize = spans.iter().map(|s| s.width()).sum();
        if used < width {
            spans.push(Span::raw(" ".repeat(width - used)));
        }
        let line = Line::from(spans);
        if selected { line.style(Style::new().bg(self.sel)) } else { line }
    }

    /// The graph column, each glyph in its lane's color, padded to `width` cells.
    pub fn graph_spans<'a>(&self, row: &GraphRow, width: usize) -> Vec<Span<'a>> {
        let mut spans: Vec<Span> = row
            .cells
            .iter()
            .take(width)
            .map(|(c, lane)| Span::styled(self.g.graph(*c).to_string(), Style::new().fg(self.lane(*lane))))
            .collect();
        if row.cells.len() > width && width > 0 {
            spans.pop();
            spans.push(Span::styled("…", self.dimmed()));
        }
        spans.push(Span::raw(" ".repeat(width.saturating_sub(row.cells.len()))));
        spans
    }

    /// One log row: graph, short hash, ref chips, subject, optional author, age.
    pub fn commit_line<'a>(&self, row: &GraphRow, c: &Commit, graph_w: usize, width: usize, selected: bool, author: bool) -> Line<'a> {
        let mut spans = self.graph_spans(row, graph_w);
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{} ", short_oid(&c.oid)), self.dimmed()));
        for r in &c.refs {
            let text = if r.kind == RefKind::Head && r.name != "HEAD" { format!("HEAD {} {}", self.g.arrow, r.name) } else { r.name.clone() };
            spans.push(self.chip(text, self.ref_color(r.kind)));
            spans.push(Span::raw(" "));
        }
        let right = if author { format!("  {}  {}", pad(&c.author, 10), pad_left(&relative_age(c.time), 4)) } else { format!("  {}", pad_left(&relative_age(c.time), 4)) };
        let used: usize = spans.iter().map(|s| s.width()).sum();
        let room = width.saturating_sub(used + str_width(&right));
        spans.push(Span::styled(pad(&c.subject, room), self.text()));
        spans.push(Span::styled(right, self.dimmed()));
        self.row(spans, width, selected)
    }

    /// Ref chip. Terminals can't draw the mockup's boxed chips inline, so a chip is
    /// colored text on a raised background (brackets in ASCII mode).
    pub fn chip<'a>(&self, text: impl Into<String>, fg: Color) -> Span<'a> {
        let t = text.into();
        if self.g.ascii {
            Span::styled(format!("[{t}]"), Style::new().fg(fg))
        } else {
            Span::styled(format!(" {t} "), Style::new().fg(fg).bg(self.border))
        }
    }
}
