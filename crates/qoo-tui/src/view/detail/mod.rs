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
use crate::ipc::types::{CatalogEntry, TaskDefinition, TaskInstance, TaskStatus};
use crate::runfiles::RunMeta;
use crate::markup::{
    DisplayLine, LineCtx, fence_states, fence_states_from, style_display_line,
    wrap_lines,
};
use crate::selectors::arg_summary;
use crate::view::Computed;
use crate::view::theme::{Palette, TITLE_DETAIL};

/// Blank-cell placeholder shown for an absent value (dimmed by the styler).
const EM_DASH: &str = "—";
/// Minimum gap between the aligned key column and the value column.
const CONFIG_KEY_GAP: usize = 2;
/// Two-space indent under each `info` sub-tab section header.
const INFO_INDENT: &str = "  ";

/// Compact human-readable token count for the `tokens` row: the bare number
/// below 1000, rounded to the nearest thousand with a `k` suffix below
/// 1,000,000, one decimal place with an `M` suffix at or above 1,000,000.
/// Mirrors `formatTokenCount` in `run-store.ts` so the TUI and report.md agree
/// on the same rendering. Pure — the ideal unit-test target.
fn compact_count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    if n < 1_000_000 {
        return format!("{}k", (n as f64 / 1000.0).round() as u64);
    }
    format!("{:.1}M", n as f64 / 1_000_000.0)
}

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

