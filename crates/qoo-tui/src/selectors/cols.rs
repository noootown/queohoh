/// Resolved per-frame column widths for the QUEUE pane. A width of `0` (or
/// `false`) means the column is omitted for this frame; `summary_w` is the flex
/// remainder. Computed from the windowed (visible) rows so alignment tracks what
/// is actually on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueColLayout {
    pub worktree_w: usize,
    pub def_w: usize,
    pub summary_w: usize,
    pub show_timestamp: bool,
    /// `AGE_W` when the relative-age column is kept, else 0 (fixed width).
    pub age_w: usize,
    /// `QUEUE_LIVE_W` when the trailing live slot is kept, else 0 (fixed width).
    /// Renders live `⏱ <elapsed>` (from `running_elapsed` + now) or
    /// `row.detail` (`#N in lane` for queued).
    pub live_w: usize,
}

/// Fit the QUEUE columns into `avail` inner cells. The identity/content columns
/// (glyph, optional ⛓ chain, worktree, def) are sized to the widest visible value
/// (capped so the summary keeps room); the summary flexes into what remains. The
/// trailing timestamp / age / live columns have FIXED reserved widths (never
/// sized from row data) — their PRESENCE is decided purely by the width ladder,
/// so a row gaining a timer or a wider value never shifts any column. When space
/// is tight the trailing columns degrade in a fixed order — timestamp, then age,
/// then live — so the summary keeps at least `SUMMARY_MIN` cells; only if that
/// still isn't enough does def drop and then worktree shrink.
pub fn queue_col_layout(rows: &[QueueRow], avail: usize, _now_epoch_s: u64) -> QueueColLayout {
    let worktree_w = capped_max(rows.iter().map(|r| r.worktree.as_str()), WORKTREE_CAP);
    let mut def_w = capped_max(rows.iter().filter_map(|r| r.def_name.as_deref()), DEF_CAP);

    // Non-flex prefix width: glyph(1) + worktree(+gutter) + def(+gutter) + the
    // gutter before the summary. The summary itself is the remainder.
    let prefix = |worktree_w: usize, def_w: usize| -> usize {
        1 + if worktree_w > 0 { COL_GAP + worktree_w } else { 0 }
            + if def_w > 0 { COL_GAP + def_w } else { 0 }
            + COL_GAP
    };
    // Summary width given the current column choices (may be negative → too tight).
    // Trailing columns are fixed-width: timestamp=TIMESTAMP_W, age=AGE_W,
    // live=QUEUE_LIVE_W — each present as a bool.
    let summary_of =
        |worktree_w: usize, def_w: usize, show_ts: bool, age_w: usize, live_w: usize| -> isize {
            let mut used = prefix(worktree_w, def_w) as isize;
            if show_ts {
                used += (COL_GAP + TIMESTAMP_W) as isize;
            }
            if age_w > 0 {
                used += (COL_GAP + age_w) as isize;
            }
            if live_w > 0 {
                used += (COL_GAP + live_w) as isize;
            }
            avail as isize - used
        };

    let min = SUMMARY_MIN as isize;
    let mut show_timestamp = true;
    let mut age_w = AGE_W;
    let mut live_w = QUEUE_LIVE_W;
    let mut worktree_w = worktree_w;

    // Trailing columns degrade first: timestamp, then age, then live.
    if summary_of(worktree_w, def_w, show_timestamp, age_w, live_w) < min {
        show_timestamp = false;
    }
    if summary_of(worktree_w, def_w, show_timestamp, age_w, live_w) < min {
        age_w = 0;
    }
    if summary_of(worktree_w, def_w, show_timestamp, age_w, live_w) < min {
        live_w = 0;
    }
    // Still cramped → drop def, then shrink worktree toward the summary floor.
    if summary_of(worktree_w, def_w, show_timestamp, age_w, live_w) < min {
        def_w = 0;
    }
    let s = summary_of(worktree_w, def_w, show_timestamp, age_w, live_w);
    if s < min && worktree_w > 0 {
        worktree_w = worktree_w.saturating_sub((min - s) as usize);
    }
    let summary_w = summary_of(worktree_w, def_w, show_timestamp, age_w, live_w).max(0) as usize;

    QueueColLayout { worktree_w, def_w, summary_w, show_timestamp, age_w, live_w }
}

