use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use super::{
    Commit, CommitDetail, ErrorReport, GitOutput, Job, JobResult, Journal, JournalEntry, PreflightInfo, RunOutcome,
    RunRequest, Snapshot, Step,
};
use crate::helpers::{is_mutating, local_hms, lossy, norm_path, now_ms, to_string_args};

/// Background execution. Mutating plans run one at a time on a dedicated thread (two
/// git operations must never race on one index); reads each get a short-lived thread.
pub struct Worker {
    run_tx: Sender<RunRequest>,
    res_tx: Sender<JobResult>,
    res_rx: Receiver<JobResult>,
}

fn rev_parse(cwd: &Path, what: &str) -> Option<String> {
    GitOutput::read(cwd, &["rev-parse", "--verify", "-q", what])
        .ok()
        .map(|o| lossy(&o).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn current_branch(cwd: &Path) -> Option<String> {
    GitOutput::read(cwd, &["symbolic-ref", "--short", "-q", "HEAD"])
        .ok()
        .map(|o| lossy(&o).trim().to_string())
        .filter(|s| !s.is_empty())
}

impl Worker {
    pub fn start(journal: Arc<Mutex<Journal>>) -> Worker {
        let (res_tx, res_rx) = channel();
        let (run_tx, run_rx) = channel::<RunRequest>();
        let tx = res_tx.clone();
        thread::spawn(move || {
            for req in run_rx {
                let outcome = Self::execute(&req, &journal);
                if tx.send(JobResult::Ran(Box::new(outcome))).is_err() {
                    break;
                }
            }
        });
        Worker { run_tx, res_tx, res_rx }
    }

    pub fn send(&self, job: Job) {
        if let Job::Run(req) = job {
            let _ = self.run_tx.send(*req);
            return;
        }
        let tx = self.res_tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Self::read(job));
        });
    }

    pub fn try_recv(&self) -> Option<JobResult> {
        self.res_rx.try_recv().ok()
    }

    fn read(job: Job) -> JobResult {
        match job {
            Job::Refresh { cwd, seq } => {
                let result = Snapshot::load(&cwd).map(Box::new).map_err(|e| e.to_string());
                JobResult::Snapshot { cwd, seq, result }
            }
            Job::Probe { cwd } => JobResult::Probe { bytes: Snapshot::probe(&cwd), cwd },
            Job::Preflight { cwd, target, dirty } => {
                let info = PreflightInfo::load(&cwd, &target, &dirty);
                JobResult::Preflight { cwd, target, info }
            }
            Job::LogPage { cwd, rev_args, skip, generation, remotes } => {
                let mut args: Vec<String> =
                    vec!["log".into(), "--topo-order".into(), "-z".into(), Commit::FORMAT.into(), format!("--skip={skip}"), "-n".into(), "200".into()];
                // the path filter (after `--`) has to stay last
                args.extend(rev_args.iter().cloned());
                let commits = GitOutput::read(&cwd, &args).map(|b| Commit::parse_log(&b, &remotes)).unwrap_or_default();
                let total = (skip == 0).then(|| {
                    let mut count: Vec<String> = vec!["rev-list".into(), "--count".into()];
                    count.extend(rev_args.iter().cloned());
                    GitOutput::read(&cwd, &count).ok().and_then(|o| lossy(&o).trim().parse().ok()).unwrap_or(0)
                });
                JobResult::LogPage { generation, skip, commits, total }
            }
            Job::Detail { cwd, oid, remotes } => {
                let get = |args: &[&str]| GitOutput::read(&cwd, args).unwrap_or_default();
                let header = get(&["show", "-s", CommitDetail::HEADER_FORMAT, &oid]);
                let numstat = get(&["show", "--format=", "-z", "--numstat", "--diff-merges=first-parent", &oid]);
                let names = get(&["show", "--format=", "-z", "--name-status", "--diff-merges=first-parent", &oid]);
                let contains = get(&["branch", "-a", "--contains", &oid, "--format=%(refname:short)"]);
                let mut patch = get(&["show", "--format=", "--patch", "--diff-merges=first-parent", &oid]);
                patch.truncate(512 * 1024);
                JobResult::Detail(Box::new(CommitDetail::parse(&header, &numstat, &names, &contains, &patch, &remotes)))
            }
            Job::Run(_) => unreachable!("runs go to the mutation thread"),
        }
    }

    fn execute(req: &RunRequest, journal: &Mutex<Journal>) -> RunOutcome {
        let cwd = req.cwd.as_path();
        let mut outcome = RunOutcome {
            plan: req.plan.clone(),
            cwd: req.cwd.clone(),
            entry_ids: Vec::new(),
            failure: None,
            stdout: String::new(),
            warnings: Vec::new(),
        };
        let repo_root = norm_path(&req.snapshot.common_dir);
        for step in &req.plan.steps {
            match step {
                Step::Git(args) => {
                    let sargs = to_string_args(args);
                    let mutating = is_mutating(&sargs);
                    let branch = current_branch(cwd);
                    let head_before = if mutating { rev_parse(cwd, "HEAD") } else { None };
                    let mut note = Vec::new();
                    if sargs.first().is_some_and(|s| s == "branch")
                        && sargs.iter().any(|a| matches!(a.as_str(), "-d" | "-D" | "--delete"))
                    {
                        for name in sargs.iter().skip(1).filter(|a| !a.starts_with('-')) {
                            if let Some(oid) = rev_parse(cwd, &format!("refs/heads/{name}")) {
                                note.push(format!("{}{name} at {oid}", JournalEntry::NOTE_DELETED));
                            }
                        }
                    }
                    let started_at = now_ms();
                    let out = GitOutput::run(args, cwd);
                    let head_after = if mutating { rev_parse(cwd, "HEAD") } else { None };
                    if !out.ok() {
                        // nothing was deleted when git refused
                        note.clear();
                    }
                    let entry = JournalEntry {
                        id: 0,
                        started_at,
                        duration_ms: out.duration_ms,
                        repo_root: repo_root.clone(),
                        worktree: norm_path(cwd),
                        branch,
                        via: req.via.clone(),
                        argv: sargs.clone(),
                        exit_code: out.status,
                        stdout: Some(out.stdout_text()),
                        stderr: Some(out.stderr_text()),
                        reflog_before: head_before.clone(),
                        head_before,
                        head_after,
                        mutating,
                        parent_id: req.parent,
                        note: (!note.is_empty()).then(|| note.join("\n")),
                    };
                    let id = journal.lock().ok().and_then(|j| j.insert(&entry).ok());
                    outcome.entry_ids.extend(id);
                    outcome.stdout.push_str(&out.stdout_text());
                    if !out.ok() {
                        outcome.failure = Some(ErrorReport::build(&sargs, &out, id, &req.snapshot));
                        break;
                    }
                }
                Step::Copy { from, to, files } => {
                    for f in files {
                        let dest = to.join(f);
                        if let Some(dir) = dest.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        if let Err(e) = std::fs::copy(from.join(f), &dest) {
                            outcome.warnings.push(format!("couldn't copy {}: {e}", f.display()));
                        }
                    }
                }
            }
        }
        if let (Some(parent), Some(first)) = (req.parent, outcome.entry_ids.first())
            && let Ok(j) = journal.lock()
        {
            let result = if outcome.failure.is_some() { "It failed too." } else { "It succeeded." };
            let _ = j.append_note(
                parent,
                &format!(
                    "you chose \"{}\" in the error dialog at {} (entry #{first}). {result}",
                    req.label,
                    local_hms(now_ms())
                ),
            );
        }
        outcome
    }
}
