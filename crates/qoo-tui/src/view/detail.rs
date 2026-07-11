use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use unicode_width::UnicodeWidthChar;

use crate::app::{App, DetailGeom, DetailSelection, PaneId};
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

/// Blank-cell placeholder shown for an absent value (dimmed by the styler).
const EM_DASH: &str = "—";
/// Minimum gap between the aligned key column and the value column.
const CONFIG_KEY_GAP: usize = 2;

/// Human-readable duration from milliseconds: `Xs` below a minute, `Xm` on the
/// minute range (whole minutes, seconds truncated), `Xh` / `Xh Ym` for hours.
/// Pure — the ideal unit-test target.
fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    let (hours, rem_min) = (mins / 60, mins % 60);
    if rem_min == 0 { format!("{hours}h") } else { format!("{hours}h {rem_min}m") }
}

/// `(key, value)` rows for the config sub-tab. The model row folds the resolved
/// id in as `alias → id` when the daemon sent a `model_resolved` that differs
/// from the authored alias; otherwise it shows the single authored value.
fn config_rows(def: &TaskDefinition) -> Vec<(&'static str, String)> {
    let model = match &def.model_resolved {
        Some(resolved) if resolved != &def.model => format!("{} → {}", def.model, resolved),
        _ => def.model.clone(),
    };
    vec![
        ("args", if def.args.is_empty() { EM_DASH.to_string() } else { arg_summary(&def.args) }),
        ("worktree", def.worktree.clone()),
        ("dedup", def.dedup.clone()),
        ("model", model),
        ("timeout", format_duration(def.timeout_ms)),
        ("priority", def.priority.clone()),
        (
            "discovery",
            def.discovery.as_ref().map(|d| d.command.clone()).unwrap_or_else(|| EM_DASH.to_string()),
        ),
    ]
}

/// Aligned config lines plus the char column where the value begins (keys are
/// left-padded to a common width + [`CONFIG_KEY_GAP`]). Returned together so the
/// renderer can tag every line with a matching [`LineCtx::Config`] for per-span
/// key/value styling.
fn config_view(def: &TaskDefinition) -> (Vec<String>, usize) {
    let rows = config_rows(def);
    let key_col =
        rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0) + CONFIG_KEY_GAP;
    let lines = rows.iter().map(|(k, v)| format!("{k:<key_col$}{v}")).collect();
    (lines, key_col)
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
            Some(d) if sub_tab == 1 => (config_view(d).0, ""),
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

/// Render a detail text selection to a string: each wrapped display line in the
/// selected range sliced by the selection's cell columns, joined with `\n`. The
/// first line starts at the anchor cell, the last ends at the cursor cell
/// (inclusive); interior lines take the whole line. Absolute line indices are
/// clamped to `lines`, so a transcript that shrank under a persisted selection
/// slices safely instead of panicking. Pure — the ideal unit-test target for the
/// range→text mapping.
pub(crate) fn extract_selection(lines: &[String], sel: &DetailSelection) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let (a, b) = sel.ordered();
    let last = lines.len() - 1;
    let a_line = a.line.min(last);
    let b_line = b.line.min(last);
    let mut out: Vec<String> = Vec::with_capacity(b_line - a_line + 1);
    for (off, text) in lines[a_line..=b_line].iter().enumerate() {
        let l = a_line + off;
        // `lo`/`hi` fall back to whole-line bounds off the first/last selected
        // line; when the selection collapsed onto one clamped line both apply,
        // and `slice_cells` is order-safe if that leaves `lo > hi`.
        let lo = if l == a_line { a.cell } else { 0 };
        let hi = if l == b_line { b.cell } else { usize::MAX };
        out.push(crate::markup::slice_cells(text, lo, hi));
    }
    out.join("\n")
}