/// Minimum name column the worktree detail lane-task rows keep before a trailing
/// column is dropped to make room.
const LANE_NAME_MIN: usize = 6;

/// Resolved column widths for one worktree-detail lane-task row: the flex `Task`
/// name (`name_w`) after the `<glyph> ` prefix, then the fixed trailing columns
/// `Created` (`TIMESTAMP_W`), `Age` (`AGE_W`), `Live` (`QUEUE_LIVE_W`) — the same
/// widths and `COL_GAP` gutters the QUEUE pane uses. A width of `0` omits that
/// column. Shared by the row and header stylers so the two align cell-for-cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LaneTaskCols {
    pub name_w: usize,
    pub created_w: usize,
    pub age_w: usize,
    pub live_w: usize,
}

/// Fit a lane-task row into `width` cells. The trailing columns are fixed-width
/// (never sized from row data) and degrade in a fixed order — Live, then Created,
/// then Age — so the `Task` name keeps at least [`LANE_NAME_MIN`] cells; the name
/// is the flex remainder. Pure over `width` (the ideal unit-test target).
pub(crate) fn lane_task_cols(width: usize) -> LaneTaskCols {
    const PREFIX: usize = 2; // `<glyph> ` (glyph + one space)
    let mut created_w = TIMESTAMP_W;
    let mut age_w = AGE_W;
    let mut live_w = QUEUE_LIVE_W;
    let trailing = |c: usize, a: usize, l: usize| {
        (if c > 0 { COL_GAP + c } else { 0 })
            + (if a > 0 { COL_GAP + a } else { 0 })
            + (if l > 0 { COL_GAP + l } else { 0 })
    };
    // Drop trailing columns (live → created → age) until the name floor fits.
    for op in 0..3 {
        if PREFIX + LANE_NAME_MIN + trailing(created_w, age_w, live_w) <= width {
            break;
        }
        match op {
            0 => live_w = 0,
            1 => created_w = 0,
            _ => age_w = 0,
        }
    }
    let name_w = width.saturating_sub(PREFIX + trailing(created_w, age_w, live_w));
    LaneTaskCols { name_w, created_w, age_w, live_w }
}

/// Resolved column widths for the TASKS pane: `name | model | description |
/// schedule`. `name_w`/`model_w` are content-capped columns; `desc_w` is the
/// FILL (the remainder, like the queue pane's summary — prose gets the slack),
/// 0 when no visible def has a description or the pane is too narrow to spare
/// any. `model_w` sits right after the name (user request: the model matters
/// more than anything else on the row), pane-gated (reserved only while some
/// visible def carries a model, blank on a def without one so the columns never
/// slide). The args column was dropped from the row entirely (user request —
/// args still show in the def picker rows and the detail config tab). The
/// schedule stays the trailing capped column (`sched_w` sizes the humanized
/// cron — see [`def_sched_text`]). Narrow-pane drop order:
/// the desc FILL shrinks to 0 first, then the model column drops, then `name_w`
/// shrinks last; the schedule column is always kept. A width of 0 means that
/// column is omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefColLayout {
    pub name_w: usize,
    pub desc_w: usize,
    pub model_w: usize,
    pub sched_w: usize,
    /// Front `⌕` discovery-marker slot — 2 cells, glyph + separator — reserved
    /// pane-wide when any visible def has discovery; 0 otherwise. Mirrors the
    /// worktree `±` dirty slot (`WtColLayout::dirty_w`).
    pub marker_w: usize,
}

/// The description cell text for a def ("" when it has none). Prose, rendered in
/// plain fg and filling the remaining pane width (truncated with `…` when tight).
pub fn def_desc_text(def: &DefinitionSummary) -> String {
    def.description.clone().unwrap_or_default()
}

