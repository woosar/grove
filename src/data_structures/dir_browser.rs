use std::path::{Path, PathBuf};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{TextInput, Theme};
use crate::helpers::{expand_user_path, home_tilde, keep_visible, pad, step, truncate};

/// One subdirectory of the folder being browsed.
struct Folder {
    name: String,
    path: PathBuf,
    /// Has a `.git` of its own (repository or worktree root).
    repo: bool,
}

/// What the browser asks its owner to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Browsed {
    /// Save the repository containing this folder.
    Pick(PathBuf),
    Cancel,
}

/// A folder browser: lists the subfolders of one folder, typing filters them (fuzzy),
/// and typing a path ending in `/` jumps there. Repositories are marked; `⏎` on one
/// picks it, on any other folder goes into it.
pub struct DirBrowser {
    /// `None` lists the drives (Windows, above a drive root).
    dir: Option<PathBuf>,
    folders: Vec<Folder>,
    filter: TextInput,
    /// Indices into `folders` matching the filter, best first.
    shown: Vec<usize>,
    sel: usize,
    offset: usize,
    /// Why the folder couldn't be read.
    error: Option<String>,
    matcher: Matcher,
}

impl DirBrowser {
    pub fn new(start: &Path) -> Self {
        let mut b = DirBrowser {
            dir: None,
            folders: Vec::new(),
            filter: TextInput::default(),
            shown: Vec::new(),
            sel: 0,
            offset: 0,
            error: None,
            matcher: Matcher::new(Config::DEFAULT),
        };
        b.go(Some(start.to_path_buf()), None);
        b
    }

    /// The folder being browsed, if any.
    #[cfg(test)]
    fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Lists `dir` (drives for `None`), selecting the entry named `select`.
    fn go(&mut self, dir: Option<PathBuf>, select: Option<&str>) {
        self.error = None;
        self.folders = match &dir {
            Some(d) => match std::fs::read_dir(d) {
                Ok(rd) => {
                    let mut out: Vec<Folder> = rd
                        .filter_map(Result::ok)
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                        .map(|e| {
                            let path = e.path();
                            Folder { name: e.file_name().to_string_lossy().into_owned(), repo: path.join(".git").exists(), path }
                        })
                        .collect();
                    out.sort_by_key(|f| f.name.to_lowercase());
                    out
                }
                Err(e) => {
                    self.error = Some(format!("can't read {}: {e}", home_tilde(d)));
                    Vec::new()
                }
            },
            None => drives(),
        };
        self.dir = dir;
        self.filter = TextInput::default();
        self.refilter();
        if let Some(name) = select
            && let Some(i) = self.shown.iter().position(|&i| self.folders[i].name.eq_ignore_ascii_case(name))
        {
            self.sel = i;
        }
    }

    fn refilter(&mut self) {
        let query = self.filter.value.clone();
        // hidden folders only when asked for
        let visible = |f: &Folder| !f.name.starts_with('.') || query.starts_with('.');
        if query.is_empty() {
            self.shown = (0..self.folders.len()).filter(|&i| visible(&self.folders[i])).collect();
        } else {
            let pattern = Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart);
            let mut buf = Vec::new();
            let mut scored: Vec<(u32, usize)> = Vec::new();
            for (i, f) in self.folders.iter().enumerate().filter(|(_, f)| visible(f)) {
                if let Some(score) = pattern.score(Utf32Str::new(&f.name, &mut buf), &mut self.matcher) {
                    scored.push((score, i));
                }
            }
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.shown = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.sel = 0;
        self.offset = 0;
    }

    fn selected(&self) -> Option<&Folder> {
        self.shown.get(self.sel).map(|&i| &self.folders[i])
    }

    fn up(&mut self) {
        let Some(dir) = self.dir.clone() else { return };
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        match dir.parent() {
            Some(p) => self.go(Some(p.to_path_buf()), name.as_deref()),
            // above a drive root: the drive list (only Windows has one)
            None if cfg!(windows) => {
                let drive = dir.to_string_lossy().trim_end_matches(['\\', '/']).to_string();
                self.go(None, Some(&format!("{drive}\\")));
            }
            None => {}
        }
    }

