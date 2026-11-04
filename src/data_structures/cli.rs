use std::path::PathBuf;

use clap::Parser;

/// A terminal UI around the git CLI that never hides git.
#[derive(Parser, Debug)]
#[command(name = "grove", version, about)]
pub struct Cli {
    /// Repository to open (default: the current directory).
    pub path: Option<PathBuf>,

    /// Draw with ASCII glyphs and borders instead of Unicode.
    #[arg(long)]
    pub ascii: bool,

    /// Untracked file offered for copying into new worktrees (repeatable).
    #[arg(long = "copy", value_name = "PATH", default_values = [".env", ".cargo/config.toml"])]
    pub copy: Vec<PathBuf>,

    /// Journal database to use instead of `<data dir>/grove/journal.db`.
    #[arg(long, value_name = "FILE")]
    pub journal: Option<PathBuf>,
}
