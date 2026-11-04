use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{JournalEntry, JournalFilter};

/// Cap for stored stdout/stderr.
const MAX_OUTPUT: usize = 64 * 1024;

const MIGRATIONS: &[&str] = &["CREATE TABLE entry (
  id            INTEGER PRIMARY KEY,
  started_at    INTEGER NOT NULL,
  duration_ms   INTEGER NOT NULL,
  repo_root     TEXT NOT NULL,
  worktree      TEXT NOT NULL,
  branch        TEXT,
  via           TEXT NOT NULL,
  argv          TEXT NOT NULL,
  exit_code     INTEGER,
  stdout        TEXT,
  stderr        TEXT,
  head_before   TEXT,
  head_after    TEXT,
  reflog_before TEXT,
  mutating      INTEGER NOT NULL,
  parent_id     INTEGER REFERENCES entry(id),
  note          TEXT
);
CREATE INDEX entry_repo_time ON entry(repo_root, started_at DESC);"];

const COLUMNS: &str = "id, started_at, duration_ms, repo_root, worktree, branch, via, argv, exit_code, stdout, stderr, head_before, head_after, reflog_before, mutating, parent_id, note";

/// The persistent command journal (SQLite, WAL mode).
pub struct Journal {
    conn: Connection,
    pub path: Option<PathBuf>,
}

fn cap(s: &Option<String>) -> Option<String> {
    s.as_ref().map(|s| {
        if s.len() <= MAX_OUTPUT {
            return s.clone();
        }
        let mut end = MAX_OUTPUT;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}\n… (truncated)", &s[..end])
    })
}

fn from_row(row: &Row) -> rusqlite::Result<JournalEntry> {
    let argv: String = row.get(7)?;
    Ok(JournalEntry {
        id: row.get(0)?,
        started_at: row.get(1)?,
        duration_ms: row.get(2)?,
        repo_root: row.get(3)?,
        worktree: row.get(4)?,
        branch: row.get(5)?,
        via: row.get(6)?,
        argv: serde_json::from_str(&argv).unwrap_or_default(),
        exit_code: row.get(8)?,
        stdout: row.get(9)?,
        stderr: row.get(10)?,
        head_before: row.get(11)?,
        head_after: row.get(12)?,
        reflog_before: row.get(13)?,
        mutating: row.get::<_, i64>(14)? != 0,
        parent_id: row.get(15)?,
        note: row.get(16)?,
    })
}

