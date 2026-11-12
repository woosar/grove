use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// One line of process output with the colors it was written in: ANSI SGR sequences
/// become styles, everything else that could disturb the screen is dropped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyledLine {
    segments: Vec<(Style, String)>,
}

/// The 16 basic ANSI colors, normal (0–7) then bright (8–15).
fn basic(n: u16) -> Color {
    const COLORS: [Color; 16] = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    COLORS[(n as usize) % 16]
}

/// `38;5;n` / `38;2;r;g;b` (and the 48 forms) starting after the 38/48.
fn extended(params: &mut impl Iterator<Item = u16>) -> Option<Color> {
    match params.next()? {
        5 => params.next().map(|n| Color::Indexed(n.min(255) as u8)),
        2 => {
            let (r, g, b) = (params.next()?, params.next()?, params.next()?);
            Some(Color::Rgb(r.min(255) as u8, g.min(255) as u8, b.min(255) as u8))
        }
        _ => None,
    }
}

/// Applies an SGR parameter list (`1;31` of `ESC[1;31m`) to `style`.
fn apply_sgr(params: &str, style: &mut Style) {
    let mut it = params.split([';', ':']).map(|p| p.parse::<u16>().unwrap_or(0));
    // `ESC[m` is a reset
    let first = it.next().unwrap_or(0);
    let mut next = Some(first);
    while let Some(p) = next {
        match p {
            0 => *style = Style::default(),
            1 => *style = style.add_modifier(Modifier::BOLD),
            2 => *style = style.add_modifier(Modifier::DIM),
            3 => *style = style.add_modifier(Modifier::ITALIC),
            4 => *style = style.add_modifier(Modifier::UNDERLINED),
            7 => *style = style.add_modifier(Modifier::REVERSED),
            9 => *style = style.add_modifier(Modifier::CROSSED_OUT),
            22 => *style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => *style = style.remove_modifier(Modifier::ITALIC),
            24 => *style = style.remove_modifier(Modifier::UNDERLINED),
            27 => *style = style.remove_modifier(Modifier::REVERSED),
            29 => *style = style.remove_modifier(Modifier::CROSSED_OUT),
            30..=37 => style.fg = Some(basic(p - 30)),
            90..=97 => style.fg = Some(basic(p - 90 + 8)),
            40..=47 => style.bg = Some(basic(p - 40)),
            100..=107 => style.bg = Some(basic(p - 100 + 8)),
            38 => style.fg = extended(&mut it).or(style.fg),
            48 => style.bg = extended(&mut it).or(style.bg),
            39 => style.fg = None,
            49 => style.bg = None,
            _ => {}
        }
        next = it.next();
    }
}

impl StyledLine {
    /// Text in a single style (step headers, grove's own messages).
    pub fn plain(text: impl Into<String>, style: Style) -> StyledLine {
        StyledLine { segments: vec![(style, text.into())] }
    }

