use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{
    Action, Branch, ChangeArea, Commit, FileChange, InternalOp, KeyBinding, Plan, PreflightInfo, PromptKind, Snapshot,
    Step, Verdict, Worktree,
};
use crate::helpers::{relative_to, short_oid};

/// How far behind main a branch may be before switching to it warns.
const FAR_BEHIND: usize = 20;

const HELP_SWITCH: &str = "git switch <branch>\n\nMoves HEAD to another branch and updates the files in your checkout to match it.\n\nUncommitted changes come along as long as they don't touch files that differ between the two branches. If they do, git refuses and nothing is lost; grove then offers ways through (stash, a separate worktree, or discarding).\n\nA branch can only be checked out in one worktree at a time. If it is checked out elsewhere, grove offers to jump to that worktree instead.\n\nUndo: switch back with `git switch -` (grove's u does this).";
const HELP_JUMP: &str = "Jump to worktree (grove only)\n\nThe branch is checked out in another worktree, so git won't check it out here as well. Jumping makes that worktree grove's active context (a new grove tab). No git command runs and nothing changes on disk.";
const HELP_WORKTREE_ADD: &str = "git worktree add <path> <branch>\n\nCreates a second checkout of the same repository in another directory. Both share history, branches and stash, but each has its own files, index and HEAD. You can build and run two branches side by side without stashing.\n\nRemove it later with `git worktree remove <path>`; the branch stays.";
const HELP_MERGE: &str = "git merge --no-ff --no-edit <branch>\n\nCreates a merge commit on the current branch that joins in <branch>'s history. --no-ff always makes the merge commit, even when a fast-forward would be possible, so the side branch stays visible in the graph.\n\nIf both sides changed the same lines, git stops with conflicts; grove shows which files and offers abort/continue.\n\nUndo: `git reset --keep <HEAD before>` (grove's u).";
const HELP_REBASE: &str = "git rebase <branch>\n\nReplays the commits of your current branch that <branch> doesn't have on top of <branch>, one by one. The result is a linear history; your commits get new ids.\n\nStops at the first conflict. You can then resolve and continue, or abort to get back exactly where you were.\n\nDon't rebase commits others have already pulled.\n\nUndo: `git reset --keep <HEAD before>` (grove's u).";
const HELP_DELETE: &str = "git branch -d <branch>\n\nDeletes the branch name. The commits stay in the repository until garbage-collected, but become hard to find.\n\n-d refuses when the branch has commits not merged into its upstream or HEAD; -D deletes anyway. A branch checked out in any worktree can't be deleted.\n\nUndo: grove records the branch's commit before deleting, so u recreates it with `git branch <name> <oid>`.";
const HELP_UPSTREAM: &str = "git branch --set-upstream-to=<remote>/<branch> <branch>\n\nTells git which remote branch this one tracks. Afterwards `git pull` and `git push` know where to go, and ahead/behind counts appear.";
const HELP_RENAME: &str = "git branch -m <old> <new>\n\nRenames a local branch. Its upstream setting and reflog move along. The remote branch (if any) keeps its old name.";
const HELP_TRACK: &str = "git switch --track <remote>/<branch>\n\nCreates a local branch with the same name that tracks the remote branch, and switches to it.";
const HELP_OPEN_WT: &str = "Open worktree (grove only)\n\nMakes that worktree grove's active context in a new grove tab. No git command runs. ctrl-tab cycles open tabs.";
const HELP_PRUNE: &str = "git worktree prune --verbose\n\nRemoves git's administrative records for worktrees whose directories no longer exist (e.g. deleted with a file manager instead of `git worktree remove`). While such a record exists its branch counts as checked out and can't be checked out anywhere else.\n\nOnly the records go; branches and commits stay.";
const HELP_LOCK: &str = "git worktree lock --reason <why> <path> / git worktree unlock <path>\n\nA locked worktree is protected from `git worktree prune` and `remove` — useful for checkouts on removable drives or long CI runs.";
const HELP_REMOVE_WT: &str = "git worktree remove <path>\n\nDeletes the worktree's directory and git's record of it. Refuses if the worktree has modified or untracked files, or is locked. The branch it had checked out stays.";
const HELP_MOVE_WT: &str = "git worktree move <path> <new-path>\n\nMoves a worktree's directory and updates git's records. The main worktree and locked worktrees can't be moved.";
const HELP_STAGE: &str = "git add -- <path> / git restore --staged -- <path>\n\nStaging puts the file's current content into the index, so the next commit includes it. Unstaging takes it back out of the index; your edits in the file are untouched.";
const HELP_COMMIT: &str = "git commit -m <message>\n\nRecords everything staged as a new commit on the current branch.\n\nUndo: `git reset --soft HEAD~1` keeps the changes staged; grove's u uses `git reset --keep <HEAD before>`.";
const HELP_DISCARD: &str = "git restore -- <path> / git clean -f -- <path>\n\nThrows away uncommitted edits to the file (or deletes the untracked file). These edits were never committed, so git can't bring them back.";
const HELP_SHOW: &str = "git show <commit>\n\nShows the commit's message and the full patch it introduced.";
const HELP_CHECKOUT: &str = "git switch --detach <commit>\n\nChecks out a commit directly instead of a branch (\"detached HEAD\"). Good for looking around or bisecting. Commits made there belong to no branch; create one with `git switch -c <name>` before leaving, or they'll be hard to find.";
const HELP_CHERRY: &str = "git cherry-pick <commit>\n\nCopies the changes of one commit onto the current branch as a new commit. The original stays where it is.\n\nStops on conflicts; abort returns to where you were.";
const HELP_REVERT: &str = "git revert --no-edit <commit>\n\nCreates a new commit that undoes the changes of <commit>. History isn't rewritten, so it's safe on shared branches.";
const HELP_RESET: &str = "git reset <mode> <commit>\n\nMoves the current branch to <commit>.\n  --soft   keeps all changes since then staged\n  --mixed  keeps them as unstaged edits\n  --keep   keeps your uncommitted edits, refuses if they'd be overwritten\n  --hard   throws away all uncommitted work — not recoverable\n\nCommits after <commit> stay reachable via the reflog for a while; grove's u can move back.";
const HELP_TAG: &str = "git tag <name> <commit>\n\nCreates a lightweight tag: a fixed name for a commit, e.g. a release. Tags aren't pushed unless you push them explicitly.";
const HELP_COPY: &str = "Copy (grove only)\n\nCopies to the clipboard via the terminal (OSC 52). No git command runs.";
const HELP_FETCH: &str = "git fetch --prune <remote>\n\nDownloads new commits and branches from the remote and updates the remote-tracking branches (origin/…). Never touches your branches or files. --prune drops remote-tracking branches deleted on the remote.";
const HELP_PULL: &str = "git pull --ff-only\n\nFetches and then fast-forwards your branch to its upstream. --ff-only refuses when both sides have new commits (diverged), instead of creating a merge commit behind your back; grove then offers rebase or merge.";
const HELP_PUSH: &str = "git push\n\nUploads your branch's new commits to its upstream. Rejected when the remote has commits you don't (someone else pushed); pull first, or force with --force-with-lease if you rewrote history on purpose.\n\nWithout an upstream, grove runs `git push -u <remote> <branch>`, which also sets it.";
const HELP_PREVIOUS: &str = "git switch -\n\nSwitches back to the branch you were on before.";
const HELP_PRUNE_MERGED: &str = "git branch -d <branches…>\n\nDeletes local branches whose commits are all in the main branch. -d still refuses anything unmerged, so nothing is lost. Branches checked out in a worktree are skipped.";
const HELP_NEW_BRANCH: &str = "git switch -c <name>\n\nCreates a branch at the current commit and switches to it. Uncommitted changes come along.";

