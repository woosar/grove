use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::style::Style;

use super::StyledLine;

const POLL: Duration = Duration::from_millis(50);
/// How long to keep reading after the shell exits. Build tools leave servers behind
/// (MSBuild nodes, VBCSCompiler) that inherit stdout and hold the pipe open for many
/// minutes, so end-of-file can't be what ends a step.
const PIPE_GRACE: Duration = Duration::from_millis(500);

/// One command line running in the platform's shell, with its output streamed as lines.
pub struct ShellProcess {
    child: Child,
    lines: Receiver<StyledLine>,
}

/// Asks the usual tools for colors even though their output goes into a pipe.
const FORCE_COLOR: [(&str, &str); 4] =
    [("FORCE_COLOR", "1"), ("CLICOLOR_FORCE", "1"), ("CARGO_TERM_COLOR", "always"), ("DOTNET_SYSTEM_CONSOLE_ALLOW_ANSI_COLOR_REDIRECTION", "1")];

/// `cmd` runs in a console of its own (nothing can scribble over the TUI) set to UTF-8,
/// so programs that follow the console code page produce output grove can decode. Only
/// cmd's own builtins (`echo`, `dir`) keep the code page it read at startup.
#[cfg(windows)]
fn shell(line: &str) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new("cmd");
    // raw: cmd has its own quoting rules; /S takes everything between the outer quotes as is
    cmd.raw_arg(format!("/D /S /C \"chcp 65001>nul & {line}\"")).creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// A process group of its own, so cancelling reaches everything the shell started.
#[cfg(not(windows))]
fn shell(line: &str) -> Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(line).process_group(0);
    cmd
}

/// Each stream keeps its own color state, as two terminals would.
fn pump(stream: impl Read + Send + 'static, tx: Sender<StyledLine>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        let mut style = Style::default();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(n) if n > 0 => {
                    if tx.send(StyledLine::parse(&buf, &mut style)).is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
    });
}

impl ShellProcess {
    pub fn spawn(line: &str, cwd: &Path) -> std::io::Result<ShellProcess> {
        let mut child =
            shell(line).envs(FORCE_COLOR).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let (tx, lines) = channel();
        if let Some(stdout) = child.stdout.take() {
            pump(stdout, tx.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            pump(stderr, tx);
        }
        Ok(ShellProcess { child, lines })
    }

    /// Takes the whole process tree down; the shell alone would leave the build running.
    fn kill_tree(&mut self) {
        let pid = self.child.id().to_string();
        #[cfg(windows)]
        let mut kill = {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let mut c = Command::new("taskkill");
            c.args(["/T", "/F", "/PID", &pid]).creation_flags(CREATE_NO_WINDOW);
            c
        };
        #[cfg(not(windows))]
        let mut kill = {
            let mut c = Command::new("kill");
            c.args(["-KILL", "--", &format!("-{pid}")]);
            c
        };
        let _ = kill.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        let _ = self.child.kill();
    }

    /// Hands over output lines until the shell exits or `cancel` is set, and returns the
    /// exit code (`None` when killed by a signal).
    pub fn wait(mut self, cancel: &AtomicBool, mut on_line: impl FnMut(StyledLine)) -> std::io::Result<Option<i32>> {
        let mut killed = false;
        let status = loop {
            match self.lines.recv_timeout(POLL) {
                Ok(line) => on_line(line),
                Err(RecvTimeoutError::Timeout) => {}
                // both streams closed while the process lives on
                Err(RecvTimeoutError::Disconnected) => thread::sleep(POLL),
            }
            if !killed && cancel.load(Ordering::Relaxed) {
                killed = true;
                self.kill_tree();
            }
            if let Some(status) = self.child.try_wait()? {
                break status;
            }
        };
        let deadline = Instant::now() + PIPE_GRACE;
        while let Ok(line) = self.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            on_line(line);
        }
        Ok(status.code())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn run(line: &str) -> (Option<i32>, Vec<String>) {
        let cancel = AtomicBool::new(false);
        let mut out = Vec::new();
        let code = ShellProcess::spawn(line, &std::env::temp_dir()).unwrap().wait(&cancel, |l| out.push(l.text())).unwrap();
        (code, out)
    }

    #[test]
    fn captures_stdout_stderr_and_the_exit_code() {
        let (code, out) = run("echo to-out&& echo to-err 1>&2&& exit 3");
        assert_eq!(code, Some(3));
        assert!(out.iter().any(|l| l.contains("to-out")), "{out:?}");
        assert!(out.iter().any(|l| l.contains("to-err")), "{out:?}");
    }

    #[test]
    fn non_ascii_output_survives() {
        // a real child process: that's what follows the code page, see `shell`
        let (code, out) = run(if cfg!(windows) { "cmd /D /C echo grüße" } else { "echo grüße" });
        assert_eq!(code, Some(0));
        assert!(out.iter().any(|l| l.contains("grüße")), "{out:?}");
    }

    #[test]
    fn a_leftover_background_process_does_not_hold_the_step_open() {
        let line = if cfg!(windows) { "start /b ping -n 20 127.0.0.1" } else { "sleep 20 &" };
        let started = Instant::now();
        let (code, _) = run(line);
        assert_eq!(code, Some(0));
        assert!(started.elapsed() < Duration::from_secs(10), "waited for the pipe: {:?}", started.elapsed());
    }

    #[test]
    fn cancelling_stops_the_process_tree_promptly() {
        let line = if cfg!(windows) { "ping -n 30 127.0.0.1" } else { "sleep 30" };
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        let code = ShellProcess::spawn(line, &std::env::temp_dir()).unwrap().wait(&cancel, |_| {}).unwrap();
        assert_ne!(code, Some(0));
        assert!(started.elapsed() < Duration::from_secs(10), "took {:?}", started.elapsed());
    }
}