impl Journal {
    /// `<data_local_dir>/journal.db`, e.g. `%LOCALAPPDATA%\grove\journal.db`.
    pub fn default_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "grove").map(|d| d.data_local_dir().join("journal.db"))
    }

    pub fn open(path: &Path) -> Result<Journal> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(3))?;
        let journal = Journal { conn, path: Some(path.to_path_buf()) };
        journal.migrate()?;
        Ok(journal)
    }

    pub fn open_in_memory() -> Result<Journal> {
        let journal = Journal { conn: Connection::open_in_memory()?, path: None };
        journal.migrate()?;
        Ok(journal)
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            self.conn.execute_batch(&format!("BEGIN; {sql}; PRAGMA user_version = {}; COMMIT;", i + 1))?;
        }
        Ok(())
    }

    pub fn insert(&self, e: &JournalEntry) -> Result<i64> {
        self.conn.execute(
            &format!("INSERT INTO entry ({}) VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)", COLUMNS),
            params![
                e.started_at,
                e.duration_ms,
                e.repo_root,
                e.worktree,
                e.branch,
                e.via,
                serde_json::to_string(&e.argv)?,
                e.exit_code,
                cap(&e.stdout),
                cap(&e.stderr),
                e.head_before,
                e.head_after,
                e.reflog_before,
                e.mutating as i64,
                e.parent_id,
                e.note,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn append_note(&self, id: i64, text: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE entry SET note = CASE WHEN note IS NULL OR note = '' THEN ?2 ELSE note || char(10) || ?2 END WHERE id = ?1",
            params![id, text],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn get(&self, id: i64) -> Option<JournalEntry> {
        self.conn
            .query_row(&format!("SELECT {COLUMNS} FROM entry WHERE id = ?1"), [id], from_row)
            .optional()
            .ok()
            .flatten()
    }

    pub fn query(&self, f: &JournalFilter) -> Vec<JournalEntry> {
        let sql = format!(
            "SELECT {COLUMNS} FROM entry
             WHERE (?1 IS NULL OR repo_root = ?1)
               AND (?2 IS NULL OR date(started_at / 1000, 'unixepoch', 'localtime') = ?2)
               AND (?3 IS NULL OR date(started_at / 1000, 'unixepoch', 'localtime') < ?3)
               AND (mutating = 1 OR ?4)
               AND (via != ':' OR ?5)
               AND (via = ':' OR ?6)
               AND (?7 = 0 OR exit_code IS NULL OR exit_code != 0)
               AND (?8 = '' OR argv LIKE '%' || ?8 || '%' OR note LIKE '%' || ?8 || '%' OR stderr LIKE '%' || ?8 || '%')
             ORDER BY started_at DESC, id DESC
             LIMIT ?9"
        );
        let run = || -> rusqlite::Result<Vec<JournalEntry>> {
            let mut stmt = self.conn.prepare_cached(&sql)?;
            let rows = stmt.query_map(
                params![f.repo, f.day, f.before_day, f.show_readonly, f.show_raw, f.show_grove, f.failed_only, f.search, f.limit as i64],
                from_row,
            )?;
            rows.collect()
        };
        run().unwrap_or_default()
    }

    /// `(local day, entry count)`, newest first.
    pub fn days(&self, repo: Option<&str>, show_readonly: bool) -> Vec<(String, i64)> {
        let run = || -> rusqlite::Result<Vec<(String, i64)>> {
            let mut stmt = self.conn.prepare_cached(
                "SELECT date(started_at / 1000, 'unixepoch', 'localtime') AS d, count(*) FROM entry
                 WHERE (?1 IS NULL OR repo_root = ?1) AND (mutating = 1 OR ?2)
                 GROUP BY d ORDER BY d DESC",
            )?;
            let rows = stmt.query_map(params![repo, show_readonly], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        };
        run().unwrap_or_default()
    }

    /// The most recent changing commands of a repo, for the dashboard tail.
    pub fn tail(&self, repo: &str, n: usize) -> Vec<JournalEntry> {
        self.query(&JournalFilter { repo: Some(repo.to_string()), limit: n, ..Default::default() })
    }

    /// The newest successful mutating entry in a repo — what `u` undoes.
    pub fn latest_mutating(&self, repo: &str) -> Option<JournalEntry> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM entry WHERE repo_root = ?1 AND mutating = 1 AND exit_code = 0 ORDER BY started_at DESC, id DESC LIMIT 1"),
                [repo],
                from_row,
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Last journal activity per worktree path (normalized), in unix ms.
    pub fn last_used(&self, repo: &str) -> HashMap<String, i64> {
        let run = || -> rusqlite::Result<HashMap<String, i64>> {
            let mut stmt = self.conn.prepare_cached("SELECT worktree, max(started_at) FROM entry WHERE repo_root = ?1 GROUP BY worktree")?;
            let rows = stmt.query_map([repo], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            rows.collect()
        };
        run().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(argv: &[&str], exit: i32, via: &str, mutating: bool) -> JournalEntry {
        JournalEntry {
            started_at: crate::helpers::now_ms(),
            duration_ms: 30,
            repo_root: "/r/.git".into(),
            worktree: "/r".into(),
            branch: Some("main".into()),
            via: via.into(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
            exit_code: Some(exit),
            mutating,
            ..Default::default()
        }
    }

    #[test]
    fn insert_query_and_filters() {
        let j = Journal::open_in_memory().unwrap();
        let a = j.insert(&sample(&["switch", "main"], 0, "s", true)).unwrap();
        j.insert(&sample(&["rebase", "main"], 1, "r", true)).unwrap();
        j.insert(&sample(&["log", "--oneline"], 0, ":", false)).unwrap();
        j.insert(&sample(&["config", "x", "y"], 0, ":", true)).unwrap();

        let all = j.query(&JournalFilter::default());
        assert_eq!(all.len(), 3, "read-only hidden by default");
        assert_eq!(all[0].argv, ["config", "x", "y"]);
        let failed = j.query(&JournalFilter { failed_only: true, ..Default::default() });
        assert_eq!(failed.len(), 1);
        let raw_only = j.query(&JournalFilter { show_grove: false, show_readonly: true, ..Default::default() });
        assert_eq!(raw_only.len(), 2);
        let found = j.query(&JournalFilter { search: "rebase".into(), ..Default::default() });
        assert_eq!(found.len(), 1);

        j.append_note(a, "first").unwrap();
        j.append_note(a, "second").unwrap();
        assert_eq!(j.get(a).unwrap().note.as_deref(), Some("first\nsecond"));
        assert_eq!(j.latest_mutating("/r/.git").unwrap().argv, ["config", "x", "y"]);
        assert_eq!(j.days(Some("/r/.git"), false)[0].1, 3);
    }

    #[test]
    fn survives_reopen() {
        let path = std::env::temp_dir().join(format!("grove-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let j = Journal::open(&path).unwrap();
            j.insert(&sample(&["commit", "-m", "x"], 0, "c", true)).unwrap();
        }
        let j = Journal::open(&path).unwrap();
        assert_eq!(j.query(&JournalFilter::default())[0].argv, ["commit", "-m", "x"]);
        drop(j);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