/// The model cell text for a def in the TASKS list: the **effective head**
/// under `ctx.active_provider` only (stable re-head + default-model /
/// group-head prepend — see [`crate::chain::resolve_model_chain`] /
/// [`crate::chain::effective_model_head`]), label-only via
/// [`crate::chain::model_ref_display`]. Not the authored yaml list and not the
/// full `a → b → c` fallback chain (that stays on the detail config pane via
/// [`crate::chain::resolved_model_chain_display`]). `None` model uses the
/// repo's `default_models`. Empty string when resolution fails (unknown model /
/// nothing runnable) so the pane-gate can drop the column when every visible
/// def is blank. Layout ([`def_col_layout`]) and render
/// ([`crate::view::panes`]) share this so widths track the displayed head.
pub fn def_model_text(def: &DefinitionSummary, ctx: &ModelResolveCtx<'_>) -> String {
    let defaults = ctx.default_models.refs_for(&def.repo);
    let enabled = ctx.enabled_refs();
    match effective_model_head(
        def.model.as_ref(),
        ctx.catalog,
        &enabled,
        &defaults,
        ctx.active_provider,
    ) {
        Some(head) => model_ref_display(ctx.catalog, &head),
        None => String::new(),
    }
}

/// Trailing schedule-cell text for a def row: the humanized cron schedule, or
/// empty when the def has none. Single source for BOTH the layout width
/// ([`def_col_layout`]) and the rendered cell ([`crate::view::panes`]). The
/// `⌕` discovery marker lives in the row's front marker slot (`marker_w`),
/// not here.
pub fn def_sched_text(def: &DefinitionSummary) -> String {
    def.cron.as_deref().and_then(cron_human).unwrap_or_default()
}

pub fn def_col_layout(
    rows: &[DefinitionSummary],
    avail: usize,
    ctx: &ModelResolveCtx<'_>,
) -> DefColLayout {
    let name_w0 = capped_max(rows.iter().map(|d| d.name.as_str()), WORKTREE_CAP);
    let sched_w = rows.iter().map(|d| cw(&def_sched_text(d))).max().unwrap_or(0).min(SCHED_CAP);
    // Trailing schedule column footprint (right-pinned by the desc fill): the
    // humanized cron (see `def_sched_text` — layout and render share it).
    // Blank for a def with none.
    let has_sched = sched_w > 0;
    let sched_col = if has_sched { sched_w } else { 0 };
    // Front `⌕` discovery-marker slot: 2 cells (glyph + separator), reserved
    // pane-wide when any visible def has discovery — mirrors the worktree `±`
    // dirty slot. It sits before the name with no COL_GAP of its own (the slot
    // already embeds its separator space).
    let marker_w = if rows.iter().any(|d| d.has_discovery) { 2 } else { 0 };
    // The desc FILL is present only when some visible def actually has a
    // description (else the schedule keeps its today-position, no layout shift).
    let has_desc = rows.iter().any(|d| d.description.as_deref().is_some_and(|s| !s.is_empty()));
    // Model column: fixed, pane-gated on whole-pane data (widest *effective
    // head* cell, 0 pane-wide when every visible def resolves to blank).
    let model_w0 = rows
        .iter()
        .map(|d| cw(&def_model_text(d, ctx)))
        .max()
        .unwrap_or(0)
        .min(MODEL_CAP);

    // Cells used by the fixed (non-fill) columns for a given name/model width.
    let used_wo_desc = |name_w: usize, model_w: usize| -> usize {
        marker_w
            + name_w
            + if model_w > 0 { COL_GAP + model_w } else { 0 }
            + if sched_col > 0 { COL_GAP + sched_col } else { 0 }
    };
    // Reclaim when even the fixed columns overflow: drop model, then shrink
    // name. (The desc fill has already implicitly shrunk to 0 — it is only ever
    // the leftover remainder below.)
    let mut model_w = model_w0;
    if used_wo_desc(name_w0, model_w) > avail {
        model_w = 0;
    }
    let mut name_w = name_w0;
    let u = used_wo_desc(name_w, model_w);
    if u > avail {
        name_w = name_w.saturating_sub(u - avail);
    }
    // Description is the FILL: the remainder after name/model/schedule and its
    // leading gutter. Zero when absent or when nothing is left to give it.
    let desc_w = if has_desc {
        avail.saturating_sub(used_wo_desc(name_w, model_w) + COL_GAP)
    } else {
        0
    };

    DefColLayout { name_w, desc_w, model_w, sched_w, marker_w }
}

