use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::style::Style;
use ratatui::{DefaultTerminal, Frame};

use super::{Browsed, DirBrowser, Journal, SavedRepo, Theme};
use crate::helpers::{home_tilde, keep_visible, nav, pad, relative_age_long, same_path, truncate, wrap_text};

/// What the picker does besides listing.
enum Mode {
    List,
    /// Browsing for a repository to save.
    Add(DirBrowser),
    /// Asking before forgetting this saved repository.
    Forget(i64),
}

/// How the picker ends.
#[derive(Debug, PartialEq, Eq)]
enum Picked {
    Open(PathBuf),
    Quit,
}

/// One row: a saved repository, or the current directory's when it isn't saved.
struct Entry {
    path: PathBuf,
    saved: Option<SavedRepo>,
}

/// The start screen: pick one of the saved base repositories (or the one grove was
/// started in), add or forget one. Grove then works in the picked repository however
/// it was started.
pub struct RepoPicker {
    entries: Vec<Entry>,
    /// Base repository of the directory grove was started in, if any.
    here: Option<PathBuf>,
    /// Where browsing for a repository starts.
    cwd: PathBuf,
    sel: usize,
    offset: usize,
    mode: Mode,
    /// Last outcome, and whether it is an error.
    notice: Option<(String, bool)>,
}

impl RepoPicker {
    pub fn new(journal: &Mutex<Journal>, cwd: &Path, error: Option<String>) -> Self {
        let here = SavedRepo::locate(cwd).ok();
        let mut picker = RepoPicker {
            entries: Vec::new(),
            here: here.clone(),
            cwd: cwd.to_path_buf(),
            sel: 0,
            offset: 0,
            mode: Mode::List,
            notice: error.map(|e| (e, true)),
        };
        picker.reload(journal, None);
        // the most recently opened repo comes first; the one grove started in wins
        if let Some(h) = &here {
            picker.select(h);
        }
        picker
    }

    /// Draws and reads keys until a repository is picked (`Some`) or the user quits.
    pub fn run(mut self, terminal: &mut DefaultTerminal, theme: &Theme, journal: &Mutex<Journal>) -> Result<Option<PathBuf>> {
        loop {
            terminal.draw(|f| self.draw(f, theme))?;
            let Event::Key(key) = event::read()? else { continue };
            if key.kind == KeyEventKind::Release {
                continue;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(None);
            }
            match self.handle_key(&key, journal) {
                Some(Picked::Open(path)) => return Ok(Some(path)),
                Some(Picked::Quit) => return Ok(None),
                None => {}
            }
        }
    }

    fn reload(&mut self, journal: &Mutex<Journal>, select: Option<&Path>) {
        let repos = journal.lock().map(|j| j.repos()).unwrap_or_default();
        self.entries.clear();
        if let Some(h) = self.here.as_ref().filter(|h| !repos.iter().any(|r| same_path(&r.path, h))) {
            self.entries.push(Entry { path: h.clone(), saved: None });
        }
        self.entries.extend(repos.into_iter().map(|r| Entry { path: r.path.clone(), saved: Some(r) }));
        self.sel = self.sel.min(self.entries.len().saturating_sub(1));
        if let Some(path) = select {
            self.select(path);
        }
    }

    fn select(&mut self, path: &Path) {
        if let Some(i) = self.entries.iter().position(|e| same_path(&e.path, path)) {
            self.sel = i;
        }
    }