/// Builds every action grove offers, with preflight verdicts from the current snapshot.
pub struct ActionFactory<'a> {
    snap: &'a Snapshot,
    preflight: &'a HashMap<String, PreflightInfo>,
}

impl<'a> ActionFactory<'a> {
    pub fn new(snap: &'a Snapshot, preflight: &'a HashMap<String, PreflightInfo>) -> Self {
        ActionFactory { snap, preflight }
    }

    fn rel(&self, path: &Path) -> String {
        relative_to(path, &self.snap.workdir)
    }

    fn current(&self) -> String {
        self.snap.head_label()
    }

    fn main(&self) -> &str {
        self.snap.main.as_deref().unwrap_or("main")
    }

    // ---- branches -------------------------------------------------------------------

    pub fn switch(&self, b: &Branch) -> Action {
        if b.remote {
            return self.track(b);
        }
        let mut verdict = Verdict::Ok;
        if b.is_head {
            verdict = Verdict::blocked(format!("already on {}", b.name));
        } else if let Some(wt) = self.snap.other_worktree_with(&b.name) {
            verdict = Verdict::Blocked {
                reason: format!("already checked out in {}", self.snap.short_path(&wt.path)),
                alternative: Some(KeyBinding::enter()),
            };
        } else if let Some(info) = self.preflight.get(&b.name).filter(|i| !i.overlap.is_empty()) {
            let n = info.overlap.len();
            verdict = Verdict::Warn(format!(
                "{n} change{} would be overwritten — git will refuse; grove offers ways through",
                if n == 1 { "" } else { "s" }
            ));
        } else if let Some((_, behind)) = b.vs_main.filter(|(_, behind)| *behind > FAR_BEHIND) {
            verdict = Verdict::Warn(format!("{behind} commits behind {}", self.main()));
        }
        Action::new(KeyBinding::ch('s'), "switch", Plan::git(&["switch", &b.name]))
            .explain("switches branches; never touches files you haven't changed")
            .help(HELP_SWITCH)
            .verdict(verdict)
    }