/// `(key, value)` rows for the definition **config** sub-tab — mirrors the
/// authored `config.yaml` surface (every field the daemon loads), with
/// `model` shown as the FULL RESOLVED chain under the operator's active
/// provider (same `resolveModelChain` as the run dialog). Absent optional
/// fields render as the dim `—` placeholder so the layout stays stable.
fn config_rows(
    def: &TaskDefinition,
    owned: &crate::selectors::ModelResolveOwned,
) -> Vec<(&'static str, String)> {
    let or_dash = |s: Option<&str>| {
        s.filter(|v| !v.is_empty())
            .map(|v| v.to_string())
            .unwrap_or_else(|| EM_DASH.to_string())
    };
    let disp = |r: &str| crate::chain::model_ref_display(&owned.catalog, r);
    let defaults = owned.default_models.refs_for(&def.repo);
    let enabled: Vec<&str> = owned.enabled_providers.iter().map(String::as_str).collect();
    let model = match crate::chain::resolved_model_chain_display(
        def.model.as_ref(),
        &owned.catalog,
        &enabled,
        &defaults,
        &owned.active_provider,
    ) {
        Some(s) => s,
        // Authored but unresolvable → show the authored refs so the operator
        // still sees what the def asked for. No model + nothing runnable → dash.
        None => match &def.model {
            Some(m) => m.refs().iter().map(|r| disp(r)).collect::<Vec<_>>().join(" → "),
            None => EM_DASH.to_string(),
        },
    };
    let discovery = match &def.discovery {
        Some(d) => {
            if d.item_key.is_empty() {
                d.command.clone()
            } else {
                format!("{}  ·  item_key: {}", d.command, d.item_key)
            }
        }
        None => EM_DASH.to_string(),
    };
    let purge = match def.purge_after_days {
        Some(n) => format!("{n}d"),
        None => EM_DASH.to_string(), // workspace default applies
    };
    vec![
        // Identity first so the operator sees which def the rows describe without
        // glancing back at the TASKS list (especially with the detail pane tall
        // enough that the selection is scrolled off).
        ("name", def.name.clone()),
        ("description", or_dash(def.description.as_deref())),
        ("args", if def.args.is_empty() { EM_DASH.to_string() } else { arg_summary(&def.args) }),
        ("worktree", def.worktree.clone()),
        ("lane", or_dash(def.lane.as_deref())),
        ("dedup", def.dedup.clone()),
        ("cron", or_dash(def.cron.as_deref())),
        ("discovery", discovery),
        ("model", model),
        ("timeout", format_duration(def.timeout_ms)),
        ("priority", def.priority.clone()),
        ("pre_run", or_dash(def.pre_run.as_deref())),
        ("post_run", or_dash(def.post_run.as_deref())),
        ("verify", or_dash(def.verify.as_deref())),
        (
            "on_done",
            def.on_done
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("stay")
                .to_string(),
        ),
        ("purge_after_days", purge),
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
fn config_view(
    def: &TaskDefinition,
    owned: &crate::selectors::ModelResolveOwned,
) -> (Vec<String>, usize) {
    align_kv(&config_rows(def, owned))
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
    catalog: &[CatalogEntry],
    now_epoch_s: u64,
    tz_offset_s: i32,
) -> (Vec<String>, Vec<LineCtx>) {
    let dash = || EM_DASH.to_string();
    let or_dash = |v: Option<String>| v.filter(|s| !s.is_empty()).unwrap_or_else(dash);
    // The recorded model is the raw resolved id (e.g. `claude-opus-4-8`), not a
    // `provider/label` ref. Resolve it against the catalog for `label (provider)`
    // when the id is a known entry; append ` · <id>` only when the raw id
    // differs from the catalog label (pins / CLI ids that aren't the short
    // versioned label). Fall back to the bare id alone when it isn't known
    // (unknown provider, or a stale/removed catalog entry — the raw id is
    // still the ground truth either way). Catalog labels are now versioned
    // (`claude-sonnet-5`, `grok-4.5`) so label often equals id — showing both
    // was redundant (`claude-sonnet-5 (claude) · claude-sonnet-5`).
    let model_display = |id: &str| match catalog.iter().find(|e| e.id == id) {
        Some(entry) if entry.id != entry.label => {
            format!("{} · {id}", entry.model_display())
        }
        Some(entry) => entry.model_display(),
        None => id.to_string(),
    };
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
        (
            "model",
            or_dash(
                meta.model
                    .clone()
                    .or_else(|| {
                        task.model.as_ref().and_then(|m| m.refs().into_iter().next())
                    })
                    .map(|id| model_display(&id)),
            ),
        ),
        ("exit code", meta.exit_code.map(|c| c.to_string()).unwrap_or_else(dash)),
    ];
    if meta.timed_out {
        details.push(("timed out", "yes".to_string()));
    }
    details.push(("cost", meta.cost_usd.map(|c| format!("${c}")).unwrap_or_else(dash)));
    // ADDITIONAL to cost, not a replacement: a provider (grok) can report token
    // counts with no priced cost, so `cost` stays `—` while `tokens` still has a
    // value — the whole reason this row exists. Only dashes out when NEITHER
    // side was reported at all; a single present side still renders (the other
    // side dashes independently), since the two counts come off the provider's
    // usage object independently of each other.
    if meta.input_tokens.is_some() || meta.output_tokens.is_some() {
        let in_str = meta.input_tokens.map(compact_count).unwrap_or_else(dash);
        let out_str = meta.output_tokens.map(compact_count).unwrap_or_else(dash);
        details.push(("tokens", format!("{in_str} in / {out_str} out")));
    } else {
        details.push(("tokens", dash()));
    }
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn content_for(
    ctx: &DetailContext,
    sub_tab: usize,
    def: Option<&TaskDefinition>,
    run_files: Option<&crate::runfiles::RunFiles>,
    detail_row: usize,
    owned: &crate::selectors::ModelResolveOwned,
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
            1 => {
                // The transcript is a TAIL window: seed the fence machinery with
                // whether that window began mid-fence, or prose after the cut is
                // mis-styled as code and vice versa. Report/prompt read from line 0.
                let lines = run_files.map(|f| f.transcript_tail.clone()).unwrap_or_default();
                let starts_in_fence =
                    run_files.is_some_and(|f| f.transcript_starts_in_fence);
                let ctxs = fence_states_from(&lines, starts_in_fence);
                (lines, ctxs, "(no transcript yet)")
            }
            2 => fenced(task.prompt.split('\n').map(str::to_string).collect(), "(no prompt)"),
            // Always render from the live task — even before a run dir/`data.json`
            // exists (queued / just-started). Missing meta fields dash out; the
            // Config section only appears once a def snapshot is recorded.
            3 => {
                let empty = RunMeta::default();
                let meta = run_files.and_then(|f| f.meta.as_ref()).unwrap_or(&empty);
                let (lines, ctxs) =
                    run_info_lines(task, meta, &owned.catalog, now_epoch_s, tz_offset_s);
                (lines, ctxs, "")
            }
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
                let (lines, key_col) = config_view(d, owned);
                let ctxs = vec![LineCtx::Config { key_col }; lines.len()];
                (lines, ctxs, "")
            }
            // Sub-tab 2: the full multi-line discovery command + item key
            // template. The config tab keeps its one-line `discovery` row; this
            // tab exists because real discovery commands don't fit one line.
            Some(d) if sub_tab == 2 => match &d.discovery {
                Some(disc) => {
                    let mut lines: Vec<String> =
                        disc.command.split('\n').map(str::to_string).collect();
                    if !disc.item_key.is_empty() {
                        lines.push(String::new());
                        lines.push(format!("item key: {}", disc.item_key));
                    }
                    fenced(lines, "")
                }
                None => (Vec::new(), Vec::new(), "(no discovery)"),
            },
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
                        live: crate::selectors::lane_task_live(
                            t,
                            now_epoch_s,
                            queue_pos,
                            tz_offset_s,
                        ),
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

/// Columns reserved on the right when the detail scrollbar shows: 1 empty
/// gutter so text never kisses the thumb (user feedback — flush right edge
/// was hard to read) + 1 for the track itself. Matches juice discuss's
/// `SCROLLBAR_MARGIN + 1` reserve.
const DETAIL_SCROLLBAR_RESERVE: usize = 2;

/// Wrap `lines` for a `width`×`height` viewport, resolving the scrollbar
/// chicken-and-egg: whether the scrollbar shows depends on the wrapped count,
/// which depends on the width its column steals. Two deterministic passes — wrap
/// at full width; if that overflows the viewport the scrollbar shows, so re-wrap
/// [`DETAIL_SCROLLBAR_RESERVE`] columns narrower (narrower can only add segments,
/// so the overflow verdict never flips back). Returns the display lines, whether
/// a scrollbar is needed, and the text width fence rules must be sized to.
fn wrap_for_viewport(
    lines: &[String],
    ctxs: &[LineCtx],
    width: usize,
    height: usize,
) -> (Vec<DisplayLine>, bool, u16) {
    let display = wrap_lines(lines, ctxs, width);
    if display.len() > height {
        // Prefer margin+track; fall back to track-only on very narrow panes.
        let reserve = DETAIL_SCROLLBAR_RESERVE.min(width.saturating_sub(1)).max(1);
        let text_w = width.saturating_sub(reserve).max(1);
        (wrap_lines(lines, ctxs, text_w), true, text_w as u16)
    } else {
        (display, false, width as u16)
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
    let owned = app.model_resolve_owned();
    let (lines, ctxs, placeholder) = content_for(
        &ctx,
        sub_tab,
        def.as_ref(),
        run_files,
        c.ui.detail_row,
        &owned,
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
            // Table visual rows carry precomputed roles; fence rules still go
            // through `style_transcript_line` via [`style_display_line`].
            // `text_width` sizes fence rules clear of the scrollbar column.
            let mut line = style_display_line(seg, text_width, p);
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
mod tests;
