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
use crate::ipc::types::{TaskDefinition, TaskInstance, TaskStatus};
use crate::runfiles::RunMeta;
use crate::markup::{DisplayLine, LineCtx, fence_states, style_transcript_line, wrap_lines};
use crate::selectors::arg_summary;
use crate::view::Computed;
use crate::view::theme::{Palette, TITLE_DETAIL};

/// Blank-cell placeholder shown for an absent value (dimmed by the styler).
const EM_DASH: &str = "—";
/// Minimum gap between the aligned key column and the value column.
const CONFIG_KEY_GAP: usize = 2;
/// Two-space indent under each `info` sub-tab section header.
const INFO_INDENT: &str = "  ";

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

/// Aligned `key   value` lines plus the char column where the value begins (keys
/// are left-padded to a common width + [`CONFIG_KEY_GAP`]). Returned together so
/// the renderer can tag every line with a matching [`LineCtx::Config`] for
/// per-span key/value styling. Shared by the definition config sub-tab and the
/// worktree detail info block.
fn align_kv(rows: &[(&str, String)]) -> (Vec<String>, usize) {
    let key_col =
        rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0) + CONFIG_KEY_GAP;
    let lines = rows.iter().map(|(k, v)| format!("{k:<key_col$}{v}")).collect();
    (lines, key_col)
}

/// Aligned config lines + the value column (see [`align_kv`]).
fn config_view(def: &TaskDefinition) -> (Vec<String>, usize) {
    align_kv(&config_rows(def))
}

/// `(key, value)` rows for the worktree detail info block: identity (path,
/// branch) plus the daemon's git enrichment (short commit hash, author name,
/// last-commit age with absolute local time, open PR number). Absent values show
/// the dim `—` placeholder. `state` is deliberately dropped — the WORKTREES pane
/// already conveys it via its status glyph.
fn worktree_rows(
    row: &crate::selectors::WorktreeRow,
    now_epoch_s: u64,
    tz_offset_s: i32,
) -> Vec<(&'static str, String)> {
    let or_dash = |s: Option<String>| s.filter(|v| !v.is_empty()).unwrap_or_else(|| EM_DASH.to_string());
    let updated = match row.last_commit_epoch {
        Some(e) => format!(
            "{} ({})",
            crate::selectors::relative_age_label(e, now_epoch_s),
            crate::selectors::absolute_local_label(e, tz_offset_s),
        ),
        None => EM_DASH.to_string(),
    };
    vec![
        ("path", row.path.clone()),
        ("branch", if row.branch.is_empty() { EM_DASH.to_string() } else { row.branch.clone() }),
        ("commit", or_dash(row.last_commit_hash.clone())),
        ("author", or_dash(row.last_commit_author.clone())),
        ("updated", updated),
        ("pr", row.pr_number.map(|n| format!("#{n}")).unwrap_or_else(|| EM_DASH.to_string())),
    ]
}

/// Wire status → the lowercase label shown in the `info` tab's Run section
/// (mirrors the daemon's kebab-case wire values).
fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "queued",
        TaskStatus::NeedsInput => "needs-input",
        TaskStatus::Running => "running",
        TaskStatus::Done => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Skipped => "skipped",
        TaskStatus::VerifyFailed => "verify-failed",
        TaskStatus::Unknown => "unknown",
    }
}

