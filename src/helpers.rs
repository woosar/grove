use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Local, NaiveDate, TimeZone};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Color;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Splits NUL-separated git output into fields, dropping a trailing empty field.
pub fn split_nul(bytes: &[u8]) -> Vec<&[u8]> {
    let mut fields: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    if fields.last().is_some_and(|f| f.is_empty()) {
        fields.pop();
    }
    fields
}

pub fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Git hands out raw bytes for paths. On Unix they map 1:1 onto `OsStr`; on Windows git
/// always emits UTF-8, so a lossy decode is the best available mapping.
pub fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(lossy(bytes))
    }
}

/// Path form used only for comparing paths: forward slashes, no trailing slash and,
/// on Windows, lowercase (git reports `C:/x`, Rust hands out `c:\x`).
pub fn norm_path(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = s.strip_prefix("//?/") {
        s = stripped.to_string();
    }
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    if cfg!(windows) { s.to_lowercase() } else { s }
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    norm_path(a) == norm_path(b)
}

/// `target` expressed relative to `base` (`../grove.wt/x`), with forward slashes.
/// Falls back to the absolute path when they share no root (different drives).
pub fn relative_to(target: &Path, base: &Path) -> String {
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .map(|c| c.as_os_str().to_string_lossy().replace('\\', "/"))
            .collect()
    };
    let key = |s: &String| if cfg!(windows) { s.to_lowercase() } else { s.clone() };
    let (t, b) = (parts(target), parts(base));
    let common = t.iter().zip(&b).take_while(|(x, y)| key(x) == key(y)).count();
    if common == 0 {
        return target.to_string_lossy().replace('\\', "/");
    }
    let mut out: Vec<String> = std::iter::repeat_n("..".to_string(), b.len() - common).collect();
    out.extend(t[common..].iter().cloned());
    if out.is_empty() { ".".to_string() } else { out.join("/") }
}

/// `base` joined with `rel`, with `.` and `..` resolved without touching the disk.
pub fn lexical_join(base: &Path, rel: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in base.join(rel).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// The target when a pipeline step is nothing but a `cd`: `""` for a bare `cd`. Lines
/// with shell syntax (`cd x && build`, `cd $HOME`) are left for the shell to run.
pub fn cd_target(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("cd")?;
    // `cd..` and `cd\` are valid in cmd; `cdk deploy` is another program
    if !rest.is_empty() && !rest.starts_with(|c: char| c.is_whitespace() || matches!(c, '.' | '/' | '\\')) {
        return None;
    }
    let mut target = rest.trim();
    if let Some(after) = target.strip_prefix("/d ").or_else(|| target.strip_prefix("/D ")) {
        target = after.trim();
    }
    if target.contains(['&', '|', ';', '<', '>', '$', '%', '`', '*', '?']) {
        return None;
    }
    for quote in ['"', '\''] {
        if let Some(inner) = target.strip_prefix(quote).and_then(|t| t.strip_suffix(quote)) {
            return Some(inner);
        }
    }
    Some(target)
}

/// Where a `cd` step leads: a bare `cd` returns to the worktree root, anything else is
/// relative to the current directory.
pub fn next_dir(root: &Path, dir: &Path, target: &str) -> PathBuf {
    if target.is_empty() { root.to_path_buf() } else { lexical_join(dir, Path::new(target)) }
}

/// MSBuild (`dotnet build` and friends) colors through the console API, which a pipe
/// never sees; `-clp:ForceConsoleColor` makes it write ANSI colors instead. Added only to
/// a line that is a single such command without console logger options of its own.
pub fn force_console_color(line: &str) -> Cow<'_, str> {
    const VERBS: [&str; 8] = ["build", "test", "publish", "pack", "restore", "clean", "msbuild", "rebuild"];
    let lower = line.trim().to_ascii_lowercase();
    let mut words = lower.split_whitespace();
    let first = words.next().unwrap_or("");
    let msbuild = match first.trim_end_matches(".exe") {
        "dotnet" => words.next().is_some_and(|v| VERBS.contains(&v)),
        "msbuild" => true,
        _ => false,
    };
    let chained = lower.contains(['&', '|']);
    let has_own = ["-clp", "/clp", "-consoleloggerparameters", "--consoleloggerparameters", "/consoleloggerparameters"]
        .iter()
        .any(|o| lower.split_whitespace().any(|w| w.starts_with(o)));
    if msbuild && !chained && !has_own { Cow::Owned(format!("{} -clp:ForceConsoleColor", line.trim_end())) } else { Cow::Borrowed(line) }
}

