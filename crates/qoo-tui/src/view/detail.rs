use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use crate::app::{App, PaneId};
use crate::detail::{
    DetailContext, bottom_anchored, clamp_sub_tab, derive_context, sub_tab_names, window_lines,
};
use crate::hit::{HitMap, HitTarget};
use crate::ipc::types::{TaskDefinition, TaskStatus};
use crate::markup::style_line;
use crate::selectors::{WtState, arg_summary, prompt_summary};
use crate::view::Computed;
use crate::view::theme::{
    GLYPH_DONE, GLYPH_FAILED, GLYPH_NEEDS_INPUT, GLYPH_QUEUED, GLYPH_RUNNING, Palette,
};

fn status_glyph(s: &TaskStatus) -> char {
    match s {
        TaskStatus::Running => GLYPH_RUNNING,
        TaskStatus::Queued => GLYPH_QUEUED,
        TaskStatus::NeedsInput => GLYPH_NEEDS_INPUT,
        TaskStatus::Done => GLYPH_DONE,
        TaskStatus::Failed | TaskStatus::Unknown => GLYPH_FAILED,
    }
}

fn config_lines(def: &TaskDefinition) -> Vec<String> {
    vec![
        format!(
            "args: {}",
            if def.args.is_empty() { "—".to_string() } else { arg_summary(&def.args) }
        ),
        format!("worktree: {}", def.worktree),
        format!("dedup: {}", def.dedup),
        format!("model: {}", def.model),
        format!("timeout: {}ms", def.timeout_ms),
        format!("priority: {}", def.priority),
        format!(
            "discovery: {}",
            def.discovery
                .as_ref()
                .map(|d| d.command.clone())
                .unwrap_or_else(|| "—".to_string())
        ),
    ]
}

/// Content lines + placeholder for the given context/sub-tab. `def` is the
/// resolved full definition (None while loading), `run_files` the current run's
/// (report, transcript_tail).
pub(crate) fn content_for(
    ctx: &DetailContext,
    sub_tab: usize,
    def: Option<&TaskDefinition>,
    run_files: Option<&crate::runfiles::RunFiles>,
) -> (Vec<String>, &'static str) {
    match ctx {
        DetailContext::Run { task } => match sub_tab {
            1 => (run_files.map(|f| f.report.clone()).unwrap_or_default(), "(no report yet)"),
            2 => (task.prompt.split('\n').map(str::to_string).collect(), "(no prompt)"),
            _ => (
                run_files.map(|f| f.transcript_tail.clone()).unwrap_or_default(),
                "(no transcript yet)",
            ),
        },
        DetailContext::Definition { .. } => match def {
            None => (Vec::new(), "(loading definition…)"),
            Some(d) if sub_tab == 1 => (config_lines(d), ""),
            Some(d) => (d.prompt.split('\n').map(str::to_string).collect(), "(no prompt)"),
        },
        DetailContext::Worktree { row, lane_tasks } => {
            let mut lines = vec![
                format!("path: {}", row.path),
                format!(
                    "branch: {}",
                    if row.branch.is_empty() { "—".to_string() } else { row.branch.clone() }
                ),
                format!(
                    "state: {}",
                    match row.state {
                        WtState::Free => "free",
                        WtState::Busy => "busy",
                        WtState::You => "you",
                        WtState::Failed => "failed",
                    }
                ),
                String::new(),
                "tasks on this lane:".to_string(),
            ];
            if lane_tasks.is_empty() {
                lines.push("(none)".to_string());
            } else {
                for t in lane_tasks {
                    lines.push(format!("{} {}", status_glyph(&t.status), prompt_summary(&t.prompt)));
                }
            }
            (lines, "")
        }
        DetailContext::Empty => (Vec::new(), "(nothing selected)"),
    }
}

/// Total content lines of the current detail view — the drag math's scrollable
/// extent. Recomputes the same context/content the renderer uses.
pub(crate) fn detail_content_len(app: &crate::app::App) -> usize {
    let c = crate::view::compute(app);
    let ctx = match (&app.snapshot, &c.active_name) {
        (Some(snap), Some(name)) => derive_context(
            snap, name, c.ui.last_list_pane, &c.queue, &c.worktrees, &c.defs, &c.ui.selections,
        ),
        _ => DetailContext::Empty,
    };
    let kind = ctx.kind();
    let sub = clamp_sub_tab(c.ui.sub_tab[kind as usize], kind);
    let def = if let DetailContext::Definition { repo, name } = &ctx {
        app.full_defs.get(&format!("{repo}/{name}")).cloned()
    } else {
        None
    };
    let run_files = match &ctx {
        DetailContext::Run { task } => app
            .run_files
            .as_ref()
            .filter(|(id, _)| id == &task.id)
            .map(|(_, f)| f),
        _ => None,
    };
    content_for(&ctx, sub, def.as_ref(), run_files).0.len()
}