    /// The long description the quick switcher shows under the command.
    pub fn switch_detail(&self, b: &Branch) -> String {
        let name = b.local_name(&self.snap.remotes);
        let dirty = self.snap.status.modified_count() + self.snap.status.count(ChangeArea::Untracked);
        let overlap = self.preflight.get(name).map(|i| i.overlap.len());
        let changes = match (dirty, overlap) {
            (0, _) => "Your checkout is clean.".to_string(),
            (n, Some(0)) => format!(
                "Your {n} local change{} don't overlap with it, so they come along.",
                if n == 1 { "" } else { "s" }
            ),
            (n, Some(k)) => format!("{k} of your {n} local changes differ on {name}; git will refuse."),
            (n, None) => format!("You have {n} local change{}; checking overlap…", if n == 1 { "" } else { "s" }),
        };
        if b.remote {
            format!("Creates local {name} tracking {}, then moves HEAD there. {changes}", b.name)
        } else {
            format!("Moves HEAD to {name}. {changes}")
        }
    }

    pub fn jump(&self, wt: &Worktree) -> Action {
        Action::new(KeyBinding::enter(), "jump", Plan::internal(InternalOp::OpenWorktree(wt.path.clone())))
            .explain("open that worktree in a new tab instead")
            .help(HELP_JUMP)
    }

    fn track(&self, b: &Branch) -> Action {
        let local = b.local_name(&self.snap.remotes).to_string();
        let mut action = Action::new(KeyBinding::ch('s'), "switch", Plan::git(&["switch", "--track", &b.name]))
            .explain(format!("creates local {local} tracking {} and switches to it", b.name))
            .help(HELP_TRACK);
        if self.snap.branches.iter().any(|l| !l.remote && l.name == local) {
            action.verdict = Verdict::blocked(format!("local branch {local} already exists — switch to it instead"));
        }
        action
    }

    pub fn switch_and_rebase(&self, b: &Branch) -> Action {
        let name = b.local_name(&self.snap.remotes).to_string();
        let mut plan = if b.remote { Plan::git(&["switch", "--track", &b.name]) } else { Plan::git(&["switch", &name]) };
        plan = plan.and(&["rebase", self.main()]);
        let mut action = Action::new(KeyBinding::ctrl('r'), "switch + rebase", plan)
            .explain(format!("switch to {name}, then replay its commits onto {}", self.main()))
            .help(HELP_REBASE);
        action.verdict = self.switch(b).verdict;
        action
    }

    pub fn create_branch(&self, name: &str) -> Action {
        Action::new(KeyBinding::enter(), "create", Plan::git(&["switch", "-c", name]))
            .explain(format!("creates {name} at {} and switches to it", self.current()))
            .help(HELP_NEW_BRANCH)
    }