pub fn home_tilde(path: &Path) -> String {
    let shown = path.to_string_lossy().replace('\\', "/");
    if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) {
        let home_s = home.to_string_lossy().replace('\\', "/");
        let (a, b) = if cfg!(windows) {
            (shown.to_lowercase(), home_s.to_lowercase())
        } else {
            (shown.clone(), home_s.clone())
        };
        if a == b {
            return "~".to_string();
        }
        if a.starts_with(&format!("{b}/")) {
            return format!("~{}", &shown[home_s.len()..]);
        }
    }
    shown
}

/// How worktree paths are shown: the main checkout as `~/src/grove`, its siblings as
/// `../grove.wt/x` (the way the mockups and most people think of them).
pub fn short_path(path: &Path, main_worktree: &Path) -> String {
    if same_path(path, main_worktree) {
        return home_tilde(path);
    }
    match main_worktree.parent() {
        Some(parent) if norm_path(path).starts_with(&format!("{}/", norm_path(parent))) => {
            relative_to(path, main_worktree)
        }
        _ => home_tilde(path),
    }
}

/// Last path segment of a branch, safe as a directory name: `feat/log-graph` → `log-graph`.
pub fn branch_slug(branch: &str) -> String {
    let last = branch.rsplit('/').next().unwrap_or(branch);
    let slug: String = last
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
        .collect();
    if slug.is_empty() { "worktree".to_string() } else { slug }
}

