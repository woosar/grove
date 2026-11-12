use std::path::Path;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Action, Ctx, Effect, InternalOp, Pipeline, PipelineRun, PromptKind, StepState};
use crate::helpers::{cd_target, home_tilde, keep_visible, nav, next_dir, pad, relative_to, step, str_width, truncate, wrap_text};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Focus {
    #[default]
    List,
    Steps,
    Output,
}

/// Screen 6: saved command pipelines of this repository, and the output of the last run.
#[derive(Default)]
pub struct PipelinesScreen {
    focus: Focus,
    sel: usize,
    offset: usize,
    step_sel: usize,
    step_offset: usize,
    /// Rows scrolled up from the end of the output; 0 follows new output.
    scroll_back: usize,
    /// `total_lines` at the last draw, to hold the view still while scrolled up.
    drawn_lines: usize,
}

impl PipelinesScreen {
    pub fn select(&mut self, index: usize) {
        self.sel = index;
        self.step_sel = 0;
    }

    pub fn select_step(&mut self, index: usize) {
        self.step_sel = index;
    }

    pub fn on_run_started(&mut self) {
        self.scroll_back = 0;
        self.drawn_lines = 0;
    }

    fn selected<'a>(&self, ctx: &Ctx<'a>) -> Option<&'a Pipeline> {
        ctx.pipelines.get(self.sel.min(ctx.pipelines.len().saturating_sub(1)))
    }

    fn running(ctx: &Ctx) -> bool {
        ctx.pipeline_run.is_some_and(PipelineRun::is_running)
    }

    pub fn preview(&self, ctx: &Ctx) -> Option<Action> {
        Some(ctx.actions().run_pipeline(self.selected(ctx)?, Self::running(ctx)))
    }

    pub fn hints(&self) -> Vec<(&'static str, &'static str)> {
        match self.focus {
            Focus::List => vec![
                ("n", "new"),
                ("⏎", "run"),
                ("a", "add steps"),
                ("x", "cancel run"),
                ("R", "rename"),
                ("d", "delete"),
                ("!", "one-off command"),
                ("tab", "pane"),
                ("?", "explain"),
            ],
            Focus::Steps => vec![
                ("a", "add"),
                ("⏎", "edit"),
                ("d", "remove"),
                ("J/K", "reorder"),
                ("r", "run"),
                ("x", "cancel run"),
                ("tab", "pane"),
                ("esc", "back"),
            ],
            Focus::Output => vec![("j/k", "scroll"), ("g/G", "top / follow"), ("r", "run"), ("x", "cancel run"), ("tab", "pane"), ("esc", "back")],
        }
    }

    fn step_prompt(pipeline: &Pipeline, at: usize, replace: bool) -> Effect {
        Effect::Internal(InternalOp::Prompt(PromptKind::PipelineStep { pipeline: pipeline.clone(), at, replace }))
    }

    fn save(pipeline: Pipeline) -> Effect {
        Effect::Internal(InternalOp::SavePipeline { pipeline, next: None })
    }

    pub fn handle_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        match key.code {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::List => Focus::Steps,
                    Focus::Steps => Focus::Output,
                    Focus::Output => Focus::List,
                };
                return Some(Effect::Nothing);
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::List => Focus::Output,
                    Focus::Steps => Focus::List,
                    Focus::Output => Focus::Steps,
                };
                return Some(Effect::Nothing);
            }
            KeyCode::Esc if self.focus != Focus::List => {
                self.focus = Focus::List;
                return Some(Effect::Nothing);
            }
            KeyCode::Char('n') => return Some(Effect::Internal(InternalOp::Prompt(PromptKind::NewPipeline))),
            KeyCode::Char('x') => return Some(Effect::Internal(InternalOp::CancelPipeline)),
            KeyCode::Char('r') => return self.preview(ctx).map(|a| Effect::run(a, "r")),
            _ => {}
        }
        match self.focus {
            Focus::List => self.list_key(key, ctx),
            Focus::Steps => self.steps_key(key, ctx),
            Focus::Output => self.output_key(key),
        }
    }

    fn list_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        if let Some(s) = nav(key, self.sel, ctx.pipelines.len()) {
            if s != self.sel {
                self.step_sel = 0;
            }
            self.sel = s;
            return Some(Effect::Nothing);
        }
        let p = self.selected(ctx)?;
        match key.code {
            KeyCode::Enter => self.preview(ctx).map(|a| Effect::run(a, "⏎")),
            KeyCode::Char('a') => Some(Self::step_prompt(p, p.steps.len(), false)),
            KeyCode::Char('e') => {
                self.focus = Focus::Steps;
                Some(Effect::Nothing)
            }
            KeyCode::Char('R') => Some(Effect::Internal(InternalOp::Prompt(PromptKind::RenamePipeline(p.clone())))),
            KeyCode::Char('d') => Some(Effect::run(ctx.actions().delete_pipeline(p), "d")),
            _ => None,
        }
    }

    fn steps_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Effect> {
        let p = self.selected(ctx)?;
        let len = p.steps.len();
        if let Some(s) = nav(key, self.step_sel, len) {
            self.step_sel = s;
            return Some(Effect::Nothing);
        }
        let sel = self.step_sel.min(len.saturating_sub(1));
        match key.code {
            KeyCode::Char('a') => Some(Self::step_prompt(p, if len == 0 { 0 } else { sel + 1 }, false)),
            KeyCode::Enter | KeyCode::Char('e') if len > 0 => Some(Self::step_prompt(p, sel, true)),
            KeyCode::Char('d') if len > 0 => {
                let mut changed = p.clone();
                changed.steps.remove(sel);
                self.step_sel = sel.min(len.saturating_sub(2));
                Some(Self::save(changed))
            }
            KeyCode::Char(c @ ('J' | 'K')) if len > 1 => {
                let to = step(sel, if c == 'J' { 1 } else { -1 }, len);
                let mut changed = p.clone();
                changed.steps.swap(sel, to);
                self.step_sel = to;
                Some(Self::save(changed))
            }
            _ => None,
        }
    }

    fn output_key(&mut self, key: &KeyEvent) -> Option<Effect> {
        self.scroll_back = match key.code {
            KeyCode::Char('k') | KeyCode::Up => self.scroll_back + 1,
            KeyCode::Char('j') | KeyCode::Down => self.scroll_back.saturating_sub(1),
            KeyCode::PageUp | KeyCode::Char(' ') => self.scroll_back + 20,
            KeyCode::PageDown => self.scroll_back.saturating_sub(20),
            // clamped to the first row when drawn
            KeyCode::Char('g') | KeyCode::Home => usize::MAX / 2,
            KeyCode::Char('G') | KeyCode::End => 0,
            _ => return None,
        };
        Some(Effect::Nothing)
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        if area.width >= 100 {
            let [left, right] = Layout::horizontal([Constraint::Percentage(38), Constraint::Min(0)]).spacing(1).areas(area);
            let list_h = (ctx.pipelines.len() as u16 + 2).clamp(5, left.height / 2);
            let [list, steps] = Layout::vertical([Constraint::Length(list_h), Constraint::Min(0)]).areas(left);
            self.draw_list(f, list, ctx);
            self.draw_steps(f, steps, ctx);
            self.draw_output(f, right, ctx);
        } else {
            let [list, steps, output] =
                Layout::vertical([Constraint::Percentage(25), Constraint::Percentage(30), Constraint::Min(0)]).areas(area);
            self.draw_list(f, list, ctx);
            self.draw_steps(f, steps, ctx);
            self.draw_output(f, output, ctx);
        }
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let focused = self.focus == Focus::List;
        let block = t.pane(format!("Pipelines {} {}", g.sep, ctx.pipelines.len()), "this repo", focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        self.sel = self.sel.min(ctx.pipelines.len().saturating_sub(1));
        if ctx.pipelines.is_empty() {
            let mut lines = vec![Line::from(vec![t.key("n"), Span::styled(" record a pipeline", t.text())])];
            lines.push(Line::from(vec![t.key_dim("!"), Span::styled(" run one command", t.text())]));
            f.render_widget(Paragraph::new(lines), inner);
            return;
        }
        let h = inner.height as usize;
        keep_visible(&mut self.offset, self.sel, h);
        let mut lines = Vec::new();
        for (i, p) in ctx.pipelines.iter().enumerate().skip(self.offset).take(h) {
            let selected = i == self.sel;
            let count = format!("{} step{}", p.steps.len(), if p.steps.len() == 1 { "" } else { "s" });
            let name_w = w.saturating_sub(count.len() + 3);
            lines.push(t.row(
                vec![
                    Span::styled(format!("{} ", if selected { g.pointer } else { " " }), t.s(t.accent)),
                    Span::styled(pad(&p.name, name_w), t.text()),
                    Span::styled(format!(" {count}"), t.dimmed()),
                ],
                w,
                selected && focused,
            ));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    /// The states to show next to `p`'s steps, if the current run is of this pipeline.
    fn run_states<'a>(p: &Pipeline, ctx: &Ctx<'a>) -> Option<&'a [StepState]> {
        ctx.pipeline_run.filter(|r| r.name == p.name && r.steps == p.steps).map(|r| r.states.as_slice())
    }

    fn draw_steps(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let focused = self.focus == Focus::Steps;
        let Some(p) = self.selected(ctx) else {
            f.render_widget(t.pane("Steps", "", focused), area);
            return;
        };
        let block = t.pane(format!("Steps {} {}", g.sep, p.name), "start at the worktree root", focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let w = inner.width as usize;
        if p.steps.is_empty() {
            let line = Line::from(vec![t.key("a"), Span::styled(" add the first step", t.text())]);
            f.render_widget(Paragraph::new(line), inner);
            return;
        }
        self.step_sel = self.step_sel.min(p.steps.len() - 1);
        let h = inner.height as usize;
        keep_visible(&mut self.step_offset, self.step_sel, h);
        let states = Self::run_states(p, ctx);
        let root = crate::helpers::lexical_join(&ctx.snap.workdir, Path::new(""));
        let mut dir = root.clone();
        let mut lines = Vec::new();
        for (i, line) in p.steps.iter().enumerate() {
            let note = cd_target(line).and_then(|target| {
                dir = next_dir(&root, &dir, target);
                if !dir.is_dir() {
                    Some((format!("{} not in this worktree", g.warn), t.err))
                } else if Path::new(target).is_absolute() {
                    Some((format!("{} absolute: won't follow worktrees", g.warn), t.accent))
                } else {
                    None
                }
            });
            if i < self.step_offset || i >= self.step_offset + h {
                continue;
            }
            let (mark, color) = match states.and_then(|s| s.get(i)) {
                Some(StepState::Running) => (g.pointer, t.accent),
                Some(StepState::Done) => (g.check, t.ok),
                Some(StepState::Failed(_) | StepState::Cancelled) => (g.cross, t.err),
                _ => (" ", t.dim),
            };
            let number = format!("{:>2} ", i + 1);
            let room = w.saturating_sub(number.len() + 2);
            let note_w = note.as_ref().map_or(0, |(n, _)| str_width(n) + 2);
            let mut spans = vec![
                Span::styled(format!("{mark} "), t.s(color)),
                Span::styled(number, t.dimmed()),
                Span::styled(truncate(line, room.saturating_sub(note_w).max(room / 2)), if cd_target(line).is_some() { t.s(t.branch) } else { t.text() }),
            ];
            if let Some((text, color)) = note {
                spans.push(Span::styled(format!("  {text}"), t.s(color)));
            }
            lines.push(t.row(spans, w, focused && i == self.step_sel));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_output(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = ctx.theme;
        let g = &t.g;
        let focused = self.focus == Focus::Output;
        let Some(run) = ctx.pipeline_run else {
            let block = t.pane("Output", "", focused);
            let inner = block.inner(area);
            f.render_widget(block, area);
            let help = format!(
                "Commands run in a shell, starting at the root of the active worktree ({}). A step that is just `cd <dir>` carries over to the next steps; a bare `cd` goes back to the root. The run stops at the first step that fails.",
                home_tilde(&ctx.snap.workdir),
            );
            let lines: Vec<Line> = wrap_text(&help, inner.width as usize).into_iter().map(|l| Line::from(Span::styled(l, t.dimmed()))).collect();
            f.render_widget(Paragraph::new(lines), inner);
            return;
        };
        let (status, color) = if run.is_running() {
            (format!("{}/{}", run.done_count() + 1, run.steps.len()), t.accent)
        } else if run.failed() {
            (format!("{} failed", g.cross), t.err)
        } else if run.cancelled() {
            (format!("{} cancelled", g.cross), t.err)
        } else {
            (format!("{} done", g.check), t.ok)
        };
        let title = Line::from(vec![
            Span::styled(format!("Output {} {} {} ", g.sep, run.name, g.sep), t.text()),
            Span::styled(status, t.s(color)),
            Span::styled(format!(" {} {}s", g.sep, run.elapsed().as_secs()), t.dimmed()),
        ]);
        let hint = if crate::helpers::same_path(&run.root, &ctx.snap.workdir) { String::new() } else { format!("in {}", relative_to(&run.root, &ctx.snap.workdir)) };
        let block = t.pane(title, &hint, focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let (w, h) = (inner.width as usize, inner.height as usize);

        // hold the view still while scrolled up and new lines arrive
        let fresh = run.total_lines.saturating_sub(self.drawn_lines);
        if self.scroll_back > 0 {
            let added: usize = run.output.iter().rev().take(fresh).map(|(_, line)| line.wrap(w, Style::default()).len()).sum();
            self.scroll_back = self.scroll_back.saturating_add(added);
        }
        self.drawn_lines = run.total_lines;

        // only wrap what can be seen: walk backwards until the view is covered
        let wanted = h.saturating_add(self.scroll_back);
        let mut rows: Vec<Line> = Vec::new();
        for (header, line) in run.output.iter().rev() {
            let base = if *header { t.s(t.accent) } else { t.text() };
            rows.extend(line.wrap(w, base).into_iter().rev());
            if rows.len() >= wanted {
                break;
            }
        }
        self.scroll_back = self.scroll_back.min(rows.len().saturating_sub(h));
        let visible: Vec<Line> = rows.into_iter().skip(self.scroll_back).take(h).rev().collect();
        f.render_widget(Paragraph::new(visible), inner);
    }
}