    fn enter_selected(&mut self) {
        if let Some(path) = self.selected().map(|f| f.path.clone()) {
            self.go(Some(path), None);
        }
    }

    /// A typed separator turns the filter into a path: `~/`, `../`, `src/`, `D:/`.
    fn jump_to_typed(&mut self) -> bool {
        let typed = self.filter.value.clone();
        let base = self.dir.clone().unwrap_or_default();
        let target = expand_user_path(&typed, &base);
        if target.is_dir() {
            let target = std::fs::canonicalize(&target).map(|p| clean_verbatim(&p)).unwrap_or(target);
            self.go(Some(target), None);
            true
        } else {
            false
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> Option<Browsed> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.shown.len();
        match key.code {
            KeyCode::Esc if !self.filter.value.is_empty() => {
                self.filter = TextInput::default();
                self.refilter();
            }
            KeyCode::Esc => return Some(Browsed::Cancel),
            KeyCode::Down => self.sel = step(self.sel, 1, n),
            KeyCode::Up => self.sel = step(self.sel, -1, n),
            KeyCode::Char('n' | 'j') if ctrl => self.sel = step(self.sel, 1, n),
            KeyCode::Char('p' | 'k') if ctrl => self.sel = step(self.sel, -1, n),
            KeyCode::PageDown => self.sel = step(self.sel, 10, n),
            KeyCode::PageUp => self.sel = step(self.sel, -10, n),
            KeyCode::Char('s') if ctrl => return self.dir.clone().map(Browsed::Pick),
            KeyCode::Right | KeyCode::Tab => self.enter_selected(),
            KeyCode::Left => self.up(),
            KeyCode::Backspace if self.filter.value.is_empty() => self.up(),
            KeyCode::Enter => match self.selected().map(|f| (f.repo, f.path.clone())) {
                Some((true, path)) => return Some(Browsed::Pick(path)),
                Some(_) => self.enter_selected(),
                None if !self.filter.value.is_empty() && !self.jump_to_typed() => {
                    self.error = Some(format!("no folder matches {}", self.filter.value));
                }
                None => {}
            },
            KeyCode::Char(c @ ('/' | '\\')) if !ctrl => {
                // a separator right after a name jumps there; otherwise keep typing
                if self.filter.value.is_empty() || !self.jump_to_typed_with(c) {
                    self.filter.handle(key);
                    self.refilter();
                }
            }
            _ => {
                if self.filter.handle(key) {
                    self.error = None;
                    self.refilter();
                }
            }
        }
        None
    }

    fn jump_to_typed_with(&mut self, sep: char) -> bool {
        let before = self.filter.value.clone();
        self.filter.set(format!("{before}{sep}"));
        // `src/` with a single fuzzy hit: go into that one
        if !self.jump_to_typed() {
            self.filter.set(before.clone());
            self.refilter();
            if self.shown.len() == 1 {
                self.enter_selected();
                return true;
            }
            return false;
        }
        true
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let g = &t.g;
        let here = match &self.dir {
            Some(d) => home_tilde(d),
            None => "drives".to_string(),
        };
        let block = t.pane(format!("Add repository {} {here}", g.sep), "⏎ open/pick", true);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        let [input, list] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);
        let mut line = self.filter.line(t.text(), true);
        line.spans.insert(0, Span::styled(format!("{} ", g.prompt), t.s(t.accent)));
        let status = match &self.error {
            Some(e) => Span::styled(format!("  {e}"), t.s(t.err)),
            None => Span::styled(format!("  {} of {} folders", self.shown.len(), self.folders.len()), t.dimmed()),
        };
        line.spans.push(status);
        f.render_widget(Paragraph::new(vec![line, Line::from(Span::styled("─".repeat(w), t.s(t.border)))]), input);

        let h = list.height as usize;
        keep_visible(&mut self.offset, self.sel, h);
        let mut lines = Vec::new();
        for (row, &i) in self.shown.iter().enumerate().skip(self.offset).take(h) {
            let folder = &self.folders[i];
            let selected = row == self.sel;
            let mark = if selected { g.pointer } else { " " };
            let (tag, name_style) = if folder.repo { (format!("{} repository", g.branch), t.bold(t.branch)) } else { (String::new(), t.text()) };
            let name_w = w.saturating_sub(4 + tag.chars().count() + 2);
            lines.push(t.row(
                vec![
                    Span::styled(format!("{mark} "), t.s(t.accent)),
                    Span::styled(pad(&truncate(&format!("{}/", folder.name.trim_end_matches(['\\', '/'])), name_w), name_w + 2), name_style),
                    Span::styled(tag, t.s(t.tag)),
                ],
                w,
                selected,
            ));
        }
        if lines.is_empty() && self.error.is_none() {
            let msg = if self.filter.value.is_empty() { "no folders here — ← goes up" } else { "no folder matches — esc clears the filter" };
            lines.push(Line::from(Span::styled(msg, t.dimmed())));
        }
        f.render_widget(Paragraph::new(lines), list);
    }
}

