use std::path::{Path, PathBuf};

use super::{AffectedItem, ChangeArea, InternalOp, Plan, PromptKind, Remedy, ScreenId, Snapshot};
use crate::helpers::{command_line, relative_to, same_path, to_os_args};

/// A failed git command, classified from its stderr (git runs with `LC_ALL=C`, so the
/// phrases are stable). Carries the concrete names needed to explain it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroveError {
    WouldOverwrite { op: String, target: Option<String>, files: Vec<String> },
    BranchInOtherWorktree { branch: String, path: PathBuf },
    MergeConflict { op: String, files: Vec<String> },
    NonFastForward { branch: Option<String> },
    NoUpstream { branch: String },
    Diverged,
    NotMerged { branch: String },
    WorktreeDirty { path: String },
    WorktreeLocked { path: String, reason: Option<String> },
    AuthFailed,
    NotARepo,
    Unknown { stderr: String },
}

/// The subcommand of an argv, skipping global options like `-C <dir>`.
fn subcommand(args: &[String]) -> String {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "-C" || a == "-c" {
            it.next();
        } else if !a.starts_with('-') {
            return a.clone();
        }
    }
    String::new()
}

/// Last positional argument after the subcommand: `switch main` → `main`.
fn last_positional(args: &[String]) -> Option<String> {
    args.iter().skip(1).rev().find(|a| !a.starts_with('-')).cloned()
}

/// The text between the first pair of single quotes after `marker`.
fn quoted_after(text: &str, marker: &str) -> Option<String> {
    let pos = text.find(marker)?;
    let before = &text[..pos];
    // the quoted name usually comes *before* the marker: "'x' is already used by …"
    let start = before.rfind('\'').and_then(|end| before[..end].rfind('\'').map(|s| (s, end)));
    start.map(|(s, e)| before[s + 1..e].to_string())
}

fn quoted_tail(text: &str, marker: &str) -> Option<String> {
    let rest = &text[text.find(marker)? + marker.len()..];
    let rest = rest.trim_start().strip_prefix('\'')?;
    Some(rest[..rest.find('\'')?].to_string())
}

impl GroveError {
    pub fn classify(args: &[String], exit: Option<i32>, output: &str) -> GroveError {
        let sub = subcommand(args);
        if output.contains("would be overwritten by") || output.contains("Your local changes to the following files would be overwritten") {
            let files = output
                .lines()
                .skip_while(|l| !l.contains("would be overwritten"))
                .skip(1)
                .take_while(|l| l.starts_with('\t') || l.starts_with("    "))
                .map(|l| l.trim().to_string())
                .collect();
            let target = matches!(sub.as_str(), "switch" | "checkout").then(|| last_positional(args)).flatten();
            return GroveError::WouldOverwrite { op: sub, target, files };
        }
        for marker in ["is already checked out at", "is already used by worktree at", "used by worktree at"] {
            if let Some(path) = quoted_tail(output, marker) {
                let branch = quoted_after(output, marker).or_else(|| last_positional(args)).unwrap_or_default();
                return GroveError::BranchInOtherWorktree { branch, path: PathBuf::from(path) };
            }
        }
        if output.contains("CONFLICT (") || output.contains("needs merge") || output.contains("you need to resolve your current index first") {
            let files = output
                .lines()
                .filter(|l| l.starts_with("CONFLICT ("))
                .filter_map(|l| {
                    l.split_once("Merge conflict in ")
                        .map(|(_, f)| f.trim().to_string())
                        .or_else(|| l.split_once("): ").map(|(_, rest)| rest.split(' ').next().unwrap_or("").to_string()))
                })
                .collect();
            let op = match sub.as_str() {
                "pull" if args.iter().any(|a| a == "--rebase" || a == "-r") => "rebase".to_string(),
                "pull" => "merge".to_string(),
                "stash" => "stash".to_string(),
                other => other.to_string(),
            };
            return GroveError::MergeConflict { op, files };
        }
        if output.contains("[rejected]") && (output.contains("non-fast-forward") || output.contains("fetch first")) {
            let branch = output
                .lines()
                .find(|l| l.contains("[rejected]"))
                .and_then(|l| l.split("->").nth(1))
                .map(|s| s.split_whitespace().next().unwrap_or("").to_string());
            return GroveError::NonFastForward { branch };
        }
        if output.contains("has no upstream branch") {
            let branch = output
                .split("The current branch ")
                .nth(1)
                .and_then(|s| s.split(" has no upstream").next())
                .unwrap_or_default()
                .to_string();
            return GroveError::NoUpstream { branch };
        }
        if output.contains("Not possible to fast-forward") || output.contains("have diverged") {
            return GroveError::Diverged;
        }
        if output.contains("is not fully merged") {
            let branch = quoted_after(output, "is not fully merged")
                .or_else(|| last_positional(args))
                .unwrap_or_default();
            return GroveError::NotMerged { branch };
        }
        if output.contains("contains modified or untracked files") {
            let path = quoted_after(output, "contains modified").or_else(|| last_positional(args)).unwrap_or_default();
            return GroveError::WorktreeDirty { path };
        }
        if output.contains("locked working tree") || (sub == "worktree" && output.contains("is locked")) {
            let reason = output
                .split("lock reason: ")
                .nth(1)
                .map(|r| r.lines().next().unwrap_or("").trim().to_string());
            return GroveError::WorktreeLocked { path: last_positional(args).unwrap_or_default(), reason };
        }
        if output.contains("Authentication failed")
            || output.contains("Permission denied (publickey")
            || output.contains("could not read Username")
            || output.contains("terminal prompts disabled")
        {
            return GroveError::AuthFailed;
        }
        if exit == Some(128) && output.contains("not a git repository") {
            return GroveError::NotARepo;
        }
        GroveError::Unknown { stderr: output.trim().to_string() }
    }