    /// `git worktree add` for an existing branch (local or remote), then open it.
    pub fn branch_worktree(&self, b: &Branch) -> Action {
        let name = b.local_name(&self.snap.remotes).to_string();
        let path = self.snap.suggest_worktree_path(&name);
        let plan = if b.remote {
            self.worktree_add_plan(&name, Some(&b.name), &path, true, Vec::new(), true)
        } else {
            self.worktree_add_plan(&name, None, &path, false, Vec::new(), true)
        };
        let mut action = Action::new(KeyBinding::ch('w'), "worktree", plan)
            .explain(format!("open {name} in a new worktree at {} — your checkout stays as it is", self.rel(&path)))
            .help(HELP_WORKTREE_ADD);
        if let Some(wt) = (!b.remote).then(|| self.snap.other_worktree_with(&b.name)).flatten() {
            action.verdict = Verdict::blocked(format!("already checked out in {}", self.snap.short_path(&wt.path)));
        } else if b.is_head && !b.remote {
            action.verdict = Verdict::blocked("checked out right here");
        }
        action
    }

    pub fn worktree_add_plan(
        &self,
        branch: &str,
        base: Option<&str>,
        path: &Path,
        new_branch: bool,
        copy: Vec<PathBuf>,
        open: bool,
    ) -> Plan {
        let rel = self.rel(path);
        let mut args: Vec<OsString> = vec!["worktree".into(), "add".into()];
        if new_branch {
            if base.is_some_and(|b| self.snap.branches.iter().any(|r| r.remote && r.name == b)) {
                args.push("--track".into());
            }
            args.extend(["-b".into(), branch.into(), rel.into()]);
            if let Some(base) = base {
                args.push(base.into());
            }
        } else {
            args.extend([rel.into(), branch.into()]);
        }
        let mut plan = Plan::git_os(args);
        if !copy.is_empty() {
            plan = plan.and_step(Step::Copy { from: self.snap.workdir.clone(), to: path.to_path_buf(), files: copy });
        }
        if open {
            plan = plan.then(InternalOp::OpenWorktree(path.to_path_buf()));
        }
        plan
    }

    pub fn branch_actions(&self, b: &Branch) -> Vec<Action> {
        if b.remote {
            return self.remote_branch_actions(b);
        }
        let mut out = vec![self.switch(b)];
        if let Some(wt) = self.snap.other_worktree_with(&b.name) {
            out.push(self.jump(wt));
        } else if !b.is_head {
            out.push(self.branch_worktree(b));
        }
        let current = self.current();
        let mut merge = Action::new(KeyBinding::ch('m'), "merge", Plan::git(&["merge", "--no-ff", "--no-edit", &b.name]))
            .explain(format!("merges {} into {current} with a merge commit", b.name))
            .note("→ into current")
            .help(HELP_MERGE);
        let head_only = self.preflight.get(&b.name).map(|i| i.head_only);
        let mut rebase = Action::new(KeyBinding::ch('r'), "rebase", Plan::git(&["rebase", &b.name]))
            .explain(format!("replays your commits on top of {}, one by one; stops at the first conflict", b.name))
            .note(match head_only {
                Some(n) => format!("replays your {n} commit{}", if n == 1 { "" } else { "s" }),
                None => "replays your commits".to_string(),
            })
            .help(HELP_REBASE);
        if b.is_head {
            merge.verdict = Verdict::blocked("can't merge a branch into itself");
            rebase.verdict = Verdict::blocked("can't rebase a branch onto itself");
        } else if self.snap.status.branch.is_none() {
            rebase.verdict = Verdict::blocked("HEAD is detached — switch to a branch first");
        } else if b.vs_main.is_some_and(|(ahead, _)| ahead == 0) && self.snap.merged.contains(&b.name) {
            merge.note = "already merged — nothing to do".to_string();
        }
        out.push(merge);
        out.push(rebase);
        out.push(self.delete_branch(b));
        out.push(self.set_upstream(b));
        out.push(
            Action::new(KeyBinding::ch('R'), "rename", Plan::internal(InternalOp::Prompt(PromptKind::RenameBranch(b.name.clone()))))
                .explain(format!("rename {} (asks for the new name)", b.name))
                .help(HELP_RENAME),
        );
        out
    }

