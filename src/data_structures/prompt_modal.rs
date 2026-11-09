use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::{Action, Ctx, Effect, InternalOp, KeyBinding, Pipeline, Plan, PromptKind, TextInput};
use crate::helpers::{cd_target, centered, home_tilde, lexical_join, next_dir, relative_to, split_shell_words, truncate, wrap_text};

/// One-line input whose result becomes a git command (shown live) or a filter.
pub struct PromptModal {
    pub kind: PromptKind,
    pub input: TextInput,
}

impl PromptModal {
    pub fn new(kind: PromptKind, prefill: impl Into<String>) -> Self {
        PromptModal { kind, input: TextInput::new(prefill) }
    }

    /// The action the current input would run; `Ok(None)` for prompts that only
    /// set something in grove (filters, search).
    pub fn action(&self, ctx: &Ctx) -> Result<Option<Action>, String> {
        let v = self.input.value.trim();
        let rel = |p: &PathBuf| relative_to(p, &ctx.snap.workdir);
        let need = |what: &str| if v.is_empty() { Err(format!("type {what}")) } else { Ok(()) };
        let action = match &self.kind {
            PromptKind::Raw => {
                let mut words = split_shell_words(v)?;
                if words.first().is_some_and(|w| w == "git") {
                    words.remove(0);
                }
                if words.is_empty() {
                    return Err("type a git command".into());
                }
                Action::new(KeyBinding::ch(':'), "raw git", Plan::git(&words).with_output())
                    .explain("runs exactly this; output is shown afterwards")
            }
            PromptKind::Commit { then_switch } => {
                need("a commit message")?;
                let mut plan = Plan::git(&["commit", "-m", v]);
                let mut explain = "records the staged changes as a new commit".to_string();
                if let Some(t) = then_switch {
                    plan = plan.and(&["switch", t]);
                    explain = format!("commits, then switches to {t}");
                }
                Action::new(KeyBinding::ch('c'), "commit", plan).explain(explain)
            }
            PromptKind::NewBranch => {
                need("a branch name")?;
                ctx.actions().create_branch(v)
            }
            PromptKind::RenameBranch(old) => {
                need("the new name")?;
                Action::new(KeyBinding::ch('R'), "rename", Plan::git(&["branch", "-m", old, v]))
                    .explain(format!("renames {old} to {v}; its upstream setting moves along"))
            }
            PromptKind::SetUpstream(branch) => {
                need("a remote branch, e.g. origin/main")?;
                Action::new(KeyBinding::ch('u'), "upstream", Plan::git(&["branch", &format!("--set-upstream-to={v}"), branch]))
                    .explain(format!("makes {branch} track {v}"))
            }
            PromptKind::Tag(oid) => {
                need("a tag name")?;
                Action::new(KeyBinding::ch('t'), "tag", Plan::git(&["tag", v, &oid[..oid.len().min(7)]]))
                    .explain("creates a lightweight tag; push it explicitly to share it")
            }
            PromptKind::MoveWorktree(path) => {
                need("the new path")?;
                Action::new(KeyBinding::ch('m'), "move", Plan::git(&["worktree", "move", &rel(path), v]))
                    .explain("moves the directory and updates git's records")
            }
            PromptKind::LockWorktree(path) => {
                let plan = if v.is_empty() {
                    Plan::git(&["worktree", "lock", &rel(path)])
                } else {
                    Plan::git(&["worktree", "lock", "--reason", v, &rel(path)])
                };
                Action::new(KeyBinding::ch('L'), "lock", plan).explain("protects it from prune and remove")
            }
            PromptKind::LogFilter
            | PromptKind::JournalSearch
            | PromptKind::OpenRepo
            | PromptKind::Shell
            | PromptKind::NewPipeline
            | PromptKind::RenamePipeline(_)
            | PromptKind::PipelineStep { .. } => return Ok(None),
        };
        Ok(Some(action))
    }

    pub fn preview(&self, ctx: &Ctx) -> Option<Action> {
        self.action(ctx).ok().flatten()
    }