    /// One line of raw process output: lossily decoded, without the line ending and with
    /// only the last of any `\r` rewrites. `state` is the stream's current SGR style; it
    /// carries over from line to line, as it does in a terminal.
    pub fn parse(bytes: &[u8], state: &mut Style) -> StyledLine {
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_end_matches(['\r', '\n']);
        let mut line = StyledLine::default();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => match chars.next() {
                    // CSI: parameters up to a final byte; only SGR (`m`) means something here
                    Some('[') => {
                        let mut params = String::new();
                        for c in chars.by_ref() {
                            if ('\x40'..='\x7e').contains(&c) {
                                if c == 'm' {
                                    apply_sgr(&params, state);
                                }
                                break;
                            }
                            params.push(c);
                        }
                    }
                    // OSC: up to BEL or ESC \
                    Some(']') => {
                        while let Some(c) = chars.next() {
                            if c == '\x07' {
                                break;
                            }
                            if c == '\x1b' {
                                chars.next();
                                break;
                            }
                        }
                    }
                    _ => {}
                },
                // a carriage return starts the line over (progress counters)
                '\r' => line.segments.clear(),
                '\t' => line.push(*state, "    "),
                c if c.is_control() => {}
                c => line.push_char(*state, c),
            }
        }
        line
    }

    fn push(&mut self, style: Style, text: &str) {
        match self.segments.last_mut() {
            Some((s, t)) if *s == style => t.push_str(text),
            _ => self.segments.push((style, text.to_string())),
        }
    }

    fn push_char(&mut self, style: Style, c: char) {
        match self.segments.last_mut() {
            Some((s, t)) if *s == style => t.push(c),
            _ => self.segments.push((style, c.to_string())),
        }
    }

    /// The text without styles.
    #[cfg(test)]
    pub fn text(&self) -> String {
        self.segments.iter().map(|(_, t)| t.as_str()).collect()
    }

    pub fn width(&self) -> usize {
        self.segments.iter().map(|(_, t)| crate::helpers::str_width(t)).sum()
    }

    /// Rows of at most `width` cells, broken after a space where possible. Leading
    /// indentation stays; each piece is drawn as `base` patched with its own style.
    pub fn wrap(&self, width: usize, base: Style) -> Vec<Line<'static>> {
        let width = width.max(8);
        if self.width() <= width {
            let spans = self.segments.iter().map(|(s, t)| Span::styled(t.clone(), base.patch(*s))).collect::<Vec<_>>();
            return vec![Line::from(spans)];
        }
        // (char, cells, segment), then cut into rows
        let cells: Vec<(char, usize, usize)> = self
            .segments
            .iter()
            .enumerate()
            .flat_map(|(i, (_, t))| t.chars().map(move |c| (c, c.width().unwrap_or(0), i)))
            .collect();
        let mut rows: Vec<&[(char, usize, usize)]> = Vec::new();
        let mut start = 0;
        while start < cells.len() {
            let mut used = 0;
            let mut end = start;
            while end < cells.len() && used + cells[end].1 <= width {
                used += cells[end].1;
                end += 1;
            }
            if end < cells.len() {
                // break after the last space that leaves something on this row
                if let Some(space) = cells[start..end].iter().rposition(|c| c.0 == ' ').filter(|&p| p > 0) {
                    end = start + space + 1;
                }
                // a character wider than the row still has to go somewhere
                end = end.max(start + 1);
            }
            rows.push(&cells[start..end]);
            start = end;
        }
        rows.into_iter()
            .map(|row| {
                let mut spans: Vec<Span<'static>> = Vec::new();
                let mut current: Option<(usize, String)> = None;
                for &(c, _, seg) in row {
                    match &mut current {
                        Some((s, text)) if *s == seg => text.push(c),
                        _ => {
                            if let Some((s, text)) = current.take() {
                                spans.push(Span::styled(text, base.patch(self.segments[s].0)));
                            }
                            current = Some((seg, c.to_string()));
                        }
                    }
                }
                if let Some((s, text)) = current {
                    spans.push(Span::styled(text, base.patch(self.segments[s].0)));
                }
                Line::from(spans)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> StyledLine {
        StyledLine::parse(bytes, &mut Style::default())
    }

    #[test]
    fn text_is_safe_to_draw() {
        assert_eq!(parse(b"plain\r\n").text(), "plain");
        assert_eq!(parse(b"\x1b[31;1merror\x1b[0m CS1002\n").text(), "error CS1002");
        assert_eq!(parse(b"\x1b]0;title\x07ok\n").text(), "ok");
        assert_eq!(parse(b"10%\r50%\rdone\n").text(), "done");
        assert_eq!(parse(b"a\tb\x08\n").text(), "a    b");
        assert_eq!(parse(b"caf\xe9\n").text(), "caf\u{fffd}");
        assert_eq!(parse(b"\x1b[2Kcleared\n").text(), "cleared");
    }

    #[test]
    fn sgr_becomes_styles() {
        let line = parse(b"\x1b[31;1merror\x1b[0m CS1002");
        assert_eq!(
            line.segments,
            vec![(Style::new().fg(Color::Red).add_modifier(Modifier::BOLD), "error".into()), (Style::default(), " CS1002".into())]
        );
        let line = parse(b"\x1b[38;5;208ma\x1b[38;2;1;2;3mb\x1b[92mc\x1b[39md");
        let fgs: Vec<_> = line.segments.iter().map(|(s, _)| s.fg).collect();
        assert_eq!(fgs, vec![Some(Color::Indexed(208)), Some(Color::Rgb(1, 2, 3)), Some(Color::LightGreen), None]);
    }

    #[test]
    fn style_carries_over_to_the_next_line() {
        let mut state = Style::default();
        StyledLine::parse(b"\x1b[33mwarning starts\n", &mut state);
        let next = StyledLine::parse(b"and goes on\x1b[m\n", &mut state);
        assert_eq!(next.segments, vec![(Style::new().fg(Color::Yellow), "and goes on".into())]);
        assert_eq!(state, Style::default());
    }

    #[test]
    fn wrapping_keeps_styles_and_indentation() {
        let line = parse(b"    \x1b[31merror\x1b[0m: something went wrong here");
        let rows = line.wrap(16, Style::default());
        let texts: Vec<String> = rows.iter().map(|l| l.to_string()).collect();
        assert_eq!(texts, vec!["    error: ", "something went ", "wrong here"]);
        assert_eq!(rows[0].spans[1].style.fg, Some(Color::Red));
        assert_eq!(rows[0].spans[1].content, "error");
        // a word longer than the row is cut
        let rows = parse(b"abcdefghijklmnopqrstuvwxyz").wrap(10, Style::default());
        assert_eq!(rows.iter().map(|l| l.to_string()).collect::<Vec<_>>(), vec!["abcdefghij", "klmnopqrst", "uvwxyz"]);
    }

    #[test]
    fn base_style_fills_in_what_the_output_left_open() {
        let base = Style::new().fg(Color::White).bg(Color::Black);
        let rows = parse(b"\x1b[32mok\x1b[0m plain").wrap(80, base);
        assert_eq!(rows[0].spans[0].style, Style::new().fg(Color::Green).bg(Color::Black));
        assert_eq!(rows[0].spans[1].style, base);
    }
}