    pub fn delete_branch(&self, b: &Branch) -> Action {
        let mut action = Action::new(KeyBinding::ch('d'), "delete", Plan::git(&["branch", "-d", &b.name]))
            .explain(format!("deletes the branch name {}; u recreates it", b.name))
            .help(HELP_DELETE);
        if b.is_head {
            action.verdict = Verdict::blocked("refused: it's the current branch");
            action.note = "refused: current branch".to_string();
        } else if let Some(wt) = self.snap.worktrees.iter().find(|w| w.branch.as_deref() == Some(&b.name)) {
            action.verdict = Verdict::blocked(format!("refused: in use by worktree {}", self.snap.short_path(&wt.path)));
            action.note = "refused: in use".to_string();
        } else if self.snap.main.as_deref() != Some(&b.name) && !self.snap.merged.contains(&b.name) && b.upstream.is_none() {
            action.verdict = Verdict::Warn(format!("not merged into {} — -d will refuse", self.main()));
        }
        action
    }

    fn set_upstream(&self, b: &Branch) -> Action {
        let candidate = format!("{}/{}", self.snap.remote(), b.name);
        let has_remote = self.snap.branches.iter().any(|r| r.remote && r.name == candidate);
        if has_remote && b.upstream.as_deref() != Some(&candidate) {
            Action::new(
                KeyBinding::ch('u'),
                "upstream",
                Plan::git(&["branch", &format!("--set-upstream-to={candidate}"), &b.name]),
            )
            .explain(format!("makes {} track {candidate}", b.name))
            .help(HELP_UPSTREAM)
        } else {
            Action::new(KeyBinding::ch('u'), "upstream", Plan::internal(InternalOp::Prompt(PromptKind::SetUpstream(b.name.clone()))))
                .explain(format!("choose the remote branch {} tracks", b.name))
                .help(HELP_UPSTREAM)
        }
    }

    fn remote_branch_actions(&self, b: &Branch) -> Vec<Action> {
        let mut track = self.track(b);
        track.key = KeyBinding::enter();
        let mut merge = Action::new(KeyBinding::ch('m'), "merge", Plan::git(&["merge", "--no-ff", "--no-edit", &b.name]))
            .explain(format!("merges {} into {}", b.name, self.current()))
            .note("→ into current")
            .help(HELP_MERGE);
        let mut rebase = Action::new(KeyBinding::ch('r'), "rebase", Plan::git(&["rebase", &b.name]))
            .explain(format!("replays your commits on top of {}", b.name))
            .help(HELP_REBASE);
        if self.snap.status.branch.is_none() {
            merge.verdict = Verdict::blocked("HEAD is detached — switch to a branch first");
            rebase.verdict = merge.verdict.clone();
        }
        vec![track, self.branch_worktree(b), merge, rebase]
    }

    pub fn previous_branch(&self) -> Action {
        Action::new(KeyBinding::ch('-'), "previous", Plan::git(&["switch", "-"]))
            .explain("switches back to the branch you were on before")
            .help(HELP_PREVIOUS)
    }

    pub fn new_branch(&self) -> Action {
        Action::new(KeyBinding::ch('n'), "new", Plan::internal(InternalOp::Prompt(PromptKind::NewBranch)))
            .explain(format!("create a branch at {} and switch to it (asks for the name)", self.current()))
            .help(HELP_NEW_BRANCH)
    }

    pub fn prune_merged(&self) -> Action {
        let names: Vec<&str> = self
            .snap
            .local_branches()
            .filter(|b| self.snap.merged.contains(&b.name))
            .filter(|b| !b.is_head && self.snap.main.as_deref() != Some(&b.name))
            .filter(|b| !self.snap.worktrees.iter().any(|w| w.branch.as_deref() == Some(&b.name)))
            .map(|b| b.name.as_str())
            .collect();
        let mut args = vec!["branch", "-d"];
        args.extend(names.iter().copied());
        let mut action = Action::new(KeyBinding::ch('D'), "prune merged", Plan::git(&args))
            .explain(format!("deletes {} branch{} already merged into {}", names.len(), if names.len() == 1 { "" } else { "es" }, self.main()))
            .help(HELP_PRUNE_MERGED);
        if names.is_empty() {
            action.verdict = Verdict::blocked(format!("no merged branches to prune (relative to {})", self.main()));
        } else {
            action.confirm = Some(format!("Delete these merged branches?\n  {}", names.join("\n  ")));
        }
        action
    }