    fn handle_key(&mut self, key: &KeyEvent, journal: &Mutex<Journal>) -> Option<Picked> {
        match &mut self.mode {
            Mode::Add(browser) => {
                match browser.handle_key(key) {
                    Some(Browsed::Cancel) => self.mode = Mode::List,
                    Some(Browsed::Pick(path)) => self.add(&path, journal),
                    None => self.notice = None,
                }
                None
            }
            Mode::Forget(id) => {
                let id = *id;
                self.mode = Mode::List;
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter) {
                    let name = self.entries.get(self.sel).and_then(|e| e.saved.as_ref()).map(SavedRepo::name).unwrap_or_default();
                    let result = journal.lock().map_err(|_| "the database is unavailable".to_string()).and_then(|j| j.forget_repo(id).map_err(|e| e.to_string()));
                    self.notice = Some(match result {
                        Ok(()) => (format!("forgot {name} — nothing on disk changed"), false),
                        Err(e) => (format!("couldn't forget {name}: {e}"), true),
                    });
                    self.reload(journal, None);
                }
                None
            }
            Mode::List => {
                if let Some(s) = nav(key, self.sel, self.entries.len()) {
                    self.sel = s;
                    return None;
                }
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => Some(Picked::Quit),
                    KeyCode::Char('a') => {
                        // start where the last browse ended, else where grove was started
                        let start = self.entries.get(self.sel).and_then(|e| e.path.parent()).filter(|p| p.is_dir()).unwrap_or(&self.cwd).to_path_buf();
                        self.mode = Mode::Add(DirBrowser::new(&start));
                        self.notice = None;
                        None
                    }
                    KeyCode::Char('d') | KeyCode::Delete => {
                        match self.entries.get(self.sel) {
                            Some(Entry { saved: Some(r), .. }) => self.mode = Mode::Forget(r.id),
                            Some(_) => self.notice = Some(("not saved — it is listed because grove was started inside it".into(), false)),
                            None => {}
                        }
                        None
                    }
                    KeyCode::Enter => {
                        let entry = self.entries.get(self.sel)?;
                        if !entry.path.is_dir() {
                            self.notice = Some((format!("{} is gone — d forgets it", home_tilde(&entry.path)), true));
                            return None;
                        }
                        Some(Picked::Open(entry.path.clone()))
                    }
                    _ => None,
                }
            }
        }
    }

    /// Saves the repository containing `path`, staying in add mode when it isn't one.
    fn add(&mut self, path: &Path, journal: &Mutex<Journal>) {
        let base = match SavedRepo::locate(path) {
            Ok(b) => b,
            Err(e) => {
                self.notice = Some((e, true));
                return;
            }
        };
        let result = journal.lock().map_err(|_| "the database is unavailable".to_string()).and_then(|j| j.add_repo(&base).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                self.mode = Mode::List;
                self.reload(journal, Some(&base));
                let via = if same_path(&base, path) { String::new() } else { " (its main worktree)".to_string() };
                self.notice = Some((format!("saved {}{via}", home_tilde(&base)), false));
            }
            Err(e) => self.notice = Some((format!("couldn't save it: {e}"), true)),
        }
    }

    fn draw(&mut self, f: &mut Frame, t: &Theme) {
        let area = f.area();
        f.render_widget(Block::new().style(Style::new().bg(t.bg).fg(t.fg)), area);
        let [header, _, body, footer] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(3)]).areas(area);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("grove", t.bold(t.accent)),
                Span::styled("  pick a repository", t.dimmed()),
            ])),
            header,
        );
        match &mut self.mode {
            Mode::Add(browser) => browser.draw(f, body, t),
            _ => self.draw_list(f, body, t),
        }
        self.draw_footer(f, footer, t);
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let g = &t.g;
        let block = t.pane(format!("Repositories {} {}", g.sep, self.entries.len()), "a add", matches!(self.mode, Mode::List));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        if self.entries.is_empty() {
            let text = "No saved repositories yet. Press a and browse to one — any folder inside a repository or one of its worktrees works; grove saves the main worktree.";
            let lines: Vec<Line> = wrap_text(text, w).into_iter().map(|l| Line::from(Span::styled(l, t.dimmed()))).collect();
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }
        let name_w = self.entries.iter().map(|e| entry_name(e).chars().count()).max().unwrap_or(8).clamp(8, 28) + 2;
        let when_w = 16;
        let path_w = w.saturating_sub(2 + name_w + when_w);
        let h = inner.height as usize;
        keep_visible(&mut self.offset, self.sel, h);
        let mut lines = Vec::new();
        for (i, e) in self.entries.iter().enumerate().skip(self.offset).take(h) {
            let selected = i == self.sel;
            let (when, when_style) = match &e.saved {
                None => ("started here".to_string(), t.s(t.tag)),
                Some(_) if !e.path.is_dir() => ("missing".to_string(), t.s(t.err)),
                Some(SavedRepo { last_opened: Some(ms), .. }) => (relative_age_long(ms / 1000), t.dimmed()),
                Some(_) => ("never opened".to_string(), t.dimmed()),
            };
            lines.push(t.row(
                vec![
                    Span::styled(format!("{} ", if selected { g.pointer } else { " " }), t.s(t.accent)),
                    Span::styled(pad(&truncate(&entry_name(e), name_w - 2), name_w), t.bold(t.branch)),
                    Span::styled(pad(&truncate(&home_tilde(&e.path), path_w.saturating_sub(1)), path_w), t.text()),
                    Span::styled(pad(&when, when_w), when_style),
                ],
                w,
                selected,
            ));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let g = &t.g;
        let w = area.width as usize;
        let [rule, line, hints] = Layout::vertical([Constraint::Length(1); 3]).areas(area);
        f.render_widget(Paragraph::new(Span::styled("─".repeat(w), t.s(t.border))), rule);
        let notice = |fallback: Line<'static>| match &self.notice {
            Some((text, err)) => Line::from(Span::styled(truncate(text, w), t.s(if *err { t.err } else { t.ok }))),
            None => fallback,
        };
        let (status, keys): (Line, Vec<(&str, &str)>) = match &self.mode {
            Mode::Add(_) => {
                let help = Line::from(Span::styled(
                    format!("type to filter {} a path ending in / jumps there (~/, ../, D:/) {} {} marks repositories", g.sep, g.sep, g.branch),
                    t.dimmed(),
                ));
                (
                    notice(help),
                    vec![("↑/↓", "move"), ("⏎", "pick repo / open folder"), ("→ ←", "in / up"), ("ctrl-s", "pick this folder"), ("esc", "back")],
                )
            }
            Mode::Forget(_) => {
                let name = self.entries.get(self.sel).map(entry_name).unwrap_or_default();
                let l = Line::from(vec![
                    Span::styled(format!("Forget {name}? ", ), t.bold(t.fg)),
                    Span::styled("Only grove's list changes; the repository on disk stays.", t.dimmed()),
                ]);
                (l, vec![("y", "forget"), ("esc", "keep")])
            }
            Mode::List => {
                let fallback = match self.entries.get(self.sel) {
                    Some(e) => Line::from(vec![
                        Span::styled(format!("{} ", g.prompt), t.s(t.accent)),
                        Span::styled(format!("cd {}", home_tilde(&e.path)), t.s(t.accent)),
                        Span::styled(format!(" {} grove runs git there; the shell you started it in stays where it is", g.dash), t.dimmed()),
                    ]),
                    None => Line::from(Span::styled(format!("{} ", g.prompt), t.s(t.accent))),
                };
                (notice(fallback), vec![("j/k", "move"), ("⏎", "open"), ("a", "add"), ("d", "forget"), ("q", "quit")])
            }
        };
        f.render_widget(Paragraph::new(status), line);
        f.render_widget(Paragraph::new(t.hints(&keys)), hints);
    }
}