pub fn render(app: &App, c: &Computed, frame: &mut ratatui::Frame, area: Rect, hits: &mut HitMap) {
    let p: &Palette = &c.palette;
    let focused = matches!(c.ui.focus, PaneId::Detail);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(p.border_style(focused))
        .title(Span::styled("DETAIL", Style::default().fg(p.fg).add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    hits.push(inner, HitTarget::PaneBody(PaneId::Detail));
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // Resolve context from the last-focused list pane.
    let ctx = match (&app.snapshot, &c.active_name) {
        (Some(snap), Some(name)) => derive_context(
            snap,
            name,
            c.ui.last_list_pane,
            &c.queue,
            &c.worktrees,
            &c.defs,
            &c.ui.selections,
        ),
        _ => DetailContext::Empty,
    };
    let kind = ctx.kind();
    let sub_tab = clamp_sub_tab(c.ui.sub_tab[kind as usize], kind);

    // Sub-tab chip row.
    let tabs = sub_tab_names(kind);
    let mut content_top = inner.y;
    if !tabs.is_empty() {
        let mut x = inner.x;
        let mut spans: Vec<Span> = Vec::new();
        for (i, label) in tabs.iter().enumerate() {
            let chip = format!(" {}:{} ", i + 1, label);
            let w = chip.chars().count() as u16;
            let style = if i == sub_tab {
                Style::default().fg(p.selection_fg).bg(p.accent).add_modifier(Modifier::BOLD)
            } else {
                p.dim_style()
            };
            if x < inner.right() {
                hits.push(
                    Rect { x, y: inner.y, width: w.min(inner.right() - x), height: 1 },
                    HitTarget::SubTab(i),
                );
            }
            spans.push(Span::styled(chip, style));
            x = x.saturating_add(w);
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
        );
        content_top = inner.y + 1;
    }
    let content_area = Rect {
        x: inner.x,
        y: content_top,
        width: inner.width,
        height: inner.bottom().saturating_sub(content_top),
    };
    if content_area.height == 0 {
        return;
    }

    // Resolve full definition + run files for the current selection.
    let def = if let DetailContext::Definition { repo, name } = &ctx {
        app.full_defs.get(&format!("{repo}/{name}")).cloned()
    } else {
        None
    };
    let run_files = match &ctx {
        DetailContext::Run { task } => {
            app.run_files.as_ref().filter(|(id, _)| id == &task.id).map(|(_, f)| f)
        }
        _ => None,
    };

    let (lines, placeholder) = content_for(&ctx, sub_tab, def.as_ref(), run_files);
    if lines.is_empty() {
        frame.render_widget(Paragraph::new(placeholder).style(p.dim_style()), content_area);
        return;
    }
    let bottom = bottom_anchored(kind, sub_tab);
    let height = content_area.height as usize;
    let (start, end) = window_lines(lines.len(), height, app_scroll_offset(app, c), bottom);
    let styled: Vec<Line> = lines[start..end]
        .iter()
        .map(|l| if l.is_empty() { Line::from(" ") } else { style_line(l, p) })
        .collect();
    frame.render_widget(Paragraph::new(Text::from(styled)), content_area);

    // Scrollbar over the content region.
    if lines.len() > height {
        let mut state = ScrollbarState::new(lines.len() - height).position(start);
        hits.push(
            Rect {
                x: content_area.right().saturating_sub(1),
                y: content_area.y,
                width: 1,
                height: content_area.height,
            },
            HitTarget::ScrollbarTrack(PaneId::Detail),
        );
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            content_area,
            &mut state,
        );
    }
}

fn app_scroll_offset(app: &App, c: &Computed) -> usize {
    let _ = app;
    c.ui.scroll_offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{DetailKind, ListPane, PaneId, TabUiState};
    use crate::hit::HitTarget;
    use crate::runfiles::RunFiles;
    use crate::test_fixtures::fixture_app;
    use crate::view::render as render_frame;
    use ratatui::{Terminal, backend::TestBackend};

    /// fixture_app focused on the detail pane over the queue selection, with a
    /// 40-line transcript loaded for the running task.
    fn detail_app(sub_tab_run: usize) -> App {
        let mut app = fixture_app();
        app.run_files = Some((
            "01RUN".to_string(),
            RunFiles {
                transcript_tail: (0..40).map(|i| format!("line {i}")).collect(),
                report: vec![],
            },
        ));
        let mut ui = TabUiState::default();
        ui.focus = PaneId::Detail;
        ui.last_list_pane = ListPane::Queue;
        ui.sub_tab[DetailKind::Run as usize] = sub_tab_run;
        app.ui_by_tab.insert("acme".to_string(), ui);
        app
    }

    fn render_at(app: &App, w: u16, h: u16) -> (Terminal<TestBackend>, HitMap) {
        let mut app = app.clone();
        app.size = (w, h);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut hits = HitMap::new();
        terminal.draw(|frame| hits = render_frame(&app, frame)).unwrap();
        (terminal, hits)
    }

    #[test]
    fn snapshot_detail_transcript() {
        let (terminal, hits) = render_at(&detail_app(0), 80, 24);
        insta::assert_snapshot!("detail_transcript", terminal.backend());
        assert!(
            hits.iter().any(|(_, t)| *t == HitTarget::SubTab(0)),
            "transcript sub-tab chip is clickable"
        );
    }

    #[test]
    fn out_of_range_sub_tab_clamps_into_range() {
        // sub_tab 9 on a Run context clamps to the last valid index (2 = prompt),
        // not the fall-through transcript an *unclamped* index would hit via the
        // `_` arm. Proves clamping is active and nothing indexes out of bounds.
        let (terminal, _hits) = render_at(&detail_app(9), 80, 24);
        let body = terminal.backend().to_string();
        assert!(body.contains("implement the widget cache"), "clamped to the prompt sub-tab");
        assert!(!body.contains("line 39"), "clamped index is not the transcript tail");
    }
}