    // ---- worktrees ------------------------------------------------------------------

    pub fn worktree_actions(&self, wt: &Worktree) -> Vec<Action> {
        let here = crate::helpers::same_path(&wt.path, &self.snap.workdir);
        let rel = self.rel(&wt.path);
        let mut open = Action::new(KeyBinding::enter(), "open", Plan::internal(InternalOp::OpenWorktree(wt.path.clone())))
            .explain("switch grove to this worktree (new tab) — no git command runs")
            .help(HELP_OPEN_WT);
        if wt.prunable.is_some() {
            open.verdict = Verdict::blocked("the directory is gone — prune it instead");
        } else if here {
            open.verdict = Verdict::blocked("you're already here");
        }

        let any_prunable = self.snap.worktrees.iter().any(|w| w.prunable.is_some());
        let mut prune = Action::new(KeyBinding::ch('p'), "prune", Plan::git(&["worktree", "prune", "--verbose"]))
            .explain("drops git's records of worktrees whose directories are gone")
            .help(HELP_PRUNE);
        if !any_prunable {
            prune.verdict = Verdict::blocked("nothing to prune — every worktree directory exists");
        }

        let mut lock = if wt.locked.is_some() {
            Action::new(KeyBinding::ch('L'), "unlock", Plan::git(&["worktree", "unlock", &rel]))
                .explain("allows prune and remove again")
                .help(HELP_LOCK)
        } else {
            Action::new(KeyBinding::ch('L'), "lock", Plan::internal(InternalOp::Prompt(PromptKind::LockWorktree(wt.path.clone()))))
                .explain("protects it from prune and remove (asks for a reason)")
                .note("protect from prune")
                .help(HELP_LOCK)
        };
        if wt.is_main {
            lock.verdict = Verdict::blocked("the main worktree can't be locked");
        }

        let mut remove = Action::new(KeyBinding::ch('x'), "remove", Plan::git(&["worktree", "remove", &rel]))
            .explain("deletes the worktree directory; its branch stays")
            .note("refuses if dirty")
            .help(HELP_REMOVE_WT);
        remove.verdict = if wt.is_main {
            Verdict::blocked("the main worktree can't be removed")
        } else if here {
            Verdict::blocked("you're in it — open another worktree first")
        } else if wt.prunable.is_some() {
            Verdict::Blocked { reason: "the directory is already gone — prune instead".into(), alternative: Some(KeyBinding::ch('p')) }
        } else if let Some(reason) = &wt.locked {
            Verdict::blocked(if reason.is_empty() { "locked".to_string() } else { format!("locked: \"{reason}\"") })
        } else if let Some(d) = wt.dirty.filter(|d| !d.is_clean()) {
            Verdict::blocked(format!("has uncommitted work: {}", d.describe()))
        } else {
            Verdict::Ok
        };

        let mut mv = Action::new(KeyBinding::ch('m'), "move", Plan::internal(InternalOp::Prompt(PromptKind::MoveWorktree(wt.path.clone()))))
            .explain("moves the worktree directory (asks for the new path)")
            .help(HELP_MOVE_WT);
        if wt.is_main {
            mv.verdict = Verdict::blocked("the main worktree can't be moved");
        } else if wt.locked.is_some() {
            mv.verdict = Verdict::blocked("locked — unlock first");
        }

        if wt.prunable.is_some() { vec![prune, open, lock, remove, mv] } else { vec![open, prune, lock, remove, mv] }
    }

    // ---- status ---------------------------------------------------------------------