pub fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_alphanumeric() || "-_./:=@,+%^~{}".contains(c))
    {
        return word.to_string();
    }
    if !word.contains('"') && !word.contains('$') && !word.contains('`') && !word.contains('\\') {
        return format!("\"{word}\"");
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// `git <args…>` as one display string. Display only — execution always uses argv.
pub fn command_line(args: &[OsString]) -> String {
    let mut out = String::from("git");
    for a in args {
        out.push(' ');
        out.push_str(&shell_quote(&a.to_string_lossy()));
    }
    out
}

pub fn to_os_args<S: AsRef<str>>(args: &[S]) -> Vec<OsString> {
    args.iter().map(|a| OsString::from(a.as_ref())).collect()
}

pub fn to_string_args(args: &[OsString]) -> Vec<String> {
    args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
}

/// Minimal POSIX-ish word splitting for the `:` prompt: whitespace, '…', "…" and `\`.
pub fn split_shell_words(input: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => cur.push(ch),
                        None => return Err("unterminated ' quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(ch) => cur.push(ch),
                            None => return Err("trailing backslash".into()),
                        },
                        Some(ch) => cur.push(ch),
                        None => return Err("unterminated \" quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(ch) => cur.push(ch),
                    None => return Err("trailing backslash".into()),
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// Whether a git invocation can change repository state. Unknown subcommands count as
/// mutating so they are never hidden from the journal by default.
pub fn is_mutating(args: &[String]) -> bool {
    let mut it = args.iter().skip_while(|a| a.starts_with('-'));
    let Some(sub) = it.next() else { return false };
    let rest: Vec<&String> = it.collect();
    let has = |f: &str| rest.iter().any(|a| a.as_str() == f);
    match sub.as_str() {
        "status" | "log" | "show" | "diff" | "for-each-ref" | "rev-parse" | "rev-list" | "ls-files"
        | "blame" | "grep" | "describe" | "shortlog" | "cat-file" | "help" | "version"
        | "show-ref" | "merge-base" | "name-rev" | "whatchanged" | "ls-remote" | "count-objects" => {
            false
        }
        "branch" => {
            rest.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "-d" | "-D" | "-m" | "-M" | "-c" | "-C" | "-u" | "--delete" | "--move" | "--copy"
                        | "--unset-upstream" | "--edit-description" | "-f" | "--force"
                ) || a.starts_with("--set-upstream-to") || a.starts_with("-u")
            }) || (rest.iter().any(|a| !a.starts_with('-'))
                && !has("--list")
                && !has("-l")
                && !rest.iter().any(|a| a.starts_with("--contains") || a.starts_with("--merged") || a.starts_with("--no-merged") || a.starts_with("--format")))
        }
        "tag" => rest.iter().any(|a| !a.starts_with('-')) && !has("-l") && !has("--list"),
        "stash" => !matches!(rest.first().map(|s| s.as_str()), Some("list" | "show")),
        "worktree" => !matches!(rest.first().map(|s| s.as_str()), Some("list")),
        "remote" => matches!(
            rest.first().map(|s| s.as_str()),
            Some("add" | "remove" | "rm" | "rename" | "set-url" | "set-head" | "prune" | "update")
        ),
        "config" => {
            let read = has("--get") || has("--list") || has("-l") || has("--get-all");
            let read_sub = rest.first().is_some_and(|a| a.as_str() == "get" || a.as_str() == "list");
            // `git config <key>` with no value reads
            let positional = rest.iter().filter(|a| !a.starts_with('-')).count();
            !(read || read_sub || positional == 1)
        }
        "reflog" => matches!(rest.first().map(|s| s.as_str()), Some("expire" | "delete")),
        _ => true,
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn now_unix() -> i64 {
    now_ms() / 1000
}

/// `12m`, `3h`, `1d`, `3w`, `2mo`, `1y`.
pub fn relative_age(unix: i64) -> String {
    let secs = (now_unix() - unix).max(0);
    match secs {
        s if s < 60 => "now".to_string(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s if s < 7 * 86_400 => format!("{}d", s / 86_400),
        s if s < 30 * 86_400 => format!("{}w", s / (7 * 86_400)),
        s if s < 365 * 86_400 => format!("{}mo", s / (30 * 86_400)),
        s => format!("{}y", s / (365 * 86_400)),
    }
}

/// `3h ago`, `now` — the longer form used in tables with room to spare.
pub fn relative_age_long(unix: i64) -> String {
    match relative_age(unix) {
        s if s == "now" => s,
        s => format!("{s} ago"),
    }
}

pub fn fmt_duration_ms(ms: i64) -> String {
    format!("{:.2}s", ms as f64 / 1000.0)
}

pub fn local_hms(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

pub fn local_date_long(unix: i64) -> String {
    Local
        .timestamp_opt(unix, 0)
        .single()
        .map(|t| t.format("%a %d %b %Y %H:%M").to_string())
        .unwrap_or_default()
}

/// `Today`, `Yesterday`, `Sun 25 Oct` for a `YYYY-MM-DD` day key.
pub fn day_label(day: &str) -> String {
    let Ok(date) = NaiveDate::parse_from_str(day, "%Y-%m-%d") else {
        return day.to_string();
    };
    let today = Local::now().date_naive();
    if date == today {
        "Today".to_string()
    } else if Some(date) == today.pred_opt() {
        "Yesterday".to_string()
    } else {
        date.format("%a %d %b").to_string()
    }
}

pub fn str_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Cuts `s` to at most `width` cells, ending in `…` when something was dropped.
pub fn truncate(s: &str, width: usize) -> String {
    if str_width(s) <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

pub fn pad(s: &str, width: usize) -> String {
    let t = truncate(s, width);
    let fill = width.saturating_sub(str_width(&t));
    format!("{t}{}", " ".repeat(fill))
}

pub fn pad_left(s: &str, width: usize) -> String {
    let t = truncate(s, width);
    let fill = width.saturating_sub(str_width(&t));
    format!("{}{t}", " ".repeat(fill))
}

/// Parses `git diff --numstat -z`; binary files report `None` counts.
pub fn parse_numstat_z(bytes: &[u8]) -> HashMap<PathBuf, (Option<u32>, Option<u32>)> {
    let mut out = HashMap::new();
    let fields = split_nul(bytes);
    let mut i = 0;
    while i < fields.len() {
        let f = lossy(fields[i]);
        let f = f.trim_start_matches('\n');
        let mut parts = f.splitn(3, '\t');
        let added = parts.next().and_then(|s| s.parse().ok());
        let removed = parts.next().and_then(|s| s.parse().ok());
        let path = parts.next().unwrap_or("");
        if path.is_empty() {
            // rename: the old and new paths follow as separate fields
            if let Some(new) = fields.get(i + 2) {
                out.insert(bytes_to_path(new), (added, removed));
            }
            i += 3;
        } else {
            out.insert(PathBuf::from(path), (added, removed));
            i += 1;
        }
    }
    out
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Copies through the terminal (OSC 52), which works over SSH and needs no clipboard crate.
pub fn copy_to_clipboard(text: &str) {
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let _ = out.flush();
}

pub fn truecolor_supported() -> bool {
    let colorterm = std::env::var("COLORTERM").unwrap_or_default().to_lowercase();
    colorterm.contains("truecolor") || colorterm.contains("24bit") || std::env::var_os("WT_SESSION").is_some()
}

/// Nearest xterm-256 entry, picking between the 6×6×6 cube and the gray ramp.
pub fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    let level = |v: u8| -> u8 { if v < 48 { 0 } else if v < 115 { 1 } else { (v - 35) / 40 } };
    let steps = [0u8, 95, 135, 175, 215, 255];
    let (cr, cg, cb) = (level(r), level(g), level(b));
    let cube = 16 + 36 * cr + 6 * cg + cb;
    let cube_rgb = (steps[cr as usize], steps[cg as usize], steps[cb as usize]);
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let gray_idx = if avg > 238 { 23 } else { (avg.saturating_sub(3) / 10) as u8 };
    let gray_v = 8 + 10 * gray_idx as u32;
    let dist = |x: (u32, u32, u32)| {
        let d = |a: u32, b: u8| (a as i64 - b as i64).pow(2);
        d(x.0, r) + d(x.1, g) + d(x.2, b)
    };
    if dist((gray_v, gray_v, gray_v)) < dist((cube_rgb.0 as u32, cube_rgb.1 as u32, cube_rgb.2 as u32)) {
        232 + gray_idx
    } else {
        cube
    }
}

pub fn color(hex: u32, truecolor: bool) -> Color {
    let (r, g, b) = ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    if truecolor { Color::Rgb(r, g, b) } else { Color::Indexed(rgb_to_256(r, g, b)) }
}

pub fn short_oid(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

/// Word-wraps to `width` cells; overlong words are hard-split. Keeps empty lines.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let mut word = word.to_string();
            while str_width(&word) > width {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                let cut: String = word.chars().take(width).collect();
                word = word.chars().skip(width).collect();
                out.push(cut);
            }
            let needed = if line.is_empty() { str_width(&word) } else { str_width(&line) + 1 + str_width(&word) };
            if needed > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        out.push(line);
    }
    out
}

/// A `w`×`h` rect centered in `area`, clamped to fit.
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

/// A `w`×`h` rect placed just below-right of `anchor`, shifted to stay inside `area`.
pub fn anchored(area: Rect, anchor: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let mut x = anchor.x.saturating_add(anchor.width / 3);
    let mut y = anchor.y.saturating_add(1);
    if x + w > area.right() {
        x = area.right().saturating_sub(w);
    }
    if y + h > area.bottom() {
        y = anchor.y.saturating_sub(h).max(area.y);
    }
    Rect::new(x.max(area.x), y.max(area.y), w, h)
}

/// Adjusts a list's scroll offset so `sel` stays within `height` visible rows.
pub fn keep_visible(offset: &mut usize, sel: usize, height: usize) {
    let height = height.max(1);
    if sel < *offset {
        *offset = sel;
    } else if sel >= *offset + height {
        *offset = sel + 1 - height;
    }
}

/// List navigation shared by every list: j/k, arrows, page keys, g/G, home/end.
/// Returns the new selection when `key` is a movement key.
pub fn nav(key: &KeyEvent, sel: usize, len: usize) -> Option<usize> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    let last = len.saturating_sub(1);
    Some(match key.code {
        KeyCode::Char('j') | KeyCode::Down => step(sel, 1, len),
        KeyCode::Char('k') | KeyCode::Up => step(sel, -1, len),
        KeyCode::PageDown => step(sel, 10, len),
        KeyCode::PageUp => step(sel, -10, len),
        KeyCode::Char('g') | KeyCode::Home => 0,
        KeyCode::Char('G') | KeyCode::End => last,
        _ => return None,
    })
}

/// Moves a selection by `delta` within `0..len`.
pub fn step(sel: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (sel as isize + delta).clamp(0, len as isize - 1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_round_trips_through_word_splitting() {
        let args = ["commit", "-m", "journal: persist entries (WAL mode)", "it's", ""];
        let line = command_line(&to_os_args(&args));
        assert_eq!(line, "git commit -m \"journal: persist entries (WAL mode)\" \"it's\" \"\"");
        let words = split_shell_words(line.strip_prefix("git ").unwrap()).unwrap();
        assert_eq!(words, args);
    }

    #[test]
    fn mutating_classification() {
        let m = |s: &str| is_mutating(&split_shell_words(s).unwrap());
        assert!(!m("status --porcelain=v2"));
        assert!(!m("log --oneline"));
        assert!(!m("branch --contains abc"));
        assert!(!m("branch"));
        assert!(m("branch -d feat/x"));
        assert!(m("branch new-name"));
        assert!(m("switch main"));
        assert!(!m("stash list"));
        assert!(m("stash push -m x"));
        assert!(!m("worktree list"));
        assert!(m("config rerere.enabled true"));
        assert!(!m("config --get user.name"));
    }

    #[test]
    fn relative_paths() {
        let base = Path::new("/home/u/src/grove");
        assert_eq!(relative_to(Path::new("/home/u/src/grove.wt/x"), base), "../grove.wt/x");
        assert_eq!(relative_to(base, base), ".");
        assert_eq!(branch_slug("feat/log-graph"), "log-graph");
    }

    #[test]
    fn numstat_with_rename() {
        let parsed = parse_numstat_z(b"1\t0\tCargo.toml\x003\t2\t\x00old.rs\x00new.rs\x00-\t-\tlogo.png\x00");
        assert_eq!(parsed[&PathBuf::from("Cargo.toml")], (Some(1), Some(0)));
        assert_eq!(parsed[&PathBuf::from("new.rs")], (Some(3), Some(2)));
        assert_eq!(parsed[&PathBuf::from("logo.png")], (None, None));
    }

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64(b"grove"), "Z3JvdmU=");
        assert_eq!(base64(b"ab"), "YWI=");
    }

    #[test]
    fn only_plain_cd_lines_are_sticky() {
        assert_eq!(cd_target("cd"), Some(""));
        assert_eq!(cd_target("  cd src/App  "), Some("src/App"));
        assert_eq!(cd_target("cd.."), Some(".."));
        assert_eq!(cd_target("cd /d D:\\work"), Some("D:\\work"));
        assert_eq!(cd_target("cd \"Program Files\""), Some("Program Files"));
        assert_eq!(cd_target("cd Program Files"), Some("Program Files"));
        assert_eq!(cd_target("cd x && dotnet build"), None);
        assert_eq!(cd_target("cd %USERPROFILE%"), None);
        assert_eq!(cd_target("cdk deploy"), None);
        assert_eq!(cd_target("dotnet build"), None);
    }

    #[test]
    fn cd_paths_resolve_without_the_disk() {
        let root = Path::new("root");
        let dir = root.join("a").join("b");
        assert_eq!(next_dir(root, &dir, ""), root);
        assert_eq!(next_dir(root, &dir, "../c"), root.join("a").join("c"));
        assert_eq!(next_dir(root, &dir, "./d"), dir.join("d"));
    }

    #[test]
    fn msbuild_is_asked_for_colors() {
        assert_eq!(force_console_color("dotnet build"), "dotnet build -clp:ForceConsoleColor");
        assert_eq!(force_console_color("dotnet test -c Release "), "dotnet test -c Release -clp:ForceConsoleColor");
        assert_eq!(force_console_color("msbuild app.sln"), "msbuild app.sln -clp:ForceConsoleColor");
        assert_eq!(force_console_color("dotnet run"), "dotnet run");
        assert_eq!(force_console_color("dotnet build && dotnet test"), "dotnet build && dotnet test");
        assert_eq!(force_console_color("dotnet build -clp:NoSummary"), "dotnet build -clp:NoSummary");
        assert_eq!(force_console_color("cargo build"), "cargo build");
    }

    #[test]
    fn truncation_is_width_aware() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(pad("ab", 4), "ab  ");
    }
}