/// Section header + indented `key   value` rows for the run `info` sub-tab, in
/// the agent247-dashboard shape. Identity/status come from the LIVE `task`
/// (freshest); timing, usage, and the def config come from the run's `data.json`
/// snapshot (`meta`). Absent values render the dim `—`. `now_epoch_s`/`tz_offset_s`
/// drive each timing stamp's absolute local time + relative age. Lines are
/// returned parallel to their [`LineCtx`] — [`LineCtx::Header`] for section
/// titles, [`LineCtx::Config`] for the rows (one value column across all rows).
fn run_info_lines(
    task: &TaskInstance,
    meta: &RunMeta,
    now_epoch_s: u64,
    tz_offset_s: i32,
) -> (Vec<String>, Vec<LineCtx>) {
    let dash = || EM_DASH.to_string();
    let or_dash = |v: Option<String>| v.filter(|s| !s.is_empty()).unwrap_or_else(dash);
    // "MM/DD HH:MM (Nd ago)" from an ISO stamp; dim `—` when absent.
    let stamp = |iso: Option<&str>| match iso.filter(|s| !s.is_empty()) {
        Some(s) => {
            let e = crate::selectors::parse_iso_epoch_s(s);
            format!(
                "{} ({})",
                crate::selectors::absolute_local_label(e, tz_offset_s),
                crate::selectors::relative_age_label(e, now_epoch_s),
            )
        }
        None => dash(),
    };

    // Section title → its rows, in render order. The renderer separates sections
    // with a blank line and a `Header`-styled title line.
    let mut sections: Vec<(&'static str, Vec<(&'static str, String)>)> = Vec::new();

    // Run — identity + status from the live task; error/reason only on failure.
    let mut run = vec![
        ("id", task.id.clone()),
        ("definition", task.definition.clone().unwrap_or_else(|| "adhoc".to_string())),
        ("status", status_label(task.status).to_string()),
    ];
    if let Some(err) = task.error.as_deref().filter(|e| !e.is_empty()) {
        run.push(("error", err.to_string()));
    } else if let Some(reason) = meta.reason.as_deref().filter(|r| !r.is_empty()) {
        run.push(("reason", reason.to_string()));
    }
    sections.push(("Run", run));

    // Timing — created (live) + started/finished (run record); duration prefers
    // the recorded usage, else spans finished − started.
    let duration = meta
        .duration_ms
        .map(format_duration)
        .or_else(|| match (meta.started_at.as_deref(), meta.finished_at.as_deref()) {
            (Some(a), Some(b)) => {
                let (s, f) =
                    (crate::selectors::parse_iso_epoch_s(a), crate::selectors::parse_iso_epoch_s(b));
                Some(format_duration(f.saturating_sub(s) * 1000))
            }
            _ => None,
        })
        .unwrap_or_else(dash);
    sections.push((
        "Timing",
        vec![
            ("created", stamp(Some(task.created.as_str()))),
            ("started", stamp(meta.started_at.as_deref())),
            ("finished", stamp(meta.finished_at.as_deref())),
            ("duration", duration),
        ],
    ));

    // Details — worktree/session/model prefer the run record, fall back to live.
    let mut details = vec![
        (
            "worktree",
            or_dash(meta.resolved_worktree.clone().or_else(|| task.target.worktree.clone())),
        ),
        ("session", or_dash(meta.session_id.clone())),
        ("model", or_dash(meta.model.clone().or_else(|| task.model.clone()))),
        ("exit code", meta.exit_code.map(|c| c.to_string()).unwrap_or_else(dash)),
    ];
    if meta.timed_out {
        details.push(("timed out", "yes".to_string()));
    }
    details.push(("cost", meta.cost_usd.map(|c| format!("${c}")).unwrap_or_else(dash)));
    details.push(("turns", meta.turns.map(|t| t.to_string()).unwrap_or_else(dash)));
    sections.push(("Details", details));

    // Config — only when the run recorded a def snapshot (absent for adhoc runs).
    if let Some(def) = &meta.definition {
        sections.push((
            "Config",
            vec![
                ("description", or_dash(def.description.clone())),
                ("worktree", def.worktree.clone()),
                ("dedup", def.dedup.clone()),
                ("timeout", format_duration(def.timeout_ms)),
                ("priority", def.priority.clone()),
                ("cron", or_dash(def.cron.clone())),
            ],
        ));
    }

    // One value column across ALL rows (indent + key), then emit.
    let key_col = sections
        .iter()
        .flat_map(|(_, rows)| rows.iter())
        .map(|(k, _)| INFO_INDENT.len() + k.chars().count())
        .max()
        .unwrap_or(0)
        + CONFIG_KEY_GAP;
    let mut lines = Vec::new();
    let mut ctxs = Vec::new();
    for (i, (title, rows)) in sections.iter().enumerate() {
        if i > 0 {
            lines.push(String::new());
            ctxs.push(LineCtx::Text);
        }
        lines.push(title.to_string());
        ctxs.push(LineCtx::Header);
        for (k, v) in rows {
            let key = format!("{INFO_INDENT}{k}");
            lines.push(format!("{key:<key_col$}{v}"));
            ctxs.push(LineCtx::Config { key_col });
        }
    }
    (lines, ctxs)
}

/// `(key, value)` rows for the report tab's `## Stats` block, built from the
/// SAME `RunMeta` the `info` sub-tab reads (not parsed from `report.md`'s
/// text) — mirrors `run-store.ts`'s `finishRun` bullets (outcome + reason,
/// model, cost, turns, duration) field-for-field, but reuses this file's own
/// `EM_DASH` fallback and `format_duration` so it reads identically to the
/// `info` tab's Details section once aligned via [`align_kv`].
fn stats_rows(meta: &RunMeta) -> Vec<(&'static str, String)> {
    let dash = || EM_DASH.to_string();
    let outcome = match &meta.outcome {
        Some(o) => match meta.reason.as_deref().filter(|r| !r.is_empty()) {
            Some(r) => format!("{o} ({r})"),
            None => o.clone(),
        },
        None => dash(),
    };
    vec![
        ("outcome", outcome),
        ("model", meta.model.clone().unwrap_or_else(dash)),
        ("cost", meta.cost_usd.map(|c| format!("${c}")).unwrap_or_else(dash)),
        ("turns", meta.turns.map(|t| t.to_string()).unwrap_or_else(dash)),
        ("duration", meta.duration_ms.map(format_duration).unwrap_or_else(dash)),
    ]
}

/// Report tab content: `report.md`'s markdown as-is, except the `## Stats`
/// block is replaced with the aligned key/value rows from [`stats_rows`] —
/// the SAME `LineCtx::Config` styling the `info`/`config` sub-tabs use, so
/// `model` picks up the metadata-gold color `config_value_style` already
/// gives it there (matching how the TASKS pane colors a model), instead of
/// the plain grey a literal `- key: value` markdown bullet gets. Falls back
/// to a plain markdown render of the raw text when there is no run record
/// yet (an old daemon, or the run hasn't reached `data.json` yet) or the
/// `## Stats` heading isn't present (an old report.md, or mid-run before
/// `finishRun` has written it).
fn report_content(report: Vec<String>, meta: Option<&RunMeta>) -> (Vec<String>, Vec<LineCtx>) {
    let ctxs = fence_states(&report);
    let Some(meta) = meta else { return (report, ctxs) };
    let Some(heading) = report.iter().position(|l| l == "## Stats") else {
        return (report, ctxs);
    };
    // The contiguous `- ` bullets `finishRun` writes immediately below the
    // heading — replaced wholesale; everything else (the `# Result` text
    // above, any `## Verify` section below) is untouched.
    let start = heading + 1;
    let end = report[start..]
        .iter()
        .position(|l| !l.starts_with("- "))
        .map(|n| start + n)
        .unwrap_or(report.len());
    let (rows, key_col) = align_kv(&stats_rows(meta));
    let row_ctxs = vec![LineCtx::Config { key_col }; rows.len()];
    let mut lines = report;
    let mut ctxs = ctxs;
    lines.splice(start..end, rows);
    ctxs.splice(start..end, row_ctxs);
    (lines, ctxs)
}

