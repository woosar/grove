use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{Branch, Commit, DirtySummary, GitOutput, GraphRow, LaneAllocator, RepoStatus, Worktree};
use crate::helpers::{branch_slug, lossy, parse_numstat_z, same_path, short_path};

/// Everything grove knows about the active worktree at one point in time.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Top level of the active worktree.
    pub workdir: PathBuf,
    /// The common git dir; groups all worktrees of one repo in the journal.
    pub common_dir: PathBuf,
    pub main_worktree: PathBuf,
    pub repo_name: String,
    pub status: RepoStatus,
    /// Local and remote branches, most recently committed first.
    pub branches: Vec<Branch>,
    pub worktrees: Vec<Worktree>,
    /// Local branches fully merged into the main branch.
    pub merged: HashSet<String>,
    pub main: Option<String>,
    pub remotes: Vec<String>,
    pub recent: Vec<Commit>,
    pub recent_graph: Vec<GraphRow>,
}

impl Snapshot {
    /// `(toplevel, common git dir)` of the repository containing `cwd`.
    pub fn locate(cwd: &Path) -> Result<(PathBuf, PathBuf), GitOutput> {
        let args = crate::helpers::to_os_args(&["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-common-dir"]);
        let out = GitOutput::run(&args, cwd);
        if !out.ok() {
            return Err(out);
        }
        let text = out.stdout_text();
        let mut lines = text.lines();
        let top = PathBuf::from(lines.next().unwrap_or_default());
        let common = PathBuf::from(lines.next().unwrap_or_default());
        Ok((top, common))
    }

    pub fn load(cwd: &Path) -> Result<Snapshot> {
        let (workdir, common_dir) = Self::locate(cwd)
            .map_err(|o| anyhow::anyhow!("{}", o.stderr_text().trim()))?;
        let wd = workdir.as_path();

        let raw_status = GitOutput::read(wd, &["status", "--porcelain=v2", "--branch", "-z"])?;
        let unstaged = parse_numstat_z(&GitOutput::read(wd, &["diff", "--numstat", "-z"]).unwrap_or_default());
        let staged = parse_numstat_z(&GitOutput::read(wd, &["diff", "--cached", "--numstat", "-z"]).unwrap_or_default());
        let status = RepoStatus::parse(&raw_status, &staged, &unstaged);

        let remotes: Vec<String> = lossy(&GitOutput::read(wd, &["remote"]).unwrap_or_default())
            .lines()
            .map(str::to_string)
            .collect();
        let main = Self::detect_main(wd, &remotes);

        let refs = ["for-each-ref", "--sort=-committerdate", "", "refs/heads", "refs/remotes"];
        let mut branches = None;
        if let Some(m) = &main {
            let mut args = refs;
            let fmt = Branch::format(Some(m));
            args[2] = &fmt;
            // ahead-behind needs git ≥ 2.41; fall back below when it is refused
            branches = GitOutput::read(wd, &args).ok().map(|b| Branch::parse_for_each_ref(&b));
        }
        let branches = match branches {
            Some(b) => b,
            None => {
                let mut args = refs;
                let fmt = Branch::format(None);
                args[2] = &fmt;
                Branch::parse_for_each_ref(&GitOutput::read(wd, &args).context("reading branches")?)
            }
        };

        let merged: HashSet<String> = match &main {
            Some(m) => lossy(
                &GitOutput::read(wd, &["for-each-ref", &format!("--merged={m}"), "--format=%(refname:short)", "refs/heads"])
                    .unwrap_or_default(),
            )
            .lines()
            .map(str::to_string)
            .collect(),
            None => HashSet::new(),
        };

        let mut worktrees = Worktree::list(wd)?;
        for wt in &mut worktrees {
            if wt.bare || wt.prunable.is_some() || !wt.path.is_dir() {
                continue;
            }
            wt.dirty = GitOutput::read(&wt.path, &["status", "--porcelain=v2", "-z"])
                .ok()
                .map(|b| DirtySummary::from_porcelain_v2(&b));
        }
        let main_worktree = worktrees.first().map(|w| w.path.clone()).unwrap_or_else(|| workdir.clone());
        let repo_name = main_worktree
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string());

