/// The Log pane's filter line: `path:src/ author:woosar since:2w free text`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogFilter {
    pub path: Option<String>,
    pub author: Option<String>,
    pub since: Option<String>,
    /// `code:<text>` → `-S<text>` (commits that add or remove that text).
    pub pickaxe: Option<String>,
    /// Free text → `--grep` on the message.
    pub text: Option<String>,
}

impl LogFilter {
    pub fn parse(input: &str) -> LogFilter {
        let mut f = LogFilter::default();
        let mut free = Vec::new();
        for word in input.split_whitespace() {
            let set = |slot: &mut Option<String>, v: &str| {
                if !v.is_empty() && v != "any" {
                    *slot = Some(v.to_string());
                }
            };
            if let Some(v) = word.strip_prefix("path:") {
                set(&mut f.path, v);
            } else if let Some(v) = word.strip_prefix("author:") {
                set(&mut f.author, v);
            } else if let Some(v) = word.strip_prefix("since:") {
                set(&mut f.since, v);
            } else if let Some(v) = word.strip_prefix("code:") {
                set(&mut f.pickaxe, v);
            } else {
                free.push(word);
            }
        }
        if !free.is_empty() {
            f.text = Some(free.join(" "));
        }
        f
    }

    pub fn to_input(&self) -> String {
        let mut parts = Vec::new();
        if let Some(p) = &self.path {
            parts.push(format!("path:{p}"));
        }
        if let Some(a) = &self.author {
            parts.push(format!("author:{a}"));
        }
        if let Some(s) = &self.since {
            parts.push(format!("since:{s}"));
        }
        if let Some(c) = &self.pickaxe {
            parts.push(format!("code:{c}"));
        }
        if let Some(t) = &self.text {
            parts.push(t.clone());
        }
        parts.join(" ")
    }

    /// Revision-limiting args shared by `git log` and `git rev-list --count`.
    /// `since:2w` is passed as `--since=2.weeks.ago`-style approxidate.
    pub fn rev_args(&self, all_branches: bool) -> Vec<String> {
        let mut args = Vec::new();
        if all_branches {
            args.push("--all".to_string());
        }
        if let Some(a) = &self.author {
            args.push(format!("--author={a}"));
        }
        if let Some(s) = &self.since {
            args.push(format!("--since={}", Self::approxidate(s)));
        }
        if let Some(t) = &self.text {
            args.push(format!("--grep={t}"));
            args.push("-i".to_string());
        }
        if let Some(c) = &self.pickaxe {
            args.push(format!("-S{c}"));
        }
        if !all_branches {
            args.push("HEAD".to_string());
        }
        if let Some(p) = &self.path {
            args.push("--".to_string());
            args.push(p.clone());
        }
        args
    }

    fn approxidate(s: &str) -> String {
        let (num, unit): (String, String) = s.chars().partition(|c| c.is_ascii_digit());
        let unit = match unit.as_str() {
            "d" => "days",
            "w" => "weeks",
            "m" | "mo" => "months",
            "y" => "years",
            "h" => "hours",
            _ => return s.to_string(),
        };
        if num.is_empty() { s.to_string() } else { format!("{num}.{unit}.ago") }
    }

    pub fn is_empty(&self) -> bool {
        *self == LogFilter::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tokens_and_free_text() {
        let f = LogFilter::parse("path:src/ author:any since:2w lane alloc");
        assert_eq!(f.path.as_deref(), Some("src/"));
        assert_eq!(f.author, None);
        assert_eq!(f.text.as_deref(), Some("lane alloc"));
        assert_eq!(
            f.rev_args(true),
            ["--all", "--since=2.weeks.ago", "--grep=lane alloc", "-i", "--", "src/"]
        );
        assert_eq!(LogFilter::parse(&f.to_input()), f);
    }
}