fn entry_name(e: &Entry) -> String {
    match &e.saved {
        Some(r) => r.name(),
        None => e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use ratatui::crossterm::event::KeyEventState;

    use super::*;

    fn press(p: &mut RepoPicker, j: &Mutex<Journal>, code: KeyCode) -> Option<Picked> {
        p.handle_key(&KeyEvent { code, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE }, j)
    }

    fn typing(p: &mut RepoPicker, j: &Mutex<Journal>, text: &str) {
        for c in text.chars() {
            press(p, j, KeyCode::Char(c));
        }
    }

    #[test]
    fn adds_a_repo_by_any_inner_path_and_opens_its_base() {
        let root = std::env::temp_dir().join(format!("grove-picker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        let out = Command::new("git").args(["init", "-q"]).current_dir(&repo).output().unwrap();
        assert!(out.status.success());
        let j = Mutex::new(Journal::open_in_memory().unwrap());
        // started outside any repository: nothing listed
        let mut p = RepoPicker::new(&j, &root, None);
        assert!(p.entries.is_empty());

        // ctrl-s on a folder outside any repository: refused, still browsing
        press(&mut p, &j, KeyCode::Char('a'));
        p.handle_key(&KeyEvent { code: KeyCode::Char('s'), modifiers: KeyModifiers::CONTROL, kind: KeyEventKind::Press, state: KeyEventState::NONE }, &j);
        assert!(matches!(p.mode, Mode::Add(_)), "a non-repo keeps the browser open");
        assert!(p.notice.as_ref().is_some_and(|n| n.1));

        // browse into a folder deep inside the repo and pick it: the repo is saved
        typing(&mut p, &j, "repo/src/deep/");
        p.handle_key(&KeyEvent { code: KeyCode::Char('s'), modifiers: KeyModifiers::CONTROL, kind: KeyEventKind::Press, state: KeyEventState::NONE }, &j);
        assert!(matches!(p.mode, Mode::List));
        assert_eq!(p.entries.len(), 1);
        assert!(same_path(&p.entries[0].path, &repo), "{:?}", p.entries[0].path);
        assert_eq!(press(&mut p, &j, KeyCode::Enter), Some(Picked::Open(p.entries[0].path.clone())));
        for (w, h) in [(160, 40), (40, 10), (12, 6)] {
            let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            term.draw(|f| p.draw(f, &Theme::new(false, false))).unwrap();
        }
        press(&mut p, &j, KeyCode::Char('a'));
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 12)).unwrap();
        term.draw(|f| p.draw(f, &Theme::new(false, true))).unwrap();
        press(&mut p, &j, KeyCode::Esc);

        // started inside it: listed once, preselected
        let mut p = RepoPicker::new(&j, &repo.join("src"), None);
        assert_eq!(p.entries.len(), 1);
        press(&mut p, &j, KeyCode::Char('d'));
        press(&mut p, &j, KeyCode::Char('y'));
        assert!(j.lock().unwrap().repos().is_empty());
        // ...and still offered, unsaved, because grove runs inside it
        assert_eq!(p.entries.len(), 1);
        assert!(p.entries[0].saved.is_none());
        assert_eq!(press(&mut p, &j, KeyCode::Char('q')), Some(Picked::Quit));
        let _ = std::fs::remove_dir_all(root);
    }
}
