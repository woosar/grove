use super::RefKind;

/// One ref decoration of a commit (`HEAD → main`, `origin/main`, `v0.3.0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

impl RefLabel {
    /// Parses `%D`: `HEAD -> feat/x, origin/main, tag: v0.3.0, main`.
    pub fn parse_decorations(decorations: &str, remotes: &[String]) -> Vec<RefLabel> {
        decorations
            .trim()
            .split(", ")
            .filter(|d| !d.is_empty() && !d.ends_with("/HEAD"))
            .map(|d| {
                if let Some(branch) = d.strip_prefix("HEAD -> ") {
                    RefLabel { name: branch.to_string(), kind: RefKind::Head }
                } else if d == "HEAD" {
                    RefLabel { name: "HEAD".to_string(), kind: RefKind::Head }
                } else if let Some(tag) = d.strip_prefix("tag: ") {
                    RefLabel { name: tag.to_string(), kind: RefKind::Tag }
                } else if remotes.iter().any(|r| d.starts_with(&format!("{r}/"))) {
                    RefLabel { name: d.to_string(), kind: RefKind::Remote }
                } else {
                    RefLabel { name: d.to_string(), kind: RefKind::Local }
                }
            })
            .collect()
    }
}
