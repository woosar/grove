/// Every non-ASCII symbol grove draws, with an ASCII fallback set (`--ascii`).
#[derive(Clone, Debug)]
pub struct Glyphs {
    pub ascii: bool,
    pub dot: &'static str,
    pub check: &'static str,
    pub cross: &'static str,
    pub warn: &'static str,
    pub diamond: &'static str,
    pub pointer: &'static str,
    pub star: &'static str,
    pub branch: &'static str,
    pub up: &'static str,
    pub down: &'static str,
    pub prompt: &'static str,
    pub enter: &'static str,
    pub more: &'static str,
    pub arrow: &'static str,
    pub dash: &'static str,
    pub sep: &'static str,
    pub spinner: [&'static str; 4],
    pub tri_down: &'static str,
    pub tri_right: &'static str,
    pub box_on: &'static str,
    pub box_off: &'static str,
    pub radio_on: &'static str,
    pub radio_off: &'static str,
}

impl Glyphs {
    pub fn unicode() -> Self {
        Glyphs {
            ascii: false,
            dot: "●",
            check: "✓",
            cross: "✗",
            warn: "⚠",
            diamond: "◆",
            pointer: "▸",
            star: "*",
            branch: "⎇",
            up: "↑",
            down: "↓",
            prompt: "›",
            enter: "⏎",
            more: "⋮",
            arrow: "→",
            dash: "—",
            sep: "·",
            spinner: ["⠋", "⠙", "⠸", "⠴"],
            tri_down: "▾",
            tri_right: "▸",
            box_on: "☑",
            box_off: "☐",
            radio_on: "◉",
            radio_off: "○",
        }
    }

    pub fn ascii() -> Self {
        Glyphs {
            ascii: true,
            dot: "*",
            check: "+",
            cross: "x",
            warn: "!",
            diamond: "#",
            pointer: ">",
            star: "*",
            branch: "br",
            up: "^",
            down: "v",
            prompt: ">",
            enter: "RET",
            more: ":",
            arrow: "->",
            dash: "-",
            sep: "-",
            spinner: ["|", "/", "-", "\\"],
            tri_down: "v",
            tri_right: ">",
            box_on: "[x]",
            box_off: "[ ]",
            radio_on: "(*)",
            radio_off: "( )",
        }
    }

    /// Maps a graph glyph from the lane allocator to its ASCII stand-in when needed.
    pub fn graph(&self, c: char) -> char {
        if !self.ascii {
            return c;
        }
        match c {
            '●' => '*',
            '│' => '|',
            '─' => '-',
            '╮' | '╰' => '\\',
            '╯' | '╭' => '/',
            '├' | '┤' => '+',
            other => other,
        }
    }
}