/// A clickable descriptor for the worktree info block's `pr` row, or `None`
/// unless the row carries BOTH an open PR number and its (non-empty) url — an
/// old daemon sends neither, and the em-dash placeholder is never a link.
/// `line_text` is [`align_kv`]'s exact output for the pr row so the renderer can
/// locate its (in-practice-unwrapped) display segment by an exact match — a
/// wrap simply declines the link. `value_col` is the shared aligned value
/// column; `value_len` the `#<n>` char width (the clickable span).
struct PrLinkGeom {
    line_text: String,
    value_col: usize,
    value_len: usize,
    url: String,
}

fn worktree_pr_link(
    row: &crate::selectors::WorktreeRow,
    now_epoch_s: u64,
    tz_offset_s: i32,
) -> Option<PrLinkGeom> {
    let number = row.pr_number?;
    let url = row.pr_url.clone().filter(|u| !u.is_empty())?;
    let rows = worktree_rows(row, now_epoch_s, tz_offset_s);
    let idx = rows.iter().position(|(k, _)| *k == "pr")?;
    let (lines, key_col) = align_kv(&rows);
    Some(PrLinkGeom {
        line_text: lines.get(idx)?.clone(),
        value_col: key_col,
        value_len: format!("#{number}").chars().count(),
        url,
    })
}

