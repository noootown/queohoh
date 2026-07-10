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
use crate::markup::{DisplayLine, LineCtx, fence_states, style_transcript_line, wrap_lines};
use crate::selectors::{WtState, arg_summary, prompt_summary};
use crate::view::Computed;
use crate::view::theme::{
    GLYPH_DONE, GLYPH_FAILED, GLYPH_NEEDS_INPUT, GLYPH_QUEUED, GLYPH_RUNNING, Palette,
    TITLE_DETAIL,
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
/// extent. Reads the render-feedback [`crate::app::App::detail_wrapped_len`]
/// (the post-wrap display-line count from the last frame) rather than recomputing
/// the wrap: a scrollbar can only be dragged after it renders, so the cell is
/// always fresh — same freshness argument as `hit` / `detail_max_scroll`.
pub(crate) fn detail_content_len(app: &crate::app::App) -> usize {
    app.detail_wrapped_len.get()
}

/// Wrap `lines` for a `width`×`height` viewport, resolving the scrollbar
/// chicken-and-egg: whether the scrollbar shows depends on the wrapped count,
/// which depends on the width its column steals. Two deterministic passes — wrap
/// at full width; if that overflows the viewport the scrollbar shows, so re-wrap
/// one column narrower (narrower can only add segments, so the overflow verdict
/// never flips back). Returns the display lines, whether a scrollbar is needed,
/// and the text width fence rules must be sized to (one narrower with a scrollbar).
fn wrap_for_viewport(
    lines: &[String],
    ctxs: &[LineCtx],
    width: usize,
    height: usize,
) -> (Vec<DisplayLine>, bool, u16) {
    let display = wrap_lines(lines, ctxs, width);
    if display.len() > height && width > 1 {
        (wrap_lines(lines, ctxs, width - 1), true, (width - 1) as u16)
    } else {
        let has_scrollbar = display.len() > height;
        (display, has_scrollbar, width as u16)
    }
}

pub fn render(app: &App, c: &Computed, frame: &mut ratatui::Frame, area: Rect, hits: &mut HitMap) {
    let p: &Palette = &c.palette;
    let focused = matches!(c.ui.focus, PaneId::Detail);
    // Spotlight: while a list pane is being search-typed, detail mutes too.
    let dimmed = c.searching.iter().any(|&s| s);
    let title_style = if dimmed {
        p.dim_style().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(if focused { p.accent } else { p.fg })
            .add_modifier(Modifier::BOLD)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(p.border_style(focused))
        .title(Span::styled(TITLE_DETAIL, title_style));
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
            let style = if dimmed {
                p.dim_style()
            } else if i == sub_tab {
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
        app.detail_max_scroll.set(0);
        app.detail_wrapped_len.set(0);
        frame.render_widget(Paragraph::new(placeholder).style(p.dim_style()), content_area);
        return;
    }
    let bottom = bottom_anchored(kind, sub_tab);
    let height = content_area.height as usize;
    // Fence state is stateful over the WHOLE transcript, so precompute it once —
    // a window into the middle of a code block must still style as code, and each
    // wrapped segment carries its line's ctx.
    let ctxs = fence_states(&lines);
    // Wrap logical lines into display lines FIRST, so every consumer (scroll
    // ceiling, windowing, scrollbar) counts on-screen lines, not logical ones.
    let (display, has_scrollbar, text_width) =
        wrap_for_viewport(&lines, &ctxs, content_area.width as usize, height);
    let total = display.len();
    // Render feedback: the true scroll ceiling (see `App::detail_max_scroll`) and
    // the wrapped length (for scrollbar-drag math), both over the WRAPPED content.
    app.detail_max_scroll.set(total.saturating_sub(height));
    app.detail_wrapped_len.set(total);
    let (start, end) = window_lines(total, height, app_scroll_offset(app, c), bottom);
    let styled: Vec<Line> = display[start..end]
        .iter()
        .map(|seg| {
            // Only original fence-delimiter lines carry `Fence` ctx (continuations
            // never do), so `style_transcript_line` regenerates a rule only for a
            // real rule line — `text_width` sizes it clear of the scrollbar column.
            let mut line = if seg.text.is_empty() {
                Line::from(" ")
            } else {
                style_transcript_line(&seg.text, &seg.ctx, text_width, p)
            };
            if dimmed {
                // Spotlight mute: flatten the markup colors while filtering.
                for span in line.spans.iter_mut() {
                    span.style = span.style.patch(p.dim_style());
                }
            }
            line
        })
        .collect();
    frame.render_widget(Paragraph::new(Text::from(styled)), content_area);

    // Scrollbar over the content region.
    if has_scrollbar {
        let mut state = ScrollbarState::new(total - height).position(start);
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
        detail_app_transcript((0..40).map(|i| format!("line {i}")).collect(), sub_tab_run)
    }

    /// Detail pane over the queue selection with a caller-supplied transcript —
    /// the single fixture-builder both the plain and fenced snapshot tests use.
    fn detail_app_transcript(transcript: Vec<String>, sub_tab_run: usize) -> App {
        let mut app = fixture_app();
        app.run_files = Some((
            "01RUN".to_string(),
            RunFiles { transcript_tail: transcript, report: vec![] },
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
    fn snapshot_detail_transcript_fenced() {
        // A ```bash and ```json block: opening fences render as labeled rules,
        // closing fences as plain rules, bodies get syntax accents — the literal
        // backticks never appear.
        let app = detail_app_transcript(
            [
                "Build steps:",
                "```bash",
                "cd ~/proj && make build",
                "cat log.txt | grep error",
                "```",
                "Config:",
                "```json",
                "{\"name\": \"qoo\", \"count\": 3, \"ok\": true}",
                "```",
                "done",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            0,
        );
        let (terminal, _hits) = render_at(&app, 80, 24);
        insta::assert_snapshot!("detail_transcript_fenced", terminal.backend());
    }

    #[test]
    fn snapshot_detail_transcript_wrapped_url() {
        // A long GitHub URL on the final (bottom-anchored) transcript line wraps
        // onto the next row instead of clipping at the pane edge. The preceding
        // short lines push the view past the viewport so this also exercises the
        // scrollbar-column two-pass and the bottom-anchored tail landing on the
        // last WRAPPED segment.
        let mut lines: Vec<String> = (0..24).map(|i| format!("line {i}")).collect();
        lines.push(
            "See https://github.com/justicebid/monorepo/pull/1234/files#diff-0a1b2c3d4e5f done"
                .to_string(),
        );
        let (terminal, _hits) = render_at(&detail_app_transcript(lines, 0), 80, 24);
        insta::assert_snapshot!("detail_transcript_wrapped_url", terminal.backend());
    }

    #[test]
    fn wrap_for_viewport_reserves_scrollbar_column_on_overflow() {
        // Four 10-cell lines into a width-10, height-3 viewport fit at full width
        // (4 display lines) but 4 > 3 forces a scrollbar, so the second pass
        // re-wraps at width 9 — each 10-cell line splits in two → 8 display lines.
        let lines: Vec<String> = vec!["abcdefghij".into(); 4];
        let ctxs = fence_states(&lines);
        let (display, has_scrollbar, text_width) = wrap_for_viewport(&lines, &ctxs, 10, 3);
        assert!(has_scrollbar);
        assert_eq!(text_width, 9);
        assert_eq!(display.len(), 8);
    }

    #[test]
    fn wrap_for_viewport_keeps_full_width_when_it_fits() {
        let lines: Vec<String> = vec!["abcdefghij".into(); 2];
        let ctxs = fence_states(&lines);
        let (display, has_scrollbar, text_width) = wrap_for_viewport(&lines, &ctxs, 10, 10);
        assert!(!has_scrollbar);
        assert_eq!(text_width, 10);
        assert_eq!(display.len(), 2);
    }

    #[test]
    fn wrapping_counts_display_lines_for_scroll_ceiling() {
        // One 2000-char logical line wraps into many display lines. The render-fed
        // ceiling + wrapped length count DISPLAY lines — a single unwrapped logical
        // line would have left `detail_max_scroll` at 0 (nothing to scroll).
        // Render the same instance (not `render_at`, which clones) so the
        // interior-mutability feedback cells are observable afterwards.
        let mut app = detail_app_transcript(vec!["x".repeat(2000)], 0);
        app.size = (80, 24);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| {
            render_frame(&app, frame);
        }).unwrap();
        let wrapped = app.detail_wrapped_len.get();
        assert!(wrapped > 1, "the long line wrapped into many display lines");
        assert!(
            app.detail_max_scroll.get() > 0,
            "wrapping opened scroll room a single logical line would not have"
        );
        assert!(app.detail_max_scroll.get() < wrapped, "ceiling stays below the wrapped total");
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