/// The Author cell text: the PR author (`pr_author`) when known, else the
/// last-commit author. `pr_author` wins because a squash-merged branch's local
/// HEAD is often an automation merge commit whose author isn't the PR author —
/// the PR author is the meaningful attribution. None when neither is supplied
/// (an old daemon, no PR, or a worktree whose `git log` failed) — the whole
/// column is then omitted pane-wide.
pub fn wt_author_text(row: &WorktreeRow) -> Option<String> {
    row.pr_author.clone().or_else(|| row.last_commit_author.clone())
}

/// The front-column status marker a worktree row shows in the shared `↣` slot,
/// or `None` for a blank slot. Only one glyph shows; precedence is
/// **merge > approve > ready-for-review > WIP** (each higher fact subsumes the
/// ones below). Pure so the precedence is unit-testable away from the view,
/// which only maps the variant to a glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WtMergeMarker {
    /// Committed work is on the default branch (or the PR merged) — `↣`, green.
    Merged,
    /// The PR's review decision is APPROVED but it isn't merged yet — `✓`, green.
    Approved,
    /// PR has the `ready-for-review` label (and is not merged/approved) — `◎`.
    ReadyForReview,
    /// PR has the `WIP` label (and none of the higher markers) — `✎`.
    Wip,
}

/// Resolve a row's front merge/label marker. Precedence:
/// merge > approve > ready-for-review > WIP. Higher facts win so e.g. an
/// approved-then-merged PR keeps showing the merged glyph, and a PR with both
/// `ready-for-review` and `WIP` labels shows `◎` not `✎`.
pub fn wt_merge_marker(row: &WorktreeRow) -> Option<WtMergeMarker> {
    if row.merged == Some(true) {
        Some(WtMergeMarker::Merged)
    } else if row.approved == Some(true) {
        Some(WtMergeMarker::Approved)
    } else if row.ready_for_review == Some(true) {
        Some(WtMergeMarker::ReadyForReview)
    } else if row.wip == Some(true) {
        Some(WtMergeMarker::Wip)
    } else {
        None
    }
}

/// Resolved per-frame column widths for the WORKTREES pane. A width of `0` means
/// the column is omitted this frame.
///
/// Columns, left→right (identity → content → time → activity):
///   `● ± ⛨ ↣ name` (anchor; the `±` dirty, `⛨` protected and `↣` merged-back
///   markers are single-cell front slots after the dot, per user request),
///   last-finished (FILL), PR `#<n>` (fixed `PR_W`), last-commit author
///   (fixed `AUTHOR_W`), last-commit age (fixed `COMMIT_AGE_W`),
///   combined Next/Live activity (fixed `WT_ACTIVITY_W` — `⏱ …` and/or
///   `→ <name>`, right-pinned by the fill). The PR column sits immediately
///   LEFT of the author (between the fill and author) so the open-PR chip
///   reads before the who·when pair; the author sits right before the
///   commit-age so the pair reads `koshea  3d ago` = who · when.
///
/// The marker/time columns (`dirty`, `protected`, `pr`, `author`,
/// `commit_age`, `activity`) are FIXED widths — never sized from row data — so a
/// row gaining a value never shifts any column; `name_w` stays content-capped
/// and `last_w` is the FILL column (absorbs the remaining width, like the queue
/// pane's summary — per user request the last task's description gets the
/// slack). The front `±`/`⛨`/`↣` marker slots and the Live activity column are
/// ALWAYS reserved when the ladder keeps them (per user request — data-gated
/// slots made columns shift as scroll/data changed); pr/author/commit-age stay
/// pane-gated (reserved only while some visible row carries the value).
/// Degradation drop priority (first dropped first): commit-age → author → PR
/// → merged → protected → dirty → last-finished → activity; only after all of
/// those drop does `name_w` shrink. PR outlives author/commit-age dropping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WtColLayout {
    pub name_w: usize,
    pub dirty_w: usize,
    /// Front `⛨` protected-marker slot beside the `±` dirty slot — a single
    /// cell (plus its embedded separator) statically reserved like `dirty_w`,
    /// so a protected worktree shows both markers at once.
    pub protected_w: usize,
    /// Front `↣` merged-marker slot beside `±`/`⛨` — a single cell (plus its
    /// embedded separator) statically reserved like the other two, marking a
    /// worktree whose committed work is merged into the default branch.
    pub merged_w: usize,
    /// Combined Next/Live activity column (`WT_ACTIVITY_W` when kept, else 0).
    /// Renders the running `⏱` timer and/or the head-of-lane `→ <name>`.
    pub activity_w: usize,
    pub last_w: usize,
    /// PR `#<n>` column (`PR_W` when some visible row has an open PR, else 0).
    /// Positioned between the last-task fill and the author column.
    pub pr_w: usize,
    pub author_w: usize,
    pub commit_age_w: usize,
}