/// Content lines, their per-line [`LineCtx`], and a placeholder for the given
/// context/sub-tab. `def` is the resolved full definition (None while loading),
/// `run_files` the current run's (report, transcript_tail). `detail_row` is the
/// worktree lane-task row cursor; `now_epoch_s`/`tz_offset_s` drive the info
/// block's `updated` age + absolute local time. The ctx vector is parallel to
/// the lines so the renderer styles each line under exactly the right rules —
/// markdown fences for run/prompt views, aligned key/value for config + the
/// worktree info block, and queue-style rows for the lane-task list.
pub(crate) fn content_for(
    ctx: &DetailContext,
    sub_tab: usize,
    def: Option<&TaskDefinition>,
    run_files: Option<&crate::runfiles::RunFiles>,
    detail_row: usize,
    now_epoch_s: u64,
    tz_offset_s: i32,
) -> (Vec<String>, Vec<LineCtx>, &'static str) {
    // Helper: plain lines flow through the markdown fence machinery.
    let fenced = |lines: Vec<String>, ph| {
        let ctxs = fence_states(&lines);
        (lines, ctxs, ph)
    };
    match ctx {
        // Sub-tabs: 0 report (default/first), 1 transcript (tail-anchored),
        // 2 prompt, 3 info. Clamp guarantees the range, so `_` == report.
        DetailContext::Run { task } => match sub_tab {
            1 => fenced(
                run_files.map(|f| f.transcript_tail.clone()).unwrap_or_default(),
                "(no transcript yet)",
            ),
            2 => fenced(task.prompt.split('\n').map(str::to_string).collect(), "(no prompt)"),
            3 => match run_files.and_then(|f| f.meta.as_ref()) {
                Some(meta) => {
                    let (lines, ctxs) = run_info_lines(task, meta, now_epoch_s, tz_offset_s);
                    (lines, ctxs, "")
                }
                None => (Vec::new(), Vec::new(), "(no run recorded yet)"),
            },
            _ => {
                let report = run_files.map(|f| f.report.clone()).unwrap_or_default();
                let meta = run_files.and_then(|f| f.meta.as_ref());
                let (lines, ctxs) = report_content(report, meta);
                (lines, ctxs, "(no report yet)")
            }
        },
        DetailContext::Definition { .. } => match def {
            None => (Vec::new(), Vec::new(), "(loading definition…)"),
            Some(d) if sub_tab == 1 => {
                let (lines, key_col) = config_view(d);
                let ctxs = vec![LineCtx::Config { key_col }; lines.len()];
                (lines, ctxs, "")
            }
            Some(d) => fenced(d.prompt.split('\n').map(str::to_string).collect(), "(no prompt)"),
        },
        DetailContext::Worktree { row, lane_tasks } => {
            // Info block: aligned key/value rows styled like the config tab.
            let (mut lines, key_col) = align_kv(&worktree_rows(row, now_epoch_s, tz_offset_s));
            let mut ctxs: Vec<LineCtx> = vec![LineCtx::Config { key_col }; lines.len()];
            // Blank separator, then the lane-task list.
            lines.push(String::new());
            ctxs.push(LineCtx::Text);
            if lane_tasks.is_empty() {
                lines.push("(none)".to_string());
                ctxs.push(LineCtx::Text);
            } else {
                // Dim column-header row above the list (chrome, never a cursor
                // row). Its line text is a non-empty placeholder — the styler
                // regenerates the whole header from the width — because an empty
                // line short-circuits to a blank row in the renderer.
                lines.push("Task".to_string());
                ctxs.push(LineCtx::LaneHeader);
                // The row cursor always renders selected-style; clamp it so a
                // shrunk list still shows a highlighted row (design choice: the
                // detail pane has no separate focus concept, so the cursor row is
                // always visibly selected in the worktree view). `detail_row`
                // indexes `lane_tasks`, so the header line above does not shift it.
                let sel = detail_row.min(lane_tasks.len() - 1);
                // `#N in lane` counts queued tasks in creation order; `lane_tasks`
                // is already creation-ordered within the queued rank (ascending
                // id), so a running counter over the list yields each queued task's
                // position — matching the queue pane's snapshot-order count.
                let mut queued_seen = 0usize;
                for (i, t) in lane_tasks.iter().enumerate() {
                    let (glyph, name, is_def, epoch) = crate::selectors::lane_task_display(t);
                    let queue_pos = if t.status == TaskStatus::Queued {
                        queued_seen += 1;
                        queued_seen
                    } else {
                        0
                    };
                    lines.push(name);
                    ctxs.push(LineCtx::LaneTask {
                        glyph,
                        is_def,
                        created: crate::selectors::absolute_local_label(epoch, tz_offset_s),
                        age: crate::selectors::relative_age_label(epoch, now_epoch_s),
                        live: crate::selectors::lane_task_live(t, now_epoch_s, queue_pos),
                        selected: i == sel,
                    });
                }
            }
            (lines, ctxs, "")
        }
        DetailContext::Empty => (Vec::new(), Vec::new(), "(nothing selected)"),
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
        // Always white; focus is shown by the pane border, not the title color.
        Style::default().fg(p.fg).add_modifier(Modifier::BOLD)
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
    // Content padding so text isn't flush against the border: `pad` cols each
    // side and a one-row gap below the tab strip (or a top pad when tab-less).
    let pad = 2u16;
    let mut content_top = inner.y + 1;
    if !tabs.is_empty() {
        let mut x = inner.x; // tab strip is flush (no padding)
        let mut spans: Vec<Span> = Vec::new();
        for (i, label) in tabs.iter().enumerate() {
            let chip = format!(" {}:{} ", i + 1, label);
            let w = chip.chars().count() as u16;
            let style = if dimmed {
                p.dim_style()
            } else if i == sub_tab {
                Style::default().fg(p.selection_fg).bg(p.accent).add_modifier(Modifier::BOLD)
            } else {
                // Inactive sub-tab labels are white — tabs are chrome (the whole
                // row still dims via the `dimmed` arm when the pane is unfocused).
                Style::default().fg(p.fg)
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
        content_top = inner.y + 2;
    }
    let content_area = Rect {
        x: inner.x + pad,
        y: content_top,
        width: inner.width.saturating_sub(pad * 2),
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
            app.run_files.as_ref().filter(|(id, _)| id == &task.id).map(|(_, f)| f.as_ref())
        }
        _ => None,
    };

    // Timezone offset for the worktree info block's absolute `updated` stamp —
    // same source the queue pane's timestamps use.
    let tz_offset = chrono::Local::now().offset().local_minus_utc();
    let (lines, ctxs, placeholder) = content_for(
        &ctx,
        sub_tab,
        def.as_ref(),
        run_files,
        c.ui.detail_row,
        app.now_epoch_s,
        tz_offset,
    );
    if lines.is_empty() {
        app.detail_max_scroll.set(0);
        app.detail_wrapped_len.set(0);
        frame.render_widget(Paragraph::new(placeholder).style(p.dim_style()), content_area);
        return;
    }
    let bottom = bottom_anchored(kind, sub_tab);
    let height = content_area.height as usize;
    // `content_for` returns each line's `LineCtx` (markdown-fence state for
    // run/prompt views, aligned key/value for config + the worktree info block,
    // queue-style rows for the lane-task list), so styling below just dispatches
    // per segment — fence state is already resolved over the WHOLE content.
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
    // Register a click target per VISIBLE lane-task row (worktree detail): the
    // Nth non-continuation LaneTask display line maps to lane task N, so a click
    // selects + opens it (mirrors j/k + Enter). Pushed after PaneBody so it wins.
    {
        let mut ordinal = 0usize;
        for (i, d) in display.iter().enumerate() {
            if d.is_continuation || !matches!(d.ctx, LineCtx::LaneTask { .. }) {
                continue;
            }
            if i >= start && i < end {
                let y = content_area.y + (i - start) as u16;
                hits.push(
                    Rect { x: content_area.x, y, width: content_area.width, height: 1 },
                    HitTarget::DetailLaneTask(ordinal),
                );
            }
            ordinal += 1;
        }
    }
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
    // Worktree info block: the `pr #<n>` value is a clickable browser link when
    // the row carries an open PR and its url. Locate the pr line among the
    // (unwrapped-in-practice) Config display segments by an exact text match — a
    // wrap declines the link — underline the `#<n>` in link teal (pre-render),
    // and stash its geometry+url for a post-render OSC 8 injection ONLY while the
    // line sits inside the visible window.
    let mut pr_osc8: Option<(u16, u16, u16, String)> = None;
    if let DetailContext::Worktree { row, .. } = &ctx
        && let Some(link) = worktree_pr_link(row, app.now_epoch_s, tz_offset)
        && let Some(seg) = display
            .iter()
            .position(|d| {
                !d.is_continuation
                    && matches!(d.ctx, LineCtx::Config { .. })
                    && d.text == link.line_text
            })
            .filter(|&s| s >= start && s < end)
    {
        let vis = seg - start;
        let lo = link.value_col;
        let hi = link.value_col + link.value_len - 1;
        let link_style = Style::default().fg(p.meta).add_modifier(Modifier::UNDERLINED);
        styled[vis] = patch_line_cols(&styled[vis], lo, hi, link_style);
        pr_osc8 = Some((
            content_area.x + link.value_col as u16,
            content_area.y + vis as u16,
            link.value_len as u16,
            link.url,
        ));
    }
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

    // Wrap the freshly-painted `#<n>` pr value in an OSC 8 terminal hyperlink
    // (cmd+click opens it — the terminal handles it, not the app). Must run
    // after the paragraph paints so it rewrites the drawn glyph cells.
    if let Some((x, y, w, url)) = pr_osc8 {
        crate::view::apply_osc8(frame.buffer_mut(), x, y, w, &url);
    }

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
            Box::new(RunFiles { transcript_tail: transcript, report: vec![], ..Default::default() }),
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
        terminal.draw(|frame| hits = render_frame(&mut app, frame)).unwrap();
        (terminal, hits)
    }

    #[test]
    fn snapshot_detail_transcript() {
        // Transcript is now sub-tab index 1 (report is first).
        let (terminal, hits) = render_at(&detail_app(1), 80, 24);
        insta::assert_snapshot!("detail_transcript", terminal.backend());
        assert!(
            hits.iter().any(|(_, t)| *t == HitTarget::SubTab(1)),
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
            1,
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
        let (terminal, _hits) = render_at(&detail_app_transcript(lines, 1), 80, 24);
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
                description: None,
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

    /// Detail pane over a WORKTREES selection: the info block (path/branch/
    /// commit/author/updated/pr as aligned key/value rows, no `state`) followed by
    /// the lane's tasks as queue-style rows — running first (mauve def name), then
    /// queued (fg prompt summary), each with a right-pinned relative age. The
    /// first row renders selected-style (the default detail row cursor).
    fn detail_worktree_app() -> App {
        let mut app = fixture_app();
        let now = app.now_epoch_s;
        if let Some(w) = app
            .snapshot
            .as_mut()
            .and_then(|snap| snap.worktrees.get_mut("acme"))
            .and_then(|wts| wts.iter_mut().find(|w| w.name == "acme.feature"))
        {
            w.last_commit_hash = Some("a1b2c3d".to_string());
            w.last_commit_author = Some("Ian Chiu".to_string());
            w.last_commit_epoch = Some(now - 3 * 86_400);
            w.pr_number = Some(42);
        }
        let mut ui = TabUiState::default();
        ui.focus = PaneId::Detail;
        ui.last_list_pane = ListPane::Worktrees;
        // acme.feature sorts first (it has live task activity), so cursor 0 selects
        // it; its lane carries the running + queued fixture tasks.
        ui.selections[ListPane::Worktrees as usize].cursor = 0;
        app.ui_by_tab.insert("acme".to_string(), ui);
        app
    }

    #[test]
    fn snapshot_detail_worktree_info() {
        let (terminal, _hits) = render_at(&detail_worktree_app(), 80, 24);
        let body = terminal.backend().to_string();
        // Info block keys present; `state` is gone; git facts surfaced.
        assert!(body.contains("commit"), "commit row present");
        assert!(body.contains("a1b2c3d"), "short hash shown");
        assert!(body.contains("#42"), "PR number shown");
        assert!(!body.contains("state"), "state row dropped");
        insta::assert_snapshot!("detail_worktree_info", terminal.backend());
    }

    /// Detail pane over the run `info` sub-tab: a single finished run (so the
    /// queue cursor deterministically selects it) with a fully-populated
    /// `data.json` meta including a def snapshot → all four sections (Run/Timing/
    /// Details/Config) render.
    fn detail_info_app() -> App {
        use crate::ipc::types::TaskStatus;
        let mut app = fixture_app();
        // Anchor `now` just after this run's timestamps so the Timing rows show
        // meaningful relative ages (the shared fixture's `now` predates them).
        app.now_epoch_s = crate::selectors::parse_iso_epoch_s("2026-07-09T12:05:03.000Z");
        if let Some(snap) = app.snapshot.as_mut() {
            let mut t = snap.tasks[0].clone(); // 01RUN base (worktree acme.feature, tui)
            t.status = TaskStatus::Done;
            t.definition = Some("squash-merge".to_string());
            t.created = "2026-07-09T12:00:00.000Z".to_string();
            t.finished_at = Some("2026-07-09T12:03:20.000Z".to_string());
            snap.tasks = vec![t];
            snap.archived_recent = vec![];
            snap.running = vec![];
        }
        app.run_files = Some((
            "01RUN".to_string(),
            Box::new(RunFiles {
                session_id: Some("sess-abc123".to_string()),
                worktree_path: Some("/repos/acme.feature".to_string()),
                meta: Some(RunMeta {
                    started_at: Some("2026-07-09T12:00:05.000Z".to_string()),
                    finished_at: Some("2026-07-09T12:03:20.000Z".to_string()),
                    outcome: Some("done".to_string()),
                    reason: None,
                    exit_code: Some(0),
                    timed_out: false,
                    session_id: Some("sess-abc123".to_string()),
                    model: Some("claude-opus-4-8".to_string()),
                    resolved_worktree: Some("/repos/acme.feature".to_string()),
                    resolved_worktree_path: Some("/repos/acme.feature".to_string()),
                    cost_usd: Some(0.42),
                    turns: Some(37),
                    duration_ms: Some(195_000),
                    definition: Some(TaskDefinition {
                        name: "squash-merge".to_string(),
                        repo: "acme".to_string(),
                        description: Some("Squash-merge the branch.".to_string()),
                        dedup: "none".to_string(),
                        worktree: "auto".to_string(),
                        model: "opus".to_string(),
                        timeout_ms: 1_800_000,
                        priority: "normal".to_string(),
                        cron: Some("30 13 * * *".to_string()),
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            }),
        ));
        let mut ui = TabUiState::default();
        ui.focus = PaneId::Detail;
        ui.last_list_pane = ListPane::Queue;
        ui.sub_tab[DetailKind::Run as usize] = 3; // info sub-tab
        app.ui_by_tab.insert("acme".to_string(), ui);
        app
    }

    #[test]
    fn snapshot_detail_run_info() {
        // Taller viewport so all four sections fit on one screen (the info panel
        // runs ~26 lines).
        let (terminal, hits) = render_at(&detail_info_app(), 80, 34);
        let body = terminal.backend().to_string();
        // All four sections present; def name surfaced; info chip clickable.
        for header in ["Run", "Timing", "Details", "Config"] {
            assert!(body.contains(header), "{header} section header present");
        }
        assert!(body.contains("squash-merge"), "definition name shown");
        assert!(body.contains("$0.42"), "cost shown");
        assert!(
            hits.iter().any(|(_, t)| *t == HitTarget::SubTab(3)),
            "info sub-tab chip is clickable"
        );
        insta::assert_snapshot!("detail_run_info", terminal.backend());
    }

    /// Detail pane over the run `report` sub-tab (index 0, the default) on a
    /// `failed` run whose result text hit Claude's session limit — the exact
    /// `report.md` shape `finishRun` writes for the bug report this feature was
    /// built for. Verifies the `## Stats` bullets render as aligned Config rows
    /// (model gold-colored) instead of a plain markdown bullet list.
    fn detail_report_app() -> App {
        use crate::ipc::types::TaskStatus;
        let mut app = fixture_app();
        app.now_epoch_s = crate::selectors::parse_iso_epoch_s("2026-07-12T23:30:29.000Z");
        if let Some(snap) = app.snapshot.as_mut() {
            let mut t = snap.tasks[0].clone(); // 01RUN base (worktree acme.feature, tui)
            t.status = TaskStatus::Failed;
            t.error = Some("session limit".to_string());
            t.created = "2026-07-12T23:11:31.000Z".to_string();
            t.finished_at = Some("2026-07-12T23:30:29.000Z".to_string());
            snap.tasks = vec![t];
            snap.archived_recent = vec![];
            snap.running = vec![];
        }
        app.run_files = Some((
            "01RUN".to_string(),
            Box::new(RunFiles {
                report: [
                    "# Result",
                    "",
                    "You've hit your session limit · resets 1pm (America/Chicago)",
                    "",
                    "## Stats",
                    "- outcome: failed (exit code 1)",
                    "- model: claude-fable-5",
                    "- cost: $31.07151099999998",
                    "- turns: 40",
                    "- duration: 1129s",
                    "",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
                meta: Some(RunMeta {
                    outcome: Some("failed".to_string()),
                    reason: Some("exit code 1".to_string()),
                    exit_code: Some(1),
                    model: Some("claude-fable-5".to_string()),
                    cost_usd: Some(31.07151099999998),
                    turns: Some(40),
                    duration_ms: Some(1_129_000),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        ));
        let mut ui = TabUiState::default();
        ui.focus = PaneId::Detail;
        ui.last_list_pane = ListPane::Queue;
        ui.sub_tab[DetailKind::Run as usize] = 0; // report sub-tab (default)
        app.ui_by_tab.insert("acme".to_string(), ui);
        app
    }

    #[test]
    fn snapshot_detail_run_report_stats() {
        let (terminal, _hits) = render_at(&detail_report_app(), 80, 24);
        let body = terminal.backend().to_string();
        assert!(body.contains("claude-fable-5"), "model value shown");
        assert!(body.contains("session limit"), "the raw result text is untouched markdown");
        assert!(!body.contains("- outcome"), "literal bullet dash replaced");
        assert!(!body.contains("- model"), "literal bullet dash replaced");
        insta::assert_snapshot!("detail_run_report_stats", terminal.backend());
    }

    /// Minimal live task for the `run_info_lines` unit tests.
    fn info_task(status: TaskStatus) -> TaskInstance {
        TaskInstance {
            id: "01RUN".to_string(),
            status,
            definition: Some("squash-merge".to_string()),
            created: "2026-07-09T12:00:00.000Z".to_string(),
            ..Default::default()
        }
    }

    // 2026-07-09T12:05:03Z, matching fixture_app; tz is arbitrary for value checks.
    const INFO_NOW: u64 = 1_752_062_703;
    const INFO_TZ: i32 = -18_000;

    #[test]
    fn run_info_lines_empty_meta() {
        // No run record yet: sections still render, but unfinished fields dash out
        // and there is no Config section (no def snapshot) and no error/reason row.
        let task = info_task(TaskStatus::Queued);
        let (lines, ctxs) = run_info_lines(&task, &RunMeta::default(), INFO_NOW, INFO_TZ);
        assert!(lines.iter().any(|l| l == "Run"));
        assert!(lines.iter().any(|l| l == "Timing"));
        assert!(lines.iter().any(|l| l == "Details"));
        assert!(!lines.iter().any(|l| l == "Config"), "no Config without a def snapshot");
        assert_eq!(ctxs.iter().filter(|c| matches!(c, LineCtx::Header)).count(), 3);
        assert!(lines.iter().any(|l| l.contains("01RUN")), "id row");
        for key in ["started", "finished", "duration", "exit code", "cost", "turns"] {
            assert!(
                lines.iter().any(|l| l.trim_start().starts_with(key) && l.contains(EM_DASH)),
                "{key} dashes out"
            );
        }
        assert!(!lines.iter().any(|l| l.trim_start().starts_with("error")));
        assert!(!lines.iter().any(|l| l.trim_start().starts_with("reason")));
        assert!(!lines.iter().any(|l| l.trim_start().starts_with("timed out")));
    }

    #[test]
    fn run_info_lines_finished_run() {
        let task = info_task(TaskStatus::Done);
        let meta = RunMeta {
            started_at: Some("2026-07-09T12:00:05.000Z".to_string()),
            finished_at: Some("2026-07-09T12:03:20.000Z".to_string()),
            outcome: Some("done".to_string()),
            exit_code: Some(0),
            session_id: Some("sess-abc123".to_string()),
            model: Some("claude-opus-4-8".to_string()),
            resolved_worktree: Some("/repos/acme.feature".to_string()),
            resolved_worktree_path: Some("/repos/acme.feature".to_string()),
            cost_usd: Some(0.42),
            turns: Some(37),
            duration_ms: Some(195_000),
            definition: Some(TaskDefinition {
                worktree: "auto".to_string(),
                dedup: "none".to_string(),
                timeout_ms: 1_800_000,
                priority: "normal".to_string(),
                description: Some("Squash-merge the branch.".to_string()),
                cron: Some("30 13 * * *".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let (lines, _) = run_info_lines(&task, &meta, INFO_NOW, INFO_TZ);
        assert!(lines.iter().any(|l| l == "Config"), "Config section present with a def");
        assert!(lines.iter().any(|l| l.contains("$0.42")), "cost shown");
        assert!(lines.iter().any(|l| l.trim_start().starts_with("turns") && l.contains("37")));
        assert!(lines.iter().any(|l| l.trim_start().starts_with("duration") && l.contains("3m")));
        assert!(lines.iter().any(|l| l.contains("Squash-merge the branch.")), "description row");
        assert!(lines.iter().any(|l| l.trim_start().starts_with("cron") && l.contains("30 13")));
        assert!(!lines.iter().any(|l| l.trim_start().starts_with("timed out")), "false → no row");
    }

    #[test]
    fn run_info_lines_failed_run_with_reason() {
        let mut task = info_task(TaskStatus::Failed);
        task.error = None; // no live error → falls back to the run record's reason
        let meta = RunMeta {
            outcome: Some("failed".to_string()),
            reason: Some("timed out waiting".to_string()),
            exit_code: Some(1),
            timed_out: true,
            ..Default::default()
        };
        let (lines, _) = run_info_lines(&task, &meta, INFO_NOW, INFO_TZ);
        assert!(
            lines.iter().any(|l| l.trim_start().starts_with("reason") && l.contains("timed out waiting"))
        );
        assert!(!lines.iter().any(|l| l.trim_start().starts_with("error")), "no live error → reason used");
        assert!(lines.iter().any(|l| l.trim_start().starts_with("timed out") && l.contains("yes")));
        // Live error preempts the run record's reason.
        task.error = Some("boom".to_string());
        let (lines2, _) = run_info_lines(&task, &meta, INFO_NOW, INFO_TZ);
        assert!(lines2.iter().any(|l| l.trim_start().starts_with("error") && l.contains("boom")));
        assert!(!lines2.iter().any(|l| l.trim_start().starts_with("reason")), "error preempts reason");
    }

    #[test]
    fn stats_rows_formats_outcome_reason_and_dashes_missing_fields() {
        let meta = RunMeta {
            outcome: Some("failed".to_string()),
            reason: Some("exit code 1".to_string()),
            model: Some("claude-fable-5".to_string()),
            cost_usd: Some(31.07151099999998),
            turns: Some(40),
            duration_ms: Some(1_129_000),
            ..Default::default()
        };
        let rows = stats_rows(&meta);
        assert_eq!(
            rows,
            vec![
                ("outcome", "failed (exit code 1)".to_string()),
                ("model", "claude-fable-5".to_string()),
                ("cost", "$31.07151099999998".to_string()),
                ("turns", "40".to_string()),
                ("duration", "18m".to_string()), // format_duration, not raw seconds
            ]
        );
    }

    #[test]
    fn stats_rows_dashes_out_an_empty_meta() {
        let rows = stats_rows(&RunMeta::default());
        assert!(rows.iter().all(|(_, v)| v == EM_DASH), "every field absent → dash: {rows:?}");
    }

    #[test]
    fn report_content_replaces_stats_bullets_with_aligned_config_rows() {
        // A real report.md shape from `run-store.ts`'s `finishRun`.
        let report: Vec<String> = [
            "# Result",
            "",
            "You've hit your session limit · resets 1pm (America/Chicago)",
            "",
            "## Stats",
            "- outcome: failed (exit code 1)",
            "- model: claude-fable-5",
            "- cost: $31.07151099999998",
            "- turns: 40",
            "- duration: 1129s",
            "",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let meta = RunMeta {
            outcome: Some("failed".to_string()),
            reason: Some("exit code 1".to_string()),
            model: Some("claude-fable-5".to_string()),
            cost_usd: Some(31.07151099999998),
            turns: Some(40),
            duration_ms: Some(1_129_000),
            ..Default::default()
        };
        let (lines, ctxs) = report_content(report, Some(&meta));
        // Everything above the heading is untouched markdown.
        assert_eq!(lines[0], "# Result");
        assert_eq!(lines[2], "You've hit your session limit · resets 1pm (America/Chicago)");
        assert_eq!(lines[4], "## Stats");
        assert_eq!(ctxs[4], LineCtx::Text, "heading stays plain markdown (is_heading bolds it)");
        // The 5 literal `- key: value` bullets collapse to 5 aligned Config rows —
        // no leftover `- ` bullet text, and each carries LineCtx::Config.
        let bullet_rows = &lines[5..10];
        assert!(bullet_rows.iter().all(|l| !l.starts_with("- ")), "bullets replaced: {bullet_rows:?}");
        assert!(bullet_rows[1].contains("claude-fable-5"), "model value present: {bullet_rows:?}");
        for ctx in &ctxs[5..10] {
            assert!(matches!(ctx, LineCtx::Config { .. }), "stats rows use Config styling: {ctx:?}");
        }
        // Trailing blank line after the (now shorter) stats block is preserved.
        assert_eq!(lines.last().map(String::as_str), Some(""));
    }

    #[test]
    fn report_content_falls_back_to_plain_markdown_without_meta_or_heading() {
        let report = vec!["# Result".to_string(), "".to_string(), "still running".to_string()];
        // No meta at all (adhoc run mid-flight, or an old daemon).
        let (lines, _) = report_content(report.clone(), None);
        assert_eq!(lines, report);
        // Meta present but report.md predates the `## Stats` heading (or the run
        // hasn't reached `finishRun` yet) — leave the text alone rather than
        // silently dropping content that doesn't match the expected shape.
        let (lines2, _) = report_content(report.clone(), Some(&RunMeta::default()));
        assert_eq!(lines2, report);
    }

    #[test]
    fn detail_worktree_pr_is_an_osc8_link_only_with_a_url() {
        // The base fixture sets pr_number but no pr_url → the `#42` value is plain
        // text: no OSC 8 opener anywhere in the rendered buffer.
        let (terminal, _hits) = render_at(&detail_worktree_app(), 80, 24);
        let buf = terminal.backend().buffer();
        let has_opener = |buf: &ratatui::buffer::Buffer| {
            (buf.area.y..buf.area.bottom()).any(|y| {
                (buf.area.x..buf.area.right()).any(|x| buf[(x, y)].symbol().contains("\x1b]8;;"))
            })
        };
        assert!(!has_opener(buf), "pr number without a url gets no OSC 8 link");

        // Add the url: the `#42` value is wrapped in an OSC 8 terminal hyperlink
        // carrying it (folded into the first glyph cell), and reads as a link
        // (underlined). The terminal — not the app — handles cmd+click.
        let mut app = detail_worktree_app();
        let url = "https://github.com/acme/acme/pull/42".to_string();
        if let Some(w) = app
            .snapshot
            .as_mut()
            .and_then(|snap| snap.worktrees.get_mut("acme"))
            .and_then(|wts| wts.iter_mut().find(|w| w.name == "acme.feature"))
        {
            w.pr_url = Some(url.clone());
        }
        let (terminal, _hits) = render_at(&app, 80, 24);
        let buf = terminal.backend().buffer();
        let opener = format!("\x1b]8;;{url}\x1b\\");
        let mut found: Option<(u16, u16)> = None;
        let mut count = 0usize;
        for y in buf.area.y..buf.area.bottom() {
            for x in buf.area.x..buf.area.right() {
                if buf[(x, y)].symbol().contains(&opener) {
                    count += 1;
                    found = Some((x, y));
                }
            }
        }
        assert_eq!(count, 1, "exactly one OSC 8 link cell");
        let (x, y) = found.expect("OSC 8 link cell present");
        let sym = buf[(x, y)].symbol();
        assert!(sym.contains("#42"), "the wrapped glyphs are #42: {sym:?}");
        assert!(sym.ends_with("\x1b]8;;\x1b\\"), "closer present: {sym:?}");
        assert!(
            buf[(x, y)].modifier.contains(Modifier::UNDERLINED),
            "the #42 link cell is underlined"
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
        let mut app = detail_app_transcript(vec!["x".repeat(2000)], 1);
        app.size = (80, 24);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| {
            render_frame(&mut app, frame);
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
        // sub_tab 9 on a Run context clamps to the last valid index (3 = info),
        // NOT the report the `_` fall-through would hit with an unclamped index.
        // The fixture has no run meta, so info shows its own placeholder — text
        // distinct from the report placeholder proves the clamp landed on info.
        let (terminal, _hits) = render_at(&detail_app(9), 80, 24);
        let body = terminal.backend().to_string();
        assert!(body.contains("(no run recorded yet)"), "clamped to the info sub-tab");
        assert!(!body.contains("(no report yet)"), "clamped index is not the report fall-through");
    }
}
