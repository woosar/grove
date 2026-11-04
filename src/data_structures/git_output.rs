use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Result, bail};

use crate::helpers::{command_line, lossy, to_os_args};

/// Result of one `git` invocation. The runner always passes `-c color.ui=never`, and
/// sets `LC_ALL=C` so stderr phrases stay stable for classification.
#[derive(Clone, Debug, Default)]
pub struct GitOutput {
    /// `None` when git could not be spawned or was killed by a signal.
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: i64,
}

impl GitOutput {
    pub fn run(args: &[OsString], cwd: &Path) -> GitOutput {
        let started = Instant::now();
        let result = Command::new("git")
            .arg("-c")
            .arg("color.ui=never")
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C")
            .env("GIT_PAGER", "cat")
            // never block on an editor nobody can see; git falls back to its default message
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .stdin(Stdio::null())
            .output();
        let duration_ms = started.elapsed().as_millis() as i64;
        match result {
            Ok(out) => GitOutput { status: out.status.code(), stdout: out.stdout, stderr: out.stderr, duration_ms },
            Err(e) => GitOutput {
                status: None,
                stdout: Vec::new(),
                stderr: format!("grove: could not run git: {e}").into_bytes(),
                duration_ms,
            },
        }
    }

    /// Runs a read-only query that has to succeed, returning stdout.
    pub fn read<S: AsRef<str>>(cwd: &Path, args: &[S]) -> Result<Vec<u8>> {
        let argv = to_os_args(args);
        let out = Self::run(&argv, cwd);
        if !out.ok() {
            bail!("{} failed: {}", command_line(&argv), lossy(&out.stderr).trim());
        }
        Ok(out.stdout)
    }

    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }

    pub fn stdout_text(&self) -> String {
        lossy(&self.stdout)
    }

    pub fn stderr_text(&self) -> String {
        lossy(&self.stderr)
    }
}
