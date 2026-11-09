use std::path::{Path, PathBuf};

use crate::helpers::{cd_target, lexical_join, next_dir};

/// A saved list of shell command lines, shared by all worktrees of a repository. Steps
/// start in the root of whichever worktree runs them, so relative paths follow along.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pipeline {
    pub id: i64,
    pub name: String,
    pub steps: Vec<String>,
}

impl Pipeline {
    /// The steps as one line, the way a shell would read them.
    pub fn display(&self) -> String {
        self.steps.join(" && ")
    }

    /// The directory step `at` runs in, following the `cd` steps before it.
    pub fn dir_at(&self, root: &Path, at: usize) -> PathBuf {
        let root = lexical_join(root, Path::new(""));
        let mut dir = root.clone();
        for line in self.steps.iter().take(at) {
            if let Some(target) = cd_target(line) {
                dir = next_dir(&root, &dir, target);
            }
        }
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directories_follow_cd_steps() {
        let root = std::env::temp_dir().join("wt");
        let p = Pipeline {
            steps: ["cd a/b", "dotnet build", "cd ../c", "pnpm install", "cd", "ls"].map(String::from).to_vec(),
            ..Default::default()
        };
        assert_eq!(p.dir_at(&root, 0), root);
        assert_eq!(p.dir_at(&root, 2), root.join("a").join("b"));
        assert_eq!(p.dir_at(&root, 4), root.join("a").join("c"));
        assert_eq!(p.dir_at(&root, 5), root);
    }
}