/// `\\?\C:\x` (what canonicalize gives on Windows) back to `C:\x`.
fn clean_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

/// Drive roots that exist (Windows); empty elsewhere.
fn drives() -> Vec<Folder> {
    if !cfg!(windows) {
        return Vec::new();
    }
    (b'A'..=b'Z')
        .map(|c| format!("{}:\\", c as char))
        .filter(|d| Path::new(d).is_dir())
        .map(|d| Folder { path: PathBuf::from(&d), name: d, repo: false })
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    use super::*;

    fn press(b: &mut DirBrowser, code: KeyCode) -> Option<Browsed> {
        b.handle_key(&KeyEvent { code, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE })
    }

    fn typing(b: &mut DirBrowser, text: &str) {
        for c in text.chars() {
            press(b, KeyCode::Char(c));
        }
    }

    #[test]
    fn browses_filters_and_picks_a_repository() {
        let root = std::env::temp_dir().join(format!("grove-browse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["work/alpha/.git", "work/beta-service/.git", "work/notes", "work/.hidden"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let mut b = DirBrowser::new(&root);
        assert_eq!(b.shown.len(), 1);
        // ⏎ on a plain folder goes in; hidden folders stay out of the list
        press(&mut b, KeyCode::Enter);
        assert!(b.dir().unwrap().ends_with("work"));
        assert_eq!(b.shown.len(), 3);
        // typing filters fuzzily
        typing(&mut b, "bsv");
        assert_eq!(b.shown.len(), 1);
        assert_eq!(press(&mut b, KeyCode::Enter), Some(Browsed::Pick(root.join("work").join("beta-service"))));
        // esc clears the filter first, then cancels
        press(&mut b, KeyCode::Esc);
        assert_eq!(b.shown.len(), 3);
        // ← goes up and lands on the folder it came from
        press(&mut b, KeyCode::Left);
        assert!(b.dir().unwrap().ends_with(root.file_name().unwrap()));
        // a typed path with a separator jumps: `work/` then into notes with a unique hit
        typing(&mut b, "work/not/");
        assert!(b.dir().unwrap().ends_with("notes"), "{:?}", b.dir());
        typing(&mut b, "../");
        assert!(b.dir().unwrap().ends_with("work"), "{:?}", b.dir());
        // ctrl-s picks the folder being browsed
        let ctrl_s = KeyEvent { code: KeyCode::Char('s'), modifiers: KeyModifiers::CONTROL, kind: KeyEventKind::Press, state: KeyEventState::NONE };
        assert!(matches!(b.handle_key(&ctrl_s), Some(Browsed::Pick(_))));
        assert_eq!(press(&mut b, KeyCode::Esc), Some(Browsed::Cancel));
        for (w, h) in [(120, 30), (20, 5)] {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            term.draw(|f| b.draw(f, f.area(), &Theme::new(false, false))).unwrap();
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