        let recent = match status.oid {
            Some(_) => Commit::parse_log(
                &GitOutput::read(wd, &["log", "--all", "--topo-order", "-z", Commit::FORMAT, "-n", "80"]).unwrap_or_default(),
                &remotes,
            ),
            None => Vec::new(),
        };
        let mut lanes = LaneAllocator::new();
        let recent_graph = recent.iter().map(|c| lanes.push(&c.oid, &c.parents)).collect();

        Ok(Snapshot {
            workdir,
            common_dir,
            main_worktree,
            repo_name,
            status,
            branches,
            worktrees,
            merged,
            main,
            remotes,
            recent,
            recent_graph,
        })
    }

    fn detect_main(wd: &Path, remotes: &[String]) -> Option<String> {
        let exists = |name: &str| {
            GitOutput::read(wd, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{name}")]).is_ok()
        };
        for remote in remotes {
            if let Ok(out) = GitOutput::read(wd, &["symbolic-ref", "--short", "-q", &format!("refs/remotes/{remote}/HEAD")]) {
                let full = lossy(&out).trim().to_string();
                if let Some(name) = full.strip_prefix(&format!("{remote}/"))
                    && exists(name)
                {
                    return Some(name.to_string());
                }
            }
        }
        ["main", "master", "trunk"].into_iter().find(|n| exists(n)).map(str::to_string)
    }

    /// The cheap poll: raw `git status` output, compared between ticks.
    pub fn probe(cwd: &Path) -> Vec<u8> {
        GitOutput::read(cwd, &["status", "--porcelain=v2", "--branch", "-z"]).unwrap_or_default()
    }

    pub fn current_branch(&self) -> Option<&str> {
        self.status.branch.as_deref()
    }

    pub fn head_label(&self) -> String {
        match (&self.status.branch, &self.status.oid) {
            (Some(b), _) => b.clone(),
            (None, Some(oid)) => format!("detached at {}", &oid[..oid.len().min(7)]),
            (None, None) => "no commits yet".to_string(),
        }
    }

    pub fn local_branches(&self) -> impl Iterator<Item = &Branch> {
        self.branches.iter().filter(|b| !b.remote)
    }

    /// Remote branches with no local branch of the same name.
    pub fn remote_only(&self) -> impl Iterator<Item = &Branch> {
        self.branches
            .iter()
            .filter(|b| b.remote && !self.branches.iter().any(|l| !l.remote && l.name == b.local_name(&self.remotes)))
    }

    /// Another worktree that has `branch` checked out.
    pub fn other_worktree_with(&self, branch: &str) -> Option<&Worktree> {
        self.worktrees
            .iter()
            .find(|w| w.branch.as_deref() == Some(branch) && !same_path(&w.path, &self.workdir))
    }

    pub fn remote(&self) -> &str {
        self.remotes
            .iter()
            .find(|r| *r == "origin")
            .or(self.remotes.first())
            .map(String::as_str)
            .unwrap_or("origin")
    }

    pub fn short_path(&self, path: &Path) -> String {
        short_path(path, &self.main_worktree)
    }

    /// `../<repo>.wt/<branch-slug>` next to the main checkout.
    pub fn suggest_worktree_path(&self, branch: &str) -> PathBuf {
        let parent = self.main_worktree.parent().map(Path::to_path_buf).unwrap_or_default();
        let base = parent.join(format!("{}.wt", self.repo_name));
        let slug = branch_slug(branch);
        let mut candidate = base.join(&slug);
        let mut n = 2;
        while candidate.exists() || self.worktrees.iter().any(|w| same_path(&w.path, &candidate)) {
            candidate = base.join(format!("{slug}-{n}"));
            n += 1;
        }
        candidate
    }
}