    pub fn file_actions(&self, fc: &FileChange) -> Vec<Action> {
        let path = fc.path.to_string_lossy().replace('\\', "/");
        let stage = match fc.area {
            ChangeArea::Staged => Action::new(KeyBinding::ch('a'), "unstage", Plan::git(&["restore", "--staged", "--", &path]))
                .explain("takes it out of the next commit; your edits stay"),
            _ => Action::new(KeyBinding::ch('a'), "stage", Plan::git(&["add", "--", &path]))
                .explain(if fc.area == ChangeArea::Conflicted { "marks the conflict as resolved" } else { "adds it to the next commit" }),
        }
        .help(HELP_STAGE);
        let mut discard = match fc.area {
            ChangeArea::Untracked => Action::new(KeyBinding::ch('X'), "delete", Plan::git(&["clean", "-f", "--", &path]))
                .explain("deletes the untracked file — not recoverable"),
            _ => Action::new(KeyBinding::ch('X'), "discard", Plan::git(&["restore", "--", &path]))
                .explain("throws away your edits to this file — not recoverable"),
        }
        .help(HELP_DISCARD)
        .destructive()
        .confirm(format!("Discard your changes to {path}? These edits were never committed."));
        if fc.area == ChangeArea::Staged {
            discard.verdict = Verdict::Blocked { reason: "staged — unstage it first".into(), alternative: Some(KeyBinding::ch('a')) };
        }
        vec![stage, self.commit(), self.stage_all(), discard]
    }

    pub fn commit(&self) -> Action {
        let staged = self.snap.status.count(ChangeArea::Staged);
        let mut action = Action::new(KeyBinding::ch('c'), "commit", Plan::internal(InternalOp::CommitDialog { then_switch: None }))
            .explain(format!("commit {staged} staged file{} (asks for a message)", if staged == 1 { "" } else { "s" }))
            .help(HELP_COMMIT);
        if staged == 0 {
            action.verdict = Verdict::Blocked { reason: "nothing staged — press a to stage".into(), alternative: Some(KeyBinding::ch('a')) };
        }
        action
    }

    pub fn stage_all(&self) -> Action {
        Action::new(KeyBinding::ch('A'), "stage all", Plan::git(&["add", "-A"]))
            .explain("stages every change, including untracked files")
            .help(HELP_STAGE)
    }

    // ---- commits --------------------------------------------------------------------

    pub fn commit_actions(&self, c: &Commit) -> Vec<Action> {
        let short = short_oid(&c.oid).to_string();
        let current = self.current();
        let diff = Action::new(KeyBinding::enter(), "diff", Plan::git(&["show", "--stat", "--patch", &short]).with_output())
            .explain("shows the full commit and patch")
            .help(HELP_SHOW);
        let checkout = Action::new(KeyBinding::ch('c'), "checkout", Plan::git(&["switch", "--detach", &short]))
            .explain("looks at this commit directly; HEAD is detached until you switch back")
            .verdict(Verdict::Warn("detaches HEAD — commits made there belong to no branch".into()))
            .confirm(format!("Check out {short} with a detached HEAD?"))
            .help(HELP_CHECKOUT);
        let mut pick = Action::new(KeyBinding::ch('p'), "cherry-pick", Plan::git(&["cherry-pick", &short]))
            .explain(format!("copies this commit onto {current} as a new commit; the original stays where it is"))
            .help(HELP_CHERRY);
        let mut revert = Action::new(KeyBinding::ch('R'), "revert", Plan::git(&["revert", "--no-edit", &short]))
            .explain(format!("adds a commit on {current} that undoes this one"))
            .help(HELP_REVERT);
        if c.parents.len() > 1 {
            pick.verdict = Verdict::blocked("merge commit — pick the individual commits instead");
            revert.verdict = Verdict::blocked("merge commit — use `git revert -m 1` from the : prompt");
        }
        let reset = Action::new(KeyBinding::ch('x'), "reset to here", Plan::internal(InternalOp::ResetMenu(c.oid.clone())))
            .explain(format!("moves {current} to {short} (choose soft / mixed / keep)"))
            .help(HELP_RESET);
        let tag = Action::new(KeyBinding::ch('t'), "tag", Plan::internal(InternalOp::Prompt(PromptKind::Tag(c.oid.clone()))))
            .explain(format!("names {short} with a tag (asks for the name)"))
            .help(HELP_TAG);
        let copy = Action::new(KeyBinding::ch('y'), "copy hash", Plan::internal(InternalOp::Copy(c.oid.clone())))
            .explain("copies the full hash to the clipboard")
            .help(HELP_COPY);
        vec![diff, checkout, pick, revert, reset, tag, copy]
    }