/// Overlay `sel_style` onto the cells of `line` in the inclusive cell range
/// `[lo, hi]` (`hi == usize::MAX` selects to end-of-line), splitting spans at the
/// range boundaries so per-span syntax colors OUTSIDE the range survive. A char
/// is highlighted when its cell span overlaps the range (a click on either half
/// of a double-width char highlights the whole char). Pure over the input line.
fn patch_line_cols(line: &Line<'static>, lo: usize, hi: usize, sel_style: Style) -> Line<'static> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;
    for span in &line.spans {
        let base = span.style;
        let mut buf = String::new();
        let mut buf_sel = false;
        for ch in span.content.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            // `w.max(1)` so a zero-width char groups with the selection region it
            // sits in rather than orphaning into its own span.
            let selected = col + w.max(1) > lo && col <= hi;
            if !buf.is_empty() && selected != buf_sel {
                let style = if buf_sel { base.patch(sel_style) } else { base };
                out.push(Span::styled(std::mem::take(&mut buf), style));
            }
            buf.push(ch);
            buf_sel = selected;
            col += w;
        }
        if !buf.is_empty() {
            let style = if buf_sel { base.patch(sel_style) } else { base };
            out.push(Span::styled(buf, style));
        }
    }
    Line::from(out)
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
    // Reset the render-feedback selection geometry each frame; the draw path
    // below republishes it when content is drawn. Any early return (no room,
    // empty content) then leaves it empty so a stray press resolves to no line.
    app.detail_geom.replace(DetailGeom::default());
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
    // The definition config sub-tab styles each `key   value` row via a dedicated
    // Config ctx (key column vs value); every other view flows through the
    // markdown fence machinery. Fence state is stateful over the WHOLE transcript,
    // so precompute it once — a window into the middle of a code block must still
    // style as code, and each wrapped segment carries its line's ctx.
    let is_config = matches!(ctx, DetailContext::Definition { .. }) && sub_tab == 1 && def.is_some();
    let ctxs = if is_config {
        let key_col = config_view(def.as_ref().expect("is_config implies Some")).1;
        vec![LineCtx::Config { key_col }; lines.len()]
    } else {
        fence_states(&lines)
    };
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
    let mut styled: Vec<Line> = display[start..end]
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
    // Overlay the mouse text selection (anchored to ABSOLUTE display-line indices,
    // so it stays put as the window scrolls) with the palette selection style.
    if let Some(sel) = &app.detail_selection {
        let (a, b) = sel.ordered();
        let sel_style = p.selection();
        for (i, line) in styled.iter_mut().enumerate() {
            let abs = start + i;
            if abs < a.line || abs > b.line {
                continue;
            }
            let lo = if abs == a.line { a.cell } else { 0 };
            let hi = if abs == b.line { b.cell } else { usize::MAX };
            *line = patch_line_cols(line, lo, hi, sel_style);
        }
    }
    // Publish selection geometry so the next mouse event resolves against exactly
    // these wrapped lines (full set, not just the window, so absolute indices and
    // scroll-persistence work). Same freshness guarantee as `hit`.
    app.detail_geom.replace(DetailGeom {
        area: content_area,
        window_start: start,
        lines: display.iter().map(|d| d.text.clone()).collect(),
    });
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

    /// Detail pane focused on the definition config sub-tab: a def summary makes
    /// the Tasks pane selectable (→ Definition context) and a full def in
    /// `full_defs` supplies the config rows. `opus`/`claude-opus-4-8` exercises
    /// the resolved-model arrow, and the `discovery: —` row the dim placeholder.
    fn detail_def_config_app() -> App {
        use crate::ipc::types::{ArgSpec, DefinitionSummary};
        let mut app = fixture_app();
        app.defs_by_project.insert(
            "acme".to_string(),
            vec![DefinitionSummary {
                repo: "acme".to_string(),
                name: "pr-ready".to_string(),
                scope: "project".to_string(),
                ..Default::default()
            }],
        );
        app.full_defs.insert(
            "acme/pr-ready".to_string(),
            TaskDefinition {
                name: "pr-ready".to_string(),
                repo: "acme".to_string(),
                discovery: None,
                cron: None,
                args: vec![ArgSpec { name: "situation".to_string(), ..Default::default() }],
                dedup: "none".to_string(),
                worktree: "auto".to_string(),
                pre_run: None,
                post_run: None,
                model: "opus".to_string(),
                model_resolved: Some("claude-opus-4-8".to_string()),
                timeout_ms: 1_800_000,
                priority: "normal".to_string(),
                prompt: "do the thing".to_string(),
            },
        );
        let mut ui = TabUiState::default();
        ui.focus = PaneId::Detail;
        ui.last_list_pane = ListPane::Tasks;
        ui.sub_tab[DetailKind::Definition as usize] = 1; // config sub-tab
        app.ui_by_tab.insert("acme".to_string(), ui);
        app
    }

    #[test]
    fn snapshot_detail_definition_config() {
        // The config tab renders aligned key/value rows: keys in accent, the
        // resolved-model arrow dimmed with the id emphasized, and the empty
        // `discovery` value as a dim `—`.
        let (terminal, hits) = render_at(&detail_def_config_app(), 60, 16);
        insta::assert_snapshot!("detail_definition_config", terminal.backend());
        assert!(
            hits.iter().any(|(_, t)| *t == HitTarget::SubTab(1)),
            "config sub-tab chip is clickable"
        );
    }

    #[test]
    fn format_duration_human_units() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(30_000), "30s");
        assert_eq!(format_duration(59_000), "59s");
        // Whole minutes truncate seconds.
        assert_eq!(format_duration(90_000), "1m");
        assert_eq!(format_duration(1_800_000), "30m");
        assert_eq!(format_duration(2_700_000), "45m");
        // Hours, whole and mixed.
        assert_eq!(format_duration(3_600_000), "1h");
        assert_eq!(format_duration(5_400_000), "1h 30m");
        assert_eq!(format_duration(7_200_000), "2h");
    }

    #[test]
    fn config_view_aligns_keys_and_folds_resolved_model() {
        let mut def = TaskDefinition {
            model: "opus".to_string(),
            model_resolved: Some("claude-opus-4-8".to_string()),
            timeout_ms: 1_800_000,
            worktree: "auto".to_string(),
            dedup: "none".to_string(),
            priority: "normal".to_string(),
            ..Default::default()
        };
        let (lines, key_col) = config_view(&def);
        // Longest key is "discovery" (9) + 2-gap → value column at char 11.
        assert_eq!(key_col, 11);
        // Every line's key column is padded to the same width.
        for line in &lines {
            assert!(line.chars().count() >= key_col, "{line:?} shorter than key column");
        }
        assert!(lines.iter().any(|l| l == "model      opus → claude-opus-4-8"));
        assert!(lines.iter().any(|l| l == "timeout    30m"));
        assert!(lines.iter().any(|l| l == "discovery  —"));
        // When resolved == authored, no arrow is shown.
        def.model_resolved = Some("opus".to_string());
        let (lines, _) = config_view(&def);
        assert!(lines.iter().any(|l| l == "model      opus"));
        // Absent resolved (old daemon) also shows the authored alias alone.
        def.model_resolved = None;
        let (lines, _) = config_view(&def);
        assert!(lines.iter().any(|l| l == "model      opus"));
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

    // ---- text selection ----------------------------------------------------

    use crate::app::{DetailPoint, DetailSelection};

    fn sel(a: (usize, usize), b: (usize, usize)) -> DetailSelection {
        DetailSelection {
            anchor: DetailPoint { line: a.0, cell: a.1 },
            cursor: DetailPoint { line: b.0, cell: b.1 },
        }
    }

    #[test]
    fn extract_selection_single_line_inclusive() {
        let lines = vec!["hello world".to_string()];
        assert_eq!(extract_selection(&lines, &sel((0, 0), (0, 4))), "hello");
        // Reversed anchor/cursor orders the same.
        assert_eq!(extract_selection(&lines, &sel((0, 4), (0, 0))), "hello");
    }

    #[test]
    fn extract_selection_spans_multiple_lines_with_newlines() {
        let lines = vec![
            "first line".to_string(),
            "middle".to_string(),
            "last one".to_string(),
        ];
        // From cell 6 on line 0 → cell 3 on line 2: "line" + whole middle + "last".
        let got = extract_selection(&lines, &sel((0, 6), (2, 3)));
        assert_eq!(got, "line\nmiddle\nlast");
    }

    #[test]
    fn extract_selection_multiwidth_and_empty_line() {
        // A CJK line (each char 2 cells) plus an empty line in the range.
        let lines = vec!["中文字".to_string(), String::new(), "tail".to_string()];
        // line0 cell2..end (字文... actually cells: 中[0,1] 文[2,3] 字[4,5]) → from
        // cell 2 = "文字"; empty middle → ""; line2 to cell1 = "ta".
        let got = extract_selection(&lines, &sel((0, 2), (2, 1)));
        assert_eq!(got, "文字\n\nta");
    }

    #[test]
    fn extract_selection_clamps_shrunk_content() {
        // A selection referencing lines past a shrunk transcript slices safely.
        let lines = vec!["only".to_string()];
        assert_eq!(extract_selection(&lines, &sel((0, 0), (9, 99))), "only");
        assert_eq!(extract_selection(&[], &sel((0, 0), (0, 3))), "");
    }

    #[test]
    fn patch_line_cols_highlights_only_the_selected_columns() {
        let p = Palette::default();
        let selection = p.selection();
        // A single plain span "hello world"; highlight cells [0,4] = "hello".
        let line = Line::from(vec![Span::raw("hello world")]);
        let out = patch_line_cols(&line, 0, 4, selection);
        let parts: Vec<(String, Style)> =
            out.spans.iter().map(|s| (s.content.to_string(), s.style)).collect();
        assert_eq!(parts[0].0, "hello");
        assert_eq!(parts[0].1, Style::default().patch(selection));
        // The remainder keeps its (plain) style.
        let rest: String = parts[1..].iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(rest, " world");
        assert!(parts[1..].iter().all(|(_, st)| *st == Style::default()));
    }

    #[test]
    fn patch_line_cols_to_end_of_line_with_max_sentinel() {
        let p = Palette::default();
        let selection = p.selection();
        let line = Line::from(vec![Span::raw("abcde")]);
        let out = patch_line_cols(&line, 2, usize::MAX, selection);
        // Cells 0..1 plain, 2..end selected.
        let sel_text: String = out
            .spans
            .iter()
            .filter(|s| s.style == Style::default().patch(selection))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(sel_text, "cde");
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