    /// The line under inputs that don't turn into a git command.
    fn help(&self, ctx: &Ctx) -> String {
        let root = &ctx.snap.workdir;
        match &self.kind {
            PromptKind::LogFilter => "tokens: path:<dir>  author:<name>  since:2w  code:<text>  — anything else searches messages".to_string(),
            PromptKind::JournalSearch => "matches command lines, notes and stderr".to_string(),
            PromptKind::Shell => format!("runs in the worktree root, {}", home_tilde(root)),
            PromptKind::NewPipeline => "a name for this repo's pipeline, e.g. rebuild; steps are recorded next".to_string(),
            PromptKind::RenamePipeline(_) => "the new name".to_string(),
            PromptKind::PipelineStep { pipeline, at, replace } => {
                let dir = pipeline.dir_at(root, *at);
                let target = cd_target(&self.input.value);
                let missing = target.map(|t| next_dir(&lexical_join(root, Path::new("")), &dir, t)).filter(|d| !d.is_dir());
                let finish = if *replace { "" } else { "  Leave it empty and press enter to finish." };
                match missing {
                    Some(d) => format!("{} isn't a directory in this worktree.{finish}", relative_to(&d, root)),
                    None if target.is_some() => format!("moves the following steps; a bare cd goes back to the root.{finish}"),
                    None => format!("runs in {}.{finish}", relative_to(&dir, root)),
                }
            }
            _ => "a folder inside a git repository".to_string(),
        }
    }

    fn submit(&self, ctx: &Ctx) -> Option<Effect> {
        let v = self.input.value.trim().to_string();
        match &self.kind {
            PromptKind::LogFilter => return Some(Effect::close_then(Effect::SetLogFilter(v))),
            PromptKind::JournalSearch => return Some(Effect::close_then(Effect::SetJournalSearch(v))),
            PromptKind::OpenRepo if !v.is_empty() => {
                return Some(Effect::close_then(Effect::Internal(InternalOp::OpenWorktree(PathBuf::from(v)))));
            }
            PromptKind::Shell if !v.is_empty() => {
                return Some(Effect::close_then(Effect::Internal(InternalOp::RunShell(v))));
            }
            PromptKind::NewPipeline if !v.is_empty() => {
                return Some(Effect::close_then(Effect::Internal(InternalOp::CreatePipeline(v))));
            }
            PromptKind::RenamePipeline(p) if !v.is_empty() => {
                let pipeline = Pipeline { name: v, ..p.clone() };
                return Some(Effect::close_then(Effect::Internal(InternalOp::SavePipeline { pipeline, next: None })));
            }
            PromptKind::PipelineStep { pipeline, at, replace } if !v.is_empty() => {
                let mut pipeline = pipeline.clone();
                if *replace {
                    pipeline.steps[*at] = v;
                } else {
                    pipeline.steps.insert(*at, v);
                }
                let next = (!*replace).then_some(*at + 1);
                return Some(Effect::close_then(Effect::Internal(InternalOp::SavePipeline { pipeline, next })));
            }
            _ => {}
        }
        match self.action(ctx) {
            Ok(Some(a)) => {
                let via = a.key.label(&ctx.theme.g);
                Some(Effect::close_then(Effect::run(a, via)))
            }
            Ok(None) => Some(Effect::Close),
            Err(e) => Some(Effect::Message(e)),
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        match key.code {
            KeyCode::Esc => Some(Effect::Close),
            KeyCode::Enter => self.submit(ctx),
            _ => {
                self.input.handle(key);
                None
            }
        }
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let w = area.width.saturating_sub(8).min(100);
        let inner_w = w.saturating_sub(4) as usize;
        let prefix = if self.kind == PromptKind::Raw { "git " } else { "" };
        let mut input_line = self.input.line(t.text(), true);
        input_line.spans.insert(0, Span::styled(format!("{} {prefix}", t.g.prompt), t.s(t.accent)));
        let mut lines = vec![input_line, Line::raw("")];
        match self.action(ctx) {
            Ok(Some(a)) => {
                for l in wrap_text(&a.plan.display(&ctx.snap.workdir), inner_w) {
                    lines.push(Line::from(Span::styled(l, t.s(t.accent))));
                }
                lines.push(Line::from(Span::styled(truncate(&a.explain, inner_w), t.dimmed())));
            }
            Ok(None) => {
                for l in wrap_text(&self.help(ctx), inner_w) {
                    lines.push(Line::from(Span::styled(l, t.dimmed())));
                }
            }
            Err(e) => lines.push(Line::from(Span::styled(e, t.dimmed()))),
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![t.key(t.g.enter), Span::styled(" ok   ", t.text()), t.key_dim("esc"), Span::styled(" cancel", t.text())]));
        let rect = centered(area, w, lines.len() as u16 + 2);
        f.render_widget(Clear, rect);
        let block = t.modal(Line::from(self.kind.title()).style(t.s(t.accent)), "", t.accent);
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        f.render_widget(Paragraph::new(lines), inner);
    }
}