impl WtColLayout {
    /// Char offset from the row start to the first cell of the PR `#<n>` value —
    /// the SINGLE source of truth shared by `worktree_line` (which lays the cell
    /// out) and `render_rows` (which registers the click rect), so the two can
    /// never drift. Mirrors the span widths `worktree_line` pushes before the PR
    /// cell: the anchor (`● ` + the `±`/`⛨` front slots + the name), then the
    /// last-task FILL when present, then the `COL_GAP` before the PR column.
    /// Meaningless when `pr_w == 0` (the column is absent); callers gate on that.
    pub fn pr_col_x(&self) -> usize {
        let anchor = 2
            + if self.dirty_w > 0 { 2 } else { 0 }
            + if self.protected_w > 0 { 2 } else { 0 }
            + if self.merged_w > 0 { 2 } else { 0 }
            + self.name_w;
        let after_last = if self.last_w > 0 { COL_GAP + self.last_w } else { 0 };
        anchor + after_last + COL_GAP
    }
}

/// Fit the WORKTREES columns into `avail` inner cells (see [`WtColLayout`] for the
/// column order, fixed-width model, and drop priority). The last-task fill and
/// the Live activity column are always candidates; dirty is static too;
/// pr/author/commit-age stay gated on whole-pane data availability.
pub fn wt_col_layout(rows: &[WorktreeRow], avail: usize) -> WtColLayout {
    let name_w0 = capped_max(rows.iter().map(|r| r.name.as_str()), WORKTREE_CAP);
    // Fixed marker/time widths. The `±` front slot is statically reserved
    // (blank when a row has no value); author/commit-age/activity stay gated
    // on whole-pane data availability.
    // STATICALLY reserved (user request): gating this slot on visible-row data
    // made the name column shift whenever a dirty flag flipped or scrolling
    // changed which rows were visible. The width ladder may still drop it under
    // width pressure (geometry-driven, not data-driven).
    let dirty_w0 = if rows.is_empty() { 0 } else { 1 };
    // The `⛨` protected marker gets its own single-cell front slot beside the
    // `±` (per user request — same size, same column treatment), statically
    // reserved for the same no-shift reason.
    let protected_w0 = if rows.is_empty() { 0 } else { 1 };
    // The `↣` merged-back marker gets the third single-cell front slot (same
    // treatment as `±`/`⛨`), statically reserved for the same no-shift reason.
    let merged_w0 = if rows.is_empty() { 0 } else { 1 };
    // Combined Next/Live activity column: STATICALLY reserved at fixed
    // `WT_ACTIVITY_W` whenever the pane has rows (blank when a row has no
    // timer/next). User request: the Live column is always there so columns
    // never shift when a lane starts or queues work. The width ladder may still
    // drop it under geometry pressure (last optional before name shrinks).
    let activity_w0 = if rows.is_empty() { 0 } else { WT_ACTIVITY_W };
    let author_w0 = if rows.iter().any(|r| wt_author_text(r).is_some()) { AUTHOR_W } else { 0 };
    let commit_w0 = if rows.iter().any(|r| r.last_commit_epoch.is_some()) { COMMIT_AGE_W } else { 0 };
    // PR is pane-gated like author/commit-age: reserved only while some visible
    // row carries an open PR number. It survives author/commit-age dropping (it
    // drops third, after them) — an open PR is the more actionable signal.
    let pr_w0 = if rows.iter().any(|r| r.pr_number.is_some()) { PR_W } else { 0 };

    // Anchor width: `● ` (dot + space) + the `± ` (dirty), `⛨ ` (protected) and
    // `↣ ` (merged) front markers — a single cell + space each when present —
    // then the name. The markers sit up front per user request, not as mid-row
    // columns.
    let anchor = |name_w: usize, dirty: bool, protected: bool, merged: bool| {
        2 + if dirty { 2 } else { 0 }
            + if protected { 2 } else { 0 }
            + if merged { 2 } else { 0 }
            + name_w
    };
    // Used cells for a set of column widths and whether the last-task FILL is
    // reserved (at its `WT_LAST_MIN` floor — the actual fill absorbs the slack).
    // cols = [author, commit]; `pr` is the fixed PR column and `activity` the
    // trailing combined Next/Live column.
    let used = |name_w: usize,
                dirty: bool,
                protected: bool,
                merged: bool,
                cols: [usize; 2],
                pr_w: usize,
                activity_w: usize,
                last: bool|
     -> usize {
        let mut u = anchor(name_w, dirty, protected, merged);
        for w in cols {
            if w > 0 {
                u += COL_GAP + w;
            }
        }
        if pr_w > 0 {
            u += COL_GAP + pr_w;
        }
        if last {
            u += COL_GAP + WT_LAST_MIN;
        }
        if activity_w > 0 {
            u += COL_GAP + activity_w;
        }
        u
    };

    // Degrade in drop order: commit → author → pr → merged → protected → dirty
    // → last → activity. cols = [author(0), commit(1)]; PR drops after author
    // (so it outlives the who·when pair); among the three front slots merged
    // drops first (a cleanup hint), then protected, then dirty (the most
    // actionable). Activity (fixed WT_ACTIVITY_W) is the last optional to go.
    let mut cols = [author_w0, commit_w0];
    let mut pr_w = pr_w0;
    let mut dirty = dirty_w0 > 0;
    let mut protected = protected_w0 > 0;
    let mut merged = merged_w0 > 0;
    let mut activity_w = activity_w0;
    let mut last = true;
    #[derive(Clone, Copy)]
    enum Drop {
        Col(usize),
        Pr,
        Merged,
        Protected,
        Dirty,
        Last,
        Activity,
    }
    for op in [
        Drop::Col(1),
        Drop::Col(0),
        Drop::Pr,
        Drop::Merged,
        Drop::Protected,
        Drop::Dirty,
        Drop::Last,
        Drop::Activity,
    ] {
        if used(name_w0, dirty, protected, merged, cols, pr_w, activity_w, last) <= avail {
            break;
        }
        match op {
            Drop::Col(i) => cols[i] = 0,
            Drop::Pr => pr_w = 0,
            Drop::Merged => merged = false,
            Drop::Protected => protected = false,
            Drop::Dirty => dirty = false,
            Drop::Last => last = false,
            Drop::Activity => activity_w = 0,
        }
    }
    // Still too wide with only `● ± ⛨ ↣ name` left → shrink the name column.
    let mut name_w = name_w0;
    let u = used(name_w, dirty, protected, merged, cols, pr_w, activity_w, last);
    if u > avail {
        name_w = name_w.saturating_sub(u - avail);
    }
    // The last-task column is the FILL: the remainder after every reserved column
    // (≥ WT_LAST_MIN by construction, 0 when dropped). Its width is data-
    // independent — a lane finishing its first task changes only its own cell,
    // never any other column's offset — and the trailing activity column stays
    // right-pinned at the row edge.
    let last_w = if last {
        let base = used(name_w, dirty, protected, merged, cols, pr_w, activity_w, false);
        avail.saturating_sub(base + COL_GAP)
    } else {
        0
    };

    WtColLayout {
        name_w,
        dirty_w: if dirty { 1 } else { 0 },
        protected_w: if protected { 1 } else { 0 },
        merged_w: if merged { 1 } else { 0 },
        activity_w,
        last_w,
        pr_w,
        author_w: cols[0],
        commit_age_w: cols[1],
    }
}