    /// The `x` submenu. `--hard` sits last, red, behind a red confirm.
    pub fn reset_actions(&self, oid: &str) -> Vec<Action> {
        let short = short_oid(oid).to_string();
        let current = self.current();
        let mut keep = Action::new(KeyBinding::ch('k'), "keep", Plan::git(&["reset", "--keep", &short]))
            .explain(format!("moves {current} to {short}; keeps your uncommitted edits, refuses if they'd be lost"))
            .help(HELP_RESET);
        let soft = Action::new(KeyBinding::ch('s'), "soft", Plan::git(&["reset", "--soft", &short]))
            .explain(format!("moves {current} to {short}; changes since then stay staged"))
            .help(HELP_RESET);
        let mixed = Action::new(KeyBinding::ch('m'), "mixed", Plan::git(&["reset", "--mixed", &short]))
            .explain(format!("moves {current} to {short}; changes since then become unstaged edits"))
            .help(HELP_RESET);
        let hard = Action::new(KeyBinding::ch('H'), "hard", Plan::git(&["reset", "--hard", &short]))
            .explain("throws away all uncommitted work — not recoverable")
            .help(HELP_RESET)
            .destructive()
            .confirm(format!("Reset --hard to {short}? Uncommitted work in this worktree is lost for good."));
        if self.snap.status.branch.is_none() {
            keep.verdict = Verdict::Warn("HEAD is detached — only HEAD moves".into());
        }
        vec![keep, soft, mixed, hard]
    }

    // ---- remote ---------------------------------------------------------------------

    pub fn fetch(&self) -> Action {
        let remote = self.snap.remote().to_string();
        let mut action = Action::new(KeyBinding::ch('f'), "fetch", Plan::git(&["fetch", "--prune", &remote]))
            .explain(format!("downloads new commits from {remote}; never touches your branches or files"))
            .help(HELP_FETCH);
        if self.snap.remotes.is_empty() {
            action.verdict = Verdict::blocked("no remote configured");
        }
        action
    }

    pub fn pull(&self) -> Action {
        let st = &self.snap.status;
        let mut action = Action::new(KeyBinding::ch('p'), "pull", Plan::git(&["pull", "--ff-only"]))
            .explain("fetches and fast-forwards to the upstream; refuses to create merges behind your back")
            .help(HELP_PULL);
        action.verdict = match (&st.branch, &st.upstream) {
            (None, _) => Verdict::blocked("HEAD is detached — nothing to pull into"),
            (Some(b), None) => Verdict::blocked(format!("{b} has no upstream — set one with u on the Branches screen")),
            _ if st.ahead > 0 && st.behind > 0 => Verdict::Warn(format!(
                "diverged: {} local and {} upstream commits — --ff-only will refuse",
                st.ahead, st.behind
            )),
            _ => Verdict::Ok,
        };
        action
    }

    pub fn push(&self) -> Action {
        let st = &self.snap.status;
        let remote = self.snap.remote().to_string();
        let Some(branch) = st.branch.clone() else {
            return Action::new(KeyBinding::ch('P'), "push", Plan::git(&["push"]))
                .help(HELP_PUSH)
                .verdict(Verdict::blocked("HEAD is detached — nothing to push"));
        };
        if st.upstream.is_none() {
            let mut action = Action::new(KeyBinding::ch('P'), "push", Plan::git(&["push", "-u", &remote, &branch]))
                .explain(format!("uploads {branch} and sets {remote}/{branch} as its upstream"))
                .help(HELP_PUSH)
                .verdict(Verdict::Warn(format!("no upstream yet — -u sets {remote}/{branch}")));
            if self.snap.remotes.is_empty() {
                action.verdict = Verdict::blocked("no remote configured");
            }
            return action;
        }
        let mut action = Action::new(KeyBinding::ch('P'), "push", Plan::git(&["push"]))
            .explain(format!("uploads {} commit{} to {}", st.ahead, if st.ahead == 1 { "" } else { "s" }, st.upstream.as_deref().unwrap_or("")))
            .help(HELP_PUSH);
        if st.behind > 0 {
            action.verdict = Verdict::Warn(format!("upstream has {} commit{} you don't — push will be rejected", st.behind, if st.behind == 1 { "" } else { "s" }));
        } else if st.ahead == 0 {
            action.verdict = Verdict::Warn("nothing to push — already up to date".into());
        }
        action
    }
}