    /// Two or three words for tables: `conflict`, `rejected`, …
    pub fn kind_label(&self) -> &'static str {
        match self {
            GroveError::WouldOverwrite { .. } => "would overwrite",
            GroveError::BranchInOtherWorktree { .. } => "in other worktree",
            GroveError::MergeConflict { .. } => "conflict",
            GroveError::NonFastForward { .. } => "rejected",
            GroveError::NoUpstream { .. } => "no upstream",
            GroveError::Diverged => "diverged",
            GroveError::NotMerged { .. } => "not merged",
            GroveError::WorktreeDirty { .. } => "worktree dirty",
            GroveError::WorktreeLocked { .. } => "worktree locked",
            GroveError::AuthFailed => "auth failed",
            GroveError::NotARepo => "not a repo",
            GroveError::Unknown { .. } => "error",
        }
    }

    /// One sentence, plain language: `(before, emphasized name, after)`.
    pub fn headline(&self) -> (String, String, String) {
        let s = String::new;
        match self {
            GroveError::WouldOverwrite { target: Some(t), files, .. } => (
                "Can't switch to ".into(),
                t.clone(),
                format!(" — {} of your changes would be overwritten.", files.len()),
            ),
            GroveError::WouldOverwrite { op, files, .. } => (
                format!("Can't {op} — "),
                s(),
                format!("{} of your changes would be overwritten.", files.len()),
            ),
            GroveError::BranchInOtherWorktree { branch, .. } => {
                (s(), branch.clone(), " is checked out in another worktree.".into())
            }
            GroveError::MergeConflict { op, files } => (
                format!("{} stopped — ", capitalize(op)),
                s(),
                format!("{} file{} conflict{}.", files.len(), plural(files.len()), if files.len() == 1 { "s" } else { "" }),
            ),
            GroveError::NonFastForward { branch } => (
                "Push rejected — the remote ".into(),
                branch.clone().unwrap_or_default(),
                " has commits you don't have.".into(),
            ),
            GroveError::NoUpstream { branch } => (s(), branch.clone(), " has no upstream branch yet.".into()),
            GroveError::Diverged => ("Can't fast-forward — your branch and its upstream have diverged.".into(), s(), s()),
            GroveError::NotMerged { branch } => (s(), branch.clone(), " has commits that aren't merged anywhere.".into()),
            GroveError::WorktreeDirty { path } => ("The worktree ".into(), path.clone(), " has uncommitted work.".into()),
            GroveError::WorktreeLocked { path, .. } => ("The worktree ".into(), path.clone(), " is locked.".into()),
            GroveError::AuthFailed => ("The remote refused your credentials.".into(), s(), s()),
            GroveError::NotARepo => ("This folder isn't a git repository.".into(), s(), s()),
            GroveError::Unknown { .. } => ("Git reported an error grove doesn't recognize.".into(), s(), s()),
        }
    }

    pub fn explanation(&self) -> String {
        match self {
            GroveError::WouldOverwrite { target: Some(t), .. } => format!(
                "Git refused, so nothing was lost. These files have uncommitted edits and are different on {t}, so switching would have to replace your version:"
            ),
            GroveError::WouldOverwrite { .. } => "Git refused, so nothing was lost. These files have uncommitted edits that the operation would have to replace:".into(),
            GroveError::BranchInOtherWorktree { path, .. } => format!(
                "A branch can only be checked out in one worktree at a time, and this one is checked out in {}. Nothing changed.",
                path.to_string_lossy().replace('\\', "/")
            ),
            GroveError::MergeConflict { op, .. } if op == "stash" => "The stash was applied, but some files changed on both sides. The stash entry is kept, so nothing is lost. Resolve the files, or reset them:".into(),
            GroveError::MergeConflict { op, .. } => format!(
                "Both sides changed the same lines. The {op} is paused half-way: resolve the files below and continue, or abort to get back exactly where you were."
            ),
            GroveError::NonFastForward { .. } => "Someone pushed to this branch since you last pulled. Git won't overwrite their commits. Nothing was sent.".into(),
            GroveError::NoUpstream { .. } => "Git doesn't know which remote branch to push to. Nothing was sent.".into(),
            GroveError::Diverged => "Both your branch and its upstream have new commits, so a fast-forward is impossible. Nothing changed; pick how to combine them.".into(),
            GroveError::NotMerged { .. } => "Deleting it now would make those commits hard to find. Git refused; the branch is still there.".into(),
            GroveError::WorktreeDirty { .. } => "Removing it would delete modified or untracked files that exist nowhere else. Git refused; nothing was deleted.".into(),
            GroveError::WorktreeLocked { reason, .. } => match reason {
                Some(r) if !r.is_empty() => format!("Someone locked it to protect it from removal (reason: \"{r}\"). Nothing changed."),
                _ => "Someone locked it to protect it from removal. Nothing changed.".into(),
            },
            GroveError::AuthFailed => "Git couldn't log in to the remote, and grove disables interactive password prompts. Nothing was sent or received.".into(),
            GroveError::NotARepo => "Git needs a folder that contains (or is inside) a repository.".into(),
            GroveError::Unknown { .. } => "The raw output below is exactly what git said.".into(),
        }
    }

    /// The `?` text: why git behaves this way.
    pub fn why(&self) -> &'static str {
        match self {
            GroveError::WouldOverwrite { .. } => "Switching branches rewrites the files in your checkout to match the target branch. For files you haven't touched that's harmless. But when a file has uncommitted edits AND differs on the target, git would have to throw your edits away to write its version — and uncommitted edits exist nowhere else. So git refuses the whole switch.\n\nWays through: keep your edits somewhere (a commit, the stash, or a separate worktree where you don't need to switch at all), or explicitly discard them.",
            GroveError::BranchInOtherWorktree { .. } => "All worktrees of a repository share one set of branches. If two worktrees had the same branch checked out, a commit in one would silently change the files the other expects under it. Git prevents that by allowing each branch in only one worktree.\n\nUse the existing worktree, or remove it first.",
            GroveError::MergeConflict { .. } => "Git combines changes line by line. When both sides changed the same lines differently, it can't know which version you want, so it writes both into the file between <<<<<<< and >>>>>>> markers and stops.\n\nEdit each file to the version you want, stage it with `git add`, then continue. Or abort, which restores everything to before the operation.",
            GroveError::NonFastForward { .. } => "A push may only add commits on top of what the remote has. If the remote branch has commits that aren't in yours, accepting your push would delete them from the remote branch.\n\nIntegrate theirs first (pull with rebase or merge). Force-pushing replaces their commits — only do that when you rewrote history on purpose; --force-with-lease at least refuses if the remote changed since your last fetch.",
            GroveError::NoUpstream { .. } => "A new local branch isn't connected to any remote branch. `git push -u <remote> <branch>` creates the remote branch and remembers the connection, so plain `git push` and `git pull` work afterwards.",
            GroveError::Diverged => "Fast-forwarding just moves your branch forward. That only works when your branch has no commits the upstream lacks. Once both have new commits, they must be combined: rebase replays yours on top of theirs (linear history); merge joins them with a merge commit.",
            GroveError::NotMerged { .. } => "`git branch -d` only deletes branches whose commits are reachable elsewhere (merged into HEAD or the upstream). Otherwise the commits would become unreachable and eventually be garbage-collected. -D deletes anyway; grove records the commit so u can recreate the branch.",
            GroveError::WorktreeDirty { .. } => "Removing a worktree deletes its directory. Modified and untracked files there aren't in any commit, so git protects them. Commit or stash in that worktree first, or force the removal if you're sure.",
            GroveError::WorktreeLocked { .. } => "`git worktree lock` marks a worktree as not to be pruned or removed — for example one on a removable disk, or one a CI job is using. Unlock it first when you're sure it's no longer needed.",
            GroveError::AuthFailed => "grove runs git with GIT_TERMINAL_PROMPT=0 so a hidden password prompt can never hang it. Configure a credential helper (`git config credential.helper …`) or load your SSH key into ssh-agent, or run the command once in a normal shell where git can ask you.",
            GroveError::NotARepo => "Every git command needs a repository: a folder with a .git directory (or file, for worktrees) in it or above it.",
            GroveError::Unknown { .. } => "grove matches git's messages to explain them; this one isn't in its list yet. The raw output is git's own explanation; the journal keeps it for later.",
        }
    }

    /// Ranked ways out, safest first. Destructive ones come last.
    pub fn remedies(&self, args: &[String], snap: &Snapshot, affected: &[AffectedItem]) -> Vec<Remedy> {
        let rel = |p: &Path| relative_to(p, &snap.workdir);
        let rerun = || args.to_vec();
        match self {
            GroveError::WouldOverwrite { target: Some(t), files, .. } => {
                let mut out = Vec::new();
                if snap.other_worktree_with(t).is_none() {
                    let path = snap.suggest_worktree_path(t);
                    out.push(Remedy::new(
                        'W',
                        format!("open {t} in a worktree"),
                        Plan::git(&["worktree", "add", &rel(&path), t]).then(InternalOp::OpenWorktree(path)),
                        "your edits stay untouched right here. safest.",
                    ));
                }
                let conflicting: Vec<&str> = affected
                    .iter()
                    .filter(|a| !a.fine && !a.detail.ends_with("differs only by your branch's commits"))
                    .map(|a| a.name.as_str())
                    .collect();
                out.push(
                    Remedy::new(
                        's',
                        "stash · switch · re-apply",
                        Plan::git(&["stash", "push", "-m", &format!("grove: before switch to {t}")])
                            .and(&["switch", t])
                            .and(&["stash", "pop"]),
                        "your edits ride along in the stash and come back on top",
                    )
                    .warning((!conflicting.is_empty()).then(|| {
                        format!("pop will likely conflict in {} (both sides edited it)", conflicting.join(", "))
                    })),
                );
                let mut add = vec!["add".to_string(), "--".to_string()];
                add.extend(files.iter().cloned());
                out.push(Remedy::new(
                    'c',
                    "commit first, then switch",
                    Plan::git(&add).then(InternalOp::CommitDialog { then_switch: Some(t.clone()) }),
                    format!("opens the commit dialog with the {} file{} staged", files.len(), plural(files.len())),
                ));
                out.push(
                    Remedy::new(
                        'F',
                        "discard my edits & switch",
                        Plan::git(&["switch", "--discard-changes", t]),
                        "these edits were never committed",
                    )
                    .destructive(false),
                );
                out
            }
            GroveError::WouldOverwrite { op, .. } => vec![
                Remedy::new(
                    's',
                    format!("stash · {op} · re-apply"),
                    Plan::git(&["stash", "push", "-m", &format!("grove: before {op}")]).and(&rerun()).and(&["stash", "pop"]),
                    "your edits ride along in the stash and come back on top",
                ),
                Remedy::new('c', "commit first", Plan::internal(InternalOp::CommitDialog { then_switch: None }), "then run it again"),
            ],
            GroveError::BranchInOtherWorktree { path, .. } => {
                let mut out = vec![Remedy::enter(
                    "jump to that worktree",
                    Plan::internal(InternalOp::OpenWorktree(path.clone())),
                    "opens it in a new grove tab; no git command runs",
                )];
                let wt = snap.worktrees.iter().find(|w| same_path(&w.path, path));
                let mut remove = Remedy::new(
                    'x',
                    "remove that worktree, then retry",
                    Plan::git(&["worktree", "remove", &rel(path)]).and(&rerun()),
                    "deletes its directory; the branch stays",
                );
                if let Some(d) = wt.and_then(|w| w.dirty).filter(|d| !d.is_clean()) {
                    remove.warning = Some(format!("it has uncommitted work ({}) — git will refuse", d.describe()));
                }
                if wt.is_some_and(|w| !w.is_main) {
                    out.push(remove);
                }
                out
            }
            GroveError::MergeConflict { op, .. } if op == "stash" => vec![
                Remedy::new(
                    'o',
                    "open the conflicted files",
                    Plan::internal(InternalOp::GoTo(ScreenId::Dashboard)),
                    "resolve them in your editor, then stage with a; the stash entry stays until you drop it",
                ),
                Remedy::new(
                    'r',
                    "reset the conflicted files",
                    Plan::git(&["reset", "--merge"]),
                    "back to before the pop; the stash entry is still there",
                ),
            ],
            GroveError::MergeConflict { op, .. } => {
                let abort_op = if op == "pull" { "merge" } else { op.as_str() };
                let continue_plan = match abort_op {
                    "merge" => Plan::git(&["commit", "--no-edit"]),
                    other => Plan::git(&[other, "--continue"]),
                };
                vec![
                    Remedy::new('a', format!("abort the {abort_op}"), Plan::git(&[abort_op, "--abort"]), "everything goes back to exactly before. safest."),
                    Remedy::new(
                        'o',
                        "open the conflicted files",
                        Plan::internal(InternalOp::GoTo(ScreenId::Dashboard)),
                        "resolve them in your editor, then stage each with a",
                    ),
                    Remedy::new('c', "continue after resolving", continue_plan, "only after every conflicted file is resolved and staged"),
                ]
            }
            GroveError::NonFastForward { .. } => vec![
                Remedy::new('r', "pull --rebase, then push", Plan::git(&["pull", "--rebase"]).and(&["push"]), "puts your commits on top of theirs; may stop on conflicts"),
                Remedy::new('f', "pull --ff-only", Plan::git(&["pull", "--ff-only"]), "only works if you have no local commits; refuses otherwise"),
                Remedy::new('F', "force-with-lease", Plan::git(&["push", "--force-with-lease"]), "replaces their commits on the remote with yours")
                    .destructive(false),
            ],
            GroveError::NoUpstream { branch } => vec![Remedy::enter(
                "push and set upstream",
                Plan::git(&["push", "-u", snap.remote(), branch]),
                format!("creates {}/{branch} and tracks it from now on", snap.remote()),
            )],
            GroveError::Diverged => vec![
                Remedy::new('r', "rebase onto upstream", Plan::git(&["rebase", "@{u}"]), "replays your commits on top of theirs; linear history"),
                Remedy::new('m', "merge upstream", Plan::git(&["merge", "--no-edit", "@{u}"]), "joins both with a merge commit; nothing is rewritten"),
            ],
            GroveError::NotMerged { branch } => {
                let main = snap.main.clone().unwrap_or_else(|| "HEAD".into());
                vec![
                    Remedy::new(
                        'l',
                        "show the unmerged commits",
                        Plan::git(&["log", "--oneline", &format!("{main}..{branch}")]).with_output(),
                        format!("lists what {branch} has that {main} doesn't"),
                    ),
                    Remedy::new('F', "delete anyway", Plan::git(&["branch", "-D", branch]), "the commits become unreachable; u can recreate the branch")
                        .destructive(true),
                ]
            }
            GroveError::WorktreeDirty { path } => {
                let abs = snap.worktrees.iter().find(|w| same_path(&w.path, Path::new(path)) || rel(&w.path) == *path).map(|w| w.path.clone());
                let mut out = Vec::new();
                if let Some(abs) = abs {
                    out.push(Remedy::new('o', "open it", Plan::internal(InternalOp::OpenWorktree(abs)), "commit or stash there first"));
                }
                out.push(
                    Remedy::new('F', "remove anyway", Plan::git(&["worktree", "remove", "--force", path]), "its uncommitted files are deleted")
                        .destructive(false),
                );
                out
            }
            GroveError::WorktreeLocked { path, .. } => vec![
                Remedy::new('u', "unlock it", Plan::git(&["worktree", "unlock", path]), "then remove it normally"),
                Remedy::new('F', "remove anyway", Plan::git(&["worktree", "remove", "-f", "-f", path]), "ignores the lock and any uncommitted files")
                    .destructive(false),
            ],
            GroveError::AuthFailed => vec![Remedy::new(
                'y',
                "copy the command",
                Plan::internal(InternalOp::Copy(command_line(&to_os_args(args)))),
                "run it in a shell where your credential helper or ssh-agent can prompt",
            )],
            GroveError::NotARepo => vec![Remedy::new(
                'o',
                "open another folder",
                Plan::internal(InternalOp::Prompt(PromptKind::OpenRepo)),
                "asks for a path",
            )],
            GroveError::Unknown { stderr } => vec![
                Remedy::new('y', "copy the error", Plan::internal(InternalOp::Copy(stderr.clone())), "paste it into a search or an issue"),
                Remedy::new('e', "edit and retry", Plan::internal(InternalOp::EditRaw(args.join(" "))), "opens the : prompt with this command"),
            ],
        }
    }

    /// The Affected box for the overwrite case: every dirty file and which side differs.
    pub fn affected_files(files: &[String], target: Option<&str>, snap: &Snapshot, commits_touching: impl Fn(&str) -> usize) -> Vec<AffectedItem> {
        let mut out = Vec::new();
        for f in files {
            let mine = snap
                .status
                .changes
                .iter()
                .filter(|c| c.path.to_string_lossy().replace('\\', "/") == *f)
                .map(|c| c.line_counts())
                .find(|s| !s.is_empty())
                .map(|s| format!("yours: {s}"))
                .unwrap_or_else(|| "yours: edited".to_string());
            let theirs = match target {
                Some(t) => match commits_touching(f) {
                    0 => format!("{t}: differs only by your branch's commits"),
                    n => format!("{t}: {n} commit{} touch{} this file", plural(n), if n == 1 { "es" } else { "" }),
                },
                None => "incoming change".to_string(),
            };
            out.push(AffectedItem { marker: 'M', name: f.clone(), detail: format!("{mine}  ·  {theirs}"), fine: false });
        }
        let mut fine: Vec<String> = snap
            .status
            .changes
            .iter()
            .filter(|c| c.area != ChangeArea::Untracked)
            .map(|c| c.path.to_string_lossy().replace('\\', "/"))
            .filter(|p| !files.contains(p))
            .collect();
        fine.dedup();
        for p in fine.into_iter().take(6) {
            let detail = match target {
                Some(t) => format!("changed, but identical on {t} — would carry over fine"),
                None => "not involved".to_string(),
            };
            out.push(AffectedItem { marker: '·', name: p, detail, fine: true });
        }
        out
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::split_shell_words;

    fn classify(cmd: &str, exit: i32, out: &str) -> GroveError {
        GroveError::classify(&split_shell_words(cmd).unwrap(), Some(exit), out)
    }

    #[test]
    fn would_overwrite() {
        let e = classify("switch main", 1, include_str!("../../tests/fixtures/err_would_overwrite.txt"));
        assert_eq!(
            e,
            GroveError::WouldOverwrite { op: "switch".into(), target: Some("main".into()), files: vec!["store.rs".into()] }
        );
        assert_eq!(e.headline().1, "main");
    }

    #[test]
    fn branch_in_other_worktree() {
        let e = classify("switch feat/log-graph", 128, include_str!("../../tests/fixtures/err_in_other_worktree.txt"));
        let GroveError::BranchInOtherWorktree { branch, path } = e else { panic!("{e:?}") };
        assert_eq!(branch, "feat/log-graph");
        assert!(path.ends_with("repo.wt/log-graph"));
        let e = classify("branch -d feat/log-graph", 1, include_str!("../../tests/fixtures/err_branch_used.txt"));
        assert!(matches!(e, GroveError::BranchInOtherWorktree { .. }));
    }

    #[test]
    fn merge_conflict_reads_stdout_too() {
        let out = include_str!("../../tests/fixtures/out_merge_conflict.txt");
        let e = classify("merge conflict-a", 1, out);
        assert_eq!(e, GroveError::MergeConflict { op: "merge".into(), files: vec!["Cargo.toml".into()] });
    }

    #[test]
    fn push_failures() {
        let e = classify("push", 1, include_str!("../../tests/fixtures/err_non_ff.txt"));
        assert_eq!(e, GroveError::NonFastForward { branch: Some("main".into()) });
        let e = classify("push", 128, include_str!("../../tests/fixtures/err_no_upstream.txt"));
        assert_eq!(e, GroveError::NoUpstream { branch: "feat/journal".into() });
    }

    #[test]
    fn pull_diverged() {
        assert_eq!(classify("pull --ff-only", 128, include_str!("../../tests/fixtures/err_diverged.txt")), GroveError::Diverged);
    }

    #[test]
    fn branch_and_worktree_refusals() {
        let e = classify("branch -d unmerged-br", 1, include_str!("../../tests/fixtures/err_not_merged.txt"));
        assert_eq!(e, GroveError::NotMerged { branch: "unmerged-br".into() });
        let e = classify("worktree remove ../repo.wt/log-graph", 128, include_str!("../../tests/fixtures/err_worktree_dirty.txt"));
        assert_eq!(e, GroveError::WorktreeDirty { path: "../repo.wt/log-graph".into() });
        let e = classify("worktree remove ../repo.wt/locked", 128, include_str!("../../tests/fixtures/err_worktree_locked.txt"));
        assert_eq!(e, GroveError::WorktreeLocked { path: "../repo.wt/locked".into(), reason: Some("waiting on CI".into()) });
    }

    #[test]
    fn not_a_repo_and_unknown() {
        assert_eq!(classify("status", 128, include_str!("../../tests/fixtures/err_not_a_repo.txt")), GroveError::NotARepo);
        assert!(matches!(classify("frobnicate", 1, "git: 'frobnicate' is not a git command"), GroveError::Unknown { .. }));
    }

    #[test]
    fn overwrite_remedies_in_documented_order() {
        let e = GroveError::WouldOverwrite { op: "switch".into(), target: Some("main".into()), files: vec!["Cargo.toml".into()] };
        let snap = Snapshot { workdir: PathBuf::from("/src/grove"), main_worktree: PathBuf::from("/src/grove"), repo_name: "grove".into(), ..Default::default() };
        let affected = GroveError::affected_files(&["Cargo.toml".into()], Some("main"), &snap, |_| 1);
        let remedies = e.remedies(&split_shell_words("switch main").unwrap(), &snap, &affected);
        let labels: Vec<&str> = remedies.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["open main in a worktree", "stash · switch · re-apply", "commit first, then switch", "discard my edits & switch"]);
        assert!(remedies[1].warning.as_deref().unwrap().contains("Cargo.toml"));
        assert!(remedies[3].destructive && !remedies[3].recoverable);
        assert_eq!(remedies[0].plan.display(&snap.workdir), "git worktree add ../grove.wt/main main");
    }
}
