use std::collections::HashSet;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use throbber_widgets_tui::{Throbber, ThrobberState};

use crate::app::{App, ListPane, PaneId, Selection};
use crate::hit::{HitMap, HitTarget, PaneButton, pane_buttons};
use crate::ipc::types::DefinitionSummary;
use crate::selectors::{
    COLLAPSED_H, DefColLayout, QueueColLayout, QueueRow, WorktreeRow, WtColLayout,
    absolute_local_label, def_col_layout, elapsed_label, pad_clip,
    pane_layout, pane_title, queue_col_layout, queue_divider_after, relative_age_label,
    wt_author_text, wt_col_layout, wt_merge_marker, WtMergeMarker,
};
use crate::view::theme::{
    BTN_LABEL_ARCHIVE, BTN_LABEL_COLLAPSE, BTN_LABEL_CREATE, BTN_LABEL_DISCOVER, BTN_LABEL_EXPAND,
    BTN_LABEL_GOTO, BTN_LABEL_REMOVE, BTN_LABEL_RERUN, BTN_LABEL_RUN, BTN_LABEL_STOP, BTN_LABEL_TASKS,
    BTN_LABEL_UNARCHIVE,
    FENCE_RULE_MIN_TRAIL, FENCE_RULE_PREFIX, GLYPH_APPROVED, GLYPH_CURSOR,
    GLYPH_DIRTY, GLYPH_DISCOVER, GLYPH_DOT, GLYPH_MERGED, GLYPH_PROTECTED, GLYPH_SEARCH, Palette,
    RULE_CHAR,
    SEARCH_HINT_IDLE, TITLE_QUEUE, TITLE_TASKS, TITLE_WORKTREES, glyph_style,
};

// The per-pane chip SETS (and their order) are the single source of truth in
// [`crate::hit::pane_buttons`], shared with the keymap so key gating tracks the
// visible chips. Here we only carry the render-time SCOPE boundary: the first
// `*_ROW_SCOPED` entries of a pane's set are the row-scoped verbs (they act on
// the highlighted row), the rest are pane-scoped (create / collapse). A `·`
// divider is drawn between the two groups. Narrow panes drop chips from the
// RIGHT (collapse first), so the row-scoped verbs survive longest; the divider
// shows only while chips from BOTH groups remain (see [`build_header`]);
// collapse always keeps its `z` key. These MUST stay in step with the ordering
// of the corresponding `pane_buttons` arm.
const QUEUE_ROW_SCOPED: usize = 4; // [r]un [x]stop [g]oto [a]rchive · [c]reate [z]
const TASKS_ROW_SCOPED: usize = 2; // [r]un [d]iscover · [c]reate [z]
const WORKTREE_ROW_SCOPED: usize = 4; // [r]un [g]oto [x]remove [t]asks · [c]reate [z]

/// Scope divider drawn between the row-scoped and pane-scoped chip groups (the
/// TUI's `·` separator convention). It REPLACES the normal single-space gap
/// before the first pane-scoped chip, so it costs [`GROUP_SEP_EXTRA`] extra
/// cells over that gap — folded into every width/fit computation so alignment
/// and hit rects stay exact.
const GROUP_SEP: &str = " · ";
const GROUP_SEP_EXTRA: usize = 2; // GROUP_SEP is 3 cells; the gap it replaces is 1

use crate::view::{Computed, is_bulk_selection, selected_positions, selection_range, window_start};

/// The bold pane title — always white (`fg`); a pane's focus is shown by its
/// border color, not the title. Shared by the expanded chrome and collapsed bar.
fn title_span(title: &str, _focused: bool, p: &Palette) -> Span<'static> {
    Span::styled(
        title.to_string(),
        Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
    )
}

/// A left pane's block: rounded border, focused accent, one column of horizontal
/// padding. The header (title + action-button chips) is supplied by the caller as
/// the border `title` Line via [`build_header`] so drawing and hit geometry share
/// one source of truth.
fn pane_block(title_line: Line<'static>, focused: bool, p: &Palette) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(p.border_style(focused))
        .padding(Padding::horizontal(1))
        .title(title_line)
}

/// The chip spans for one title-bar button plus its display width in CELLS.
/// Labeled form is `[{key}]{label}` (`[k]` accent+bold, then the lowercase
/// label word in fg, no gap — fused to `[c]reate` when the key is the label's
/// first letter, `[z]collapse` when it isn't); the compact form drops the
/// label, leaving `[{key}]`. Hotkeys always render in square brackets — the
/// global convention across chips, hint rows, footer, and help. Width is
/// measured with `Span::width()` (ratatui's unicode-width) rather than
/// assumed, so a renamed label can't silently desync the border-fill /
/// right-alignment / hit-rect math. The collapse chip flips its label on
/// `collapsed`; the run chip flips its label to `rerun` on QUEUE (`pane`),
/// where `r` re-queues the selected task rather than running a definition.
/// `bulk` (the pane has an active multi-row selection) dims the WHOLE chip
/// (key + label, both forms) when [`crate::hit::bulk_allowed`] says `b` can't
/// act on a range — the same de-emphasis `Palette::dim_style` uses for
/// archived rows, not a disabled *color*, just less contrast. The key/click
/// handlers refuse a dimmed chip's action with a status line instead of
/// silently acting on the cursor row alone (see `App::apply_action`).
fn button_chip(
    b: PaneButton,
    pane: PaneId,
    collapsed: bool,
    labeled: bool,
    bulk: bool,
    // The QUEUE `[a]rchive` chip flips to `[a]unarchive` when the first selected
    // row is already archived (the direction `a` will take — see
    // [`render_list_pane`]). Ignored by every non-Archive chip.
    archive_unarchive: bool,
    p: &Palette,
) -> (Vec<Span<'static>>, u16) {
    let (key, label) = match b {
        PaneButton::Create => ('c', BTN_LABEL_CREATE),
        PaneButton::Tasks => ('t', BTN_LABEL_TASKS),
        PaneButton::Run => ('r', if pane == PaneId::Queue { BTN_LABEL_RERUN } else { BTN_LABEL_RUN }),
        PaneButton::Discover => ('d', BTN_LABEL_DISCOVER),
        PaneButton::Goto => ('g', BTN_LABEL_GOTO),
        PaneButton::Cancel => ('x', BTN_LABEL_STOP),
        PaneButton::Archive => {
            ('a', if archive_unarchive { BTN_LABEL_UNARCHIVE } else { BTN_LABEL_ARCHIVE })
        }
        PaneButton::Remove => ('x', BTN_LABEL_REMOVE),
        PaneButton::Collapse => {
            ('z', if collapsed { BTN_LABEL_EXPAND } else { BTN_LABEL_COLLAPSE })
        }
    };
    let disabled = bulk && !crate::hit::bulk_allowed(pane, b);
    let key_style = if disabled {
        p.dim_style()
    } else {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    };
    let label_style = if disabled { p.dim_style() } else { Style::default().fg(p.fg) };
    let mut spans = vec![Span::styled(format!("[{key}]"), key_style)];
    if labeled {
        // `[c]reate` / `[g]oto` when the key is the label's first letter
        // (the footer's `[q]uit` pattern); otherwise `[z]collapse`.
        match label.strip_prefix(key) {
            Some(rest) => spans.push(Span::styled(rest.to_string(), label_style)),
            None => spans.push(Span::styled(label.to_string(), label_style)),
        }
    }
    let w: usize = spans.iter().map(|s| s.width()).sum();
    (spans, w as u16)
}

/// Chip-strip fit selection (pure, so the two-stage degradation is unit-testable).
/// Stage 1: if the fully-labeled strip fits alongside the title, use it whole.
/// Stage 2 (labels don't all fit): fall back to the compact strip and drop whole
/// chips from the RIGHT (last chip — the collapse toggle, which keeps its `z`
/// key — goes first) until the strip fits. Each chip costs a leading separator
/// space plus its cell width; when the kept run spans BOTH scope groups (i.e.
/// keeps more than `group1` chips) the wider `·` scope divider replaces one of
/// those gaps, adding `sep_extra` cells. Returns `(labeled, kept)`.
fn fit_chip_strip(
    title_w: usize,
    avail: usize,
    labeled_widths: &[usize],
    compact_widths: &[usize],
    group1: usize,
    sep_extra: usize,
) -> (bool, usize) {
    let strip = |widths: &[usize], kept: usize| -> usize {
        let base: usize = widths[..kept].iter().map(|w| 1 + w).sum();
        base + if kept > group1 { sep_extra } else { 0 }
    };
    if title_w + strip(labeled_widths, labeled_widths.len()) <= avail {
        return (true, labeled_widths.len());
    }
    let mut kept = compact_widths.len();
    while kept > 0 && title_w + strip(compact_widths, kept) > avail {
        kept -= 1;
    }
    (false, kept)
}

/// Build a pane's top-border header: the (already decorated) title at the left,
/// an optional meta-colored summary right after it (`· N queued · N running`),
/// then its action-button chips pushed to the RIGHT END of the top border (just
/// before the corner), with a run of border characters filling the gap between.
/// The chips are two scope groups (`buttons[..row_scoped]` = row-scoped verbs,
/// the rest pane-scoped); a `·` divider sits between them, drawn only while chips
/// from BOTH groups survive. The summary is the FIRST thing dropped for width: it
/// renders only when the full chip strip (labeled or compact) survives alongside
/// it — a pane narrow enough to shed chips never shows it. After that, chips drop
/// from the right until the title + strip fits between the corners (each chip
/// costs a leading space + its cell width, the group boundary a wider `·`); when
/// even zero chips fit the title is left to be clipped by `Block::title`. Returns
/// the border `title` Line to draw and the absolute hit rects for the surviving
/// chips at their right-aligned coordinates. All widths are cell widths
/// (`Span::width`).
#[allow(clippy::too_many_arguments)]
fn build_header(
    area: Rect,
    title: &str,
    summary: Option<&str>,
    focused: bool,
    pane: PaneId,
    buttons: &[PaneButton],
    row_scoped: usize,
    collapsed: bool,
    bulk: bool,
    // Flips the QUEUE `[a]rchive` chip to `[a]unarchive` (see `button_chip`).
    archive_unarchive: bool,
    p: &Palette,
) -> (Line<'static>, Vec<(Rect, PaneButton)>) {
    let x0 = area.x + 1;
    let avail = area.width.saturating_sub(2) as usize; // cells between the corners
    let title_span = title_span(title, focused, p);
    let summary_span = summary
        .map(|s| Span::styled(format!(" · {s}"), Style::default().fg(p.meta)));
    let summary_w = summary_span.as_ref().map_or(0, Span::width);
    let title_w = title_span.width();

    // Two-stage degradation: build both the labeled and the compact chip sets,
    // then let `fit_chip_strip` decide which form fits and how many survive.
    // Labels are all-or-nothing (dropped together to stay visually consistent);
    // chip-dropping happens only in the compact form.
    let labeled: Vec<(Vec<Span<'static>>, usize)> = buttons
        .iter()
        .map(|&b| {
            let (spans, w) = button_chip(b, pane, collapsed, true, bulk, archive_unarchive, p);
            (spans, w as usize)
        })
        .collect();
    let compact: Vec<(Vec<Span<'static>>, usize)> = buttons
        .iter()
        .map(|&b| {
            let (spans, w) = button_chip(b, pane, collapsed, false, bulk, archive_unarchive, p);
            (spans, w as usize)
        })
        .collect();
    let labeled_w: Vec<usize> = labeled.iter().map(|(_, w)| *w).collect();
    let compact_w: Vec<usize> = compact.iter().map(|(_, w)| *w).collect();
    // Fit with the summary counted as part of the title; it survives only while
    // the FULL chip strip does too — the moment chips would drop, the summary is
    // shed first and the fit re-runs on the bare title.
    let (use_labeled, kept) =
        fit_chip_strip(title_w + summary_w, avail, &labeled_w, &compact_w, row_scoped, GROUP_SEP_EXTRA);
    let (show_summary, use_labeled, kept) = if kept == buttons.len() && summary_w > 0 {
        (true, use_labeled, kept)
    } else {
        let (ul, k) = fit_chip_strip(title_w, avail, &labeled_w, &compact_w, row_scoped, GROUP_SEP_EXTRA);
        (false, ul, k)
    };
    let head_w = title_w + if show_summary { summary_w } else { 0 };
    let chips = if use_labeled { labeled } else { compact };
    // The scope divider shows only when the kept run crosses the group boundary
    // (chips from both groups survive); it then replaces one inter-chip gap.
    let show_separator = kept > row_scoped;
    let strip_cost = |kept: usize| -> usize {
        let base: usize = chips[..kept].iter().map(|(_, w)| 1 + w).sum();
        base + if kept > row_scoped { GROUP_SEP_EXTRA } else { 0 }
    };

    let mut spans = vec![title_span];
    if let Some(s) = summary_span.filter(|_| show_summary) {
        spans.push(s);
    }

    // No chips fit: leave the border to draw itself and let Block clip the title.
    if kept == 0 {
        return (Line::from(spans), Vec::new());
    }

    let strip_w = strip_cost(kept);
    // Border-character run between the head (title + summary) and the
    // right-aligned chip strip.
    let filler_w = avail.saturating_sub(head_w + strip_w);
    if filler_w > 0 {
        spans.push(Span::styled("─".repeat(filler_w), p.border_style(focused)));
    }
    // Chips begin at the first cell after head + filler; the strip ends flush
    // against the corner at x0 + avail.
    let mut x = x0.saturating_add((head_w + filler_w) as u16);
    let mut rects = Vec::new();
    for (i, (&b, (chip, w))) in buttons.iter().zip(chips).take(kept).enumerate() {
        if i == row_scoped && show_separator {
            // Scope boundary: the `·` divider (dim, chrome punctuation) replaces
            // this chip's normal 1-cell leading gap.
            spans.push(Span::styled(GROUP_SEP.to_string(), p.dim_style()));
            x = x.saturating_add(GROUP_SEP.chars().count() as u16);
        } else {
            spans.push(Span::raw(" ")); // separator
            x = x.saturating_add(1);
        }
        rects.push((Rect { x, y: area.y, width: w as u16, height: 1 }, b));
        spans.extend(chip);
        x = x.saturating_add(w as u16);
    }
    (Line::from(spans), rects)
}

/// An expanded list pane's vertical layout plan: a blank spacer row above the
/// search-hint row (gap under the title border), the hint row itself (always
/// present, one row), an optional row below the hint hosting the COLUMN
/// HEADERS, then the data rows. Both extra rows are inert — no hit target, not
/// selectable — they only shift the data rows (and every geometry derived from
/// `rows_area`) down. `inner_height` is the pane's inner (inside-border) row
/// count. Degradation prioritizes the header row (it carries information; the
/// top gap is cosmetic):
///
/// - `inner_height ≥ 6`: both rows (hint + gap + headers + ≥3 data rows).
/// - `inner_height == 5`: header row only (hint + headers + 3 data rows).
/// - `inner_height < 5`: neither (density preserved).
///
/// Returns `(top_spacer, bottom_spacer, data_capacity)`.
fn pane_vplan(inner_height: u16) -> (bool, bool, u16) {
    let top_spacer = inner_height >= 6;
    let bottom_spacer = inner_height >= 5;
    let data_capacity = inner_height
        .saturating_sub(1) // hint row
        .saturating_sub(top_spacer as u16)
        .saturating_sub(bottom_spacer as u16);
    (top_spacer, bottom_spacer, data_capacity)
}

/// Render one pane's chrome (border with the header + horizontal padding).
/// Returns the padded inner content `Rect` — everything downstream (rows, hit
/// rects, scrollbar, throbbers) uses it so the geometry stays aligned.
fn pane_chrome(
    frame: &mut ratatui::Frame,
    area: Rect,
    header: Line<'static>,
    focused: bool,
    p: &Palette,
) -> Rect {
    let block = pane_block(header, focused, p);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

/// Render a collapsed pane: only the 2-row title bar (top border with the header +
/// bottom border) pinned to the top of `area`. No content, no scrollbar, no
/// PaneBody/Row hit rects. Appends the header's button hit rects to `btn_hits`
/// (registered LAST by the caller so buttons win their border-row sub-rects).
#[allow(clippy::too_many_arguments)]
fn render_collapsed_pane(
    frame: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    summary: &str,
    focused: bool,
    pane: PaneId,
    buttons: &[PaneButton],
    btn_hits: &mut Vec<(Rect, PaneId, PaneButton)>,
    p: &Palette,
    dimmed: bool,
    bulk: bool,
) {
    // A collapsed bar shows ONLY the expand chip: it is the bar's primary
    // affordance and must never be the one dropped for width (create/goto
    // return with the pane).
    let _ = buttons;
    // A collapsed bar shows only the Collapse chip, so the archive direction is
    // irrelevant here.
    let (mut header, rects) = build_header(
        area,
        title,
        Some(summary),
        focused,
        pane,
        &[PaneButton::Collapse],
        1,
        true,
        bulk,
        false,
        p,
    );
    if dimmed {
        patch_line(&mut header, p.dim_style());
    }
    let h = area.height.min(COLLAPSED_H);
    let bar = Rect { x: area.x, y: area.y, width: area.width, height: h };
    frame.render_widget(pane_block(header, focused, p), bar);
    for (r, b) in rects {
        btn_hits.push((r, pane, b));
    }
}

/// The inline search-hint/input row (row 0 of every expanded pane). Idle:
/// `🔍 [/]filter` with the hotkey accent+bold (hotkeys always in square
/// brackets, never grey — the visible-hotkey convention). Searching that pane:
/// `🔍 /{query}█` with a colored query + block cursor. Filter set but not
/// searching: `🔍 /{query}` colored, no cursor.
fn search_hint_line(query: &str, searching: bool, p: &Palette) -> Line<'static> {
    // 🔍 is double-width but it is the first column, so nothing after it shifts.
    let mut spans = vec![Span::raw(format!("{GLYPH_SEARCH} "))];
    if searching {
        spans.push(Span::styled(format!("/{query}"), Style::default().fg(p.meta)));
        spans.push(Span::styled(GLYPH_CURSOR.to_string(), Style::default().fg(p.meta)));
    } else if query.is_empty() {
        spans.push(Span::styled(
            "[/]".to_string(),
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(SEARCH_HINT_IDLE.to_string(), Style::default().fg(p.fg)));
    } else {
        spans.push(Span::styled(format!("/{query}"), Style::default().fg(p.meta)));
    }
    Line::from(spans)
}

fn queue_line(
    row: &QueueRow,
    layout: &QueueColLayout,
    p: &Palette,
    now_epoch_s: u64,
    tz_offset_s: i32,
) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    // Glyph column: running rows get a placeholder space (throbber painted over).
    if row.running {
        spans.push(Span::raw(" "));
    } else {
        spans.push(Span::styled(row.glyph.to_string(), glyph_style(row.glyph, p)));
    }
    let gap = " ".repeat(crate::selectors::COL_GAP);
    if layout.worktree_w > 0 {
        spans.push(Span::raw(gap.clone()));
        spans.push(Span::styled(pad_clip(&row.worktree, layout.worktree_w), Style::default().fg(p.worktree)));
    }
    if layout.def_w > 0 {
        spans.push(Span::raw(gap.clone()));
        let def = row.def_name.as_deref().unwrap_or("");
        spans.push(Span::styled(pad_clip(def, layout.def_w), Style::default().fg(p.mauve)));
    }
    spans.push(Span::raw(gap.clone()));
    // Prompt/summary is prose → the terminal's default grey. White (`fg`) is
    // reserved for actions/tabs, so this stays intentionally unstyled `Span::raw`.
    spans.push(Span::raw(pad_clip(&row.summary, layout.summary_w)));
    // Timestamps read in teal — a real color, not grey (grey-on-dark was
    // unreadable per user feedback).
    if layout.show_timestamp {
        spans.push(Span::raw(gap.clone()));
        spans.push(Span::styled(
            absolute_local_label(row.created_epoch_s, tz_offset_s),
            Style::default().fg(p.info),
        ));
    }
    if layout.age_w > 0 {
        spans.push(Span::raw(gap.clone()));
        spans.push(Span::styled(
            pad_clip(&relative_age_label(row.created_epoch_s, now_epoch_s), layout.age_w),
            Style::default().fg(p.info),
        ));
    }
    // Trailing live slot (fixed QUEUE_LIVE_W): `⏱ <elapsed>` for running rows or
    // `#N in lane` for queued rows — both are "live/now" → warn. Archived/blank
    // rows render raw padding so the reserved cell stays aligned.
    if layout.live_w > 0 {
        spans.push(Span::raw(gap));
        if row.running || !row.detail.is_empty() {
            spans.push(Span::styled(pad_clip(&row.detail, layout.live_w), Style::default().fg(p.warn)));
        } else {
            spans.push(Span::raw(pad_clip(&row.detail, layout.live_w)));
        }
    }
    Line::from(spans)
}

/// One padded header label over a column: the leading gutter + the label
/// clipped/padded to the column width, in the pane's de-emphasis dim (headers
/// are chrome, not data).
fn header_col(spans: &mut Vec<Span<'static>>, label: &str, w: usize, p: &Palette) {
    spans.push(Span::raw(" ".repeat(crate::selectors::COL_GAP)));
    spans.push(Span::styled(pad_clip(label, w), p.dim_style()));
}

/// QUEUE column-header row, mirroring `queue_line`'s span structure cell for
/// cell so every label sits over its column.
fn queue_header(layout: &QueueColLayout, p: &Palette) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")]; // status-glyph slot
    if layout.worktree_w > 0 {
        header_col(&mut spans, "Worktree", layout.worktree_w, p);
    }
    if layout.def_w > 0 {
        header_col(&mut spans, "Task", layout.def_w, p);
    }
    header_col(&mut spans, "Prompt", layout.summary_w, p);
    if layout.show_timestamp {
        header_col(&mut spans, "Created", crate::selectors::TIMESTAMP_W, p);
    }
    if layout.age_w > 0 {
        header_col(&mut spans, "Age", layout.age_w, p);
    }
    if layout.live_w > 0 {
        header_col(&mut spans, "Live", layout.live_w, p);
    }
    Line::from(spans)
}

/// TASKS column-header row (`Name | Model | Description | Cron`), mirroring
/// `def_line`'s span structure (the front `⌕` discovery-marker slot stays blank).
fn def_header(layout: &DefColLayout, p: &Palette) -> Line<'static> {
    let mut spans = Vec::new();
    if layout.marker_w > 0 {
        spans.push(Span::raw("  ")); // ⌕ slot + separator
    }
    spans.push(Span::styled(pad_clip("Name", layout.name_w), p.dim_style()));
    if layout.model_w > 0 {
        header_col(&mut spans, "Model", layout.model_w, p);
    }
    if layout.desc_w > 0 {
        header_col(&mut spans, "Description", layout.desc_w, p);
    }
    if layout.sched_w > 0 {
        header_col(&mut spans, "Cron", layout.sched_w, p);
    }
    Line::from(spans)
}

/// WORKTREES column-header row, mirroring `worktree_line`'s span structure
/// (the lead indicator and `±` marker slot stay blank).
fn wt_header(layout: &WtColLayout, p: &Palette) -> Line<'static> {
    let mut spans = vec![Span::raw("  ")]; // lead indicator + separator
    if layout.dirty_w > 0 {
        spans.push(Span::raw("  ")); // ± slot + separator
    }
    if layout.protected_w > 0 {
        spans.push(Span::raw("  ")); // ⛨ slot + separator
    }
    if layout.merged_w > 0 {
        spans.push(Span::raw("  ")); // ↣ slot + separator
    }
    spans.push(Span::styled(pad_clip("Name", layout.name_w), p.dim_style()));
    if layout.last_w > 0 {
        header_col(&mut spans, "Last Task", layout.last_w, p);
    }
    if layout.pr_w > 0 {
        header_col(&mut spans, "PR", layout.pr_w, p);
    }
    if layout.author_w > 0 {
        header_col(&mut spans, "Author", layout.author_w, p);
    }
    if layout.commit_age_w > 0 {
        header_col(&mut spans, "Commit", layout.commit_age_w, p);
    }
    if layout.queued_w > 0 {
        header_col(&mut spans, "Next", layout.queued_w, p);
    }
    if layout.elapsed_w > 0 {
        header_col(&mut spans, "Live", layout.elapsed_w, p);
    }
    Line::from(spans)
}

fn worktree_line(
    row: &WorktreeRow,
    layout: &WtColLayout,
    p: &Palette,
    now_epoch_s: u64,
) -> Line<'static> {
    // Leading indicator: outstanding work on the lane (the running task + queued
    // tasks). 0 → the green idle dot; N > 0 → the count as a single yellow digit
    // ('9' caps the cell at ≥9). Failed lanes no longer color this slot red —
    // the ✗ glyph in the last-task column already carries failure (user request:
    // green/yellow only, count over state).
    let outstanding = row.queued + usize::from(row.running_elapsed.is_some());
    let lead = if outstanding == 0 {
        Span::styled(GLYPH_DOT.to_string(), Style::default().fg(p.ok))
    } else {
        let digit = char::from_digit(outstanding.min(9) as u32, 10).expect("min(9) is a digit");
        Span::styled(digit.to_string(), Style::default().fg(p.warn))
    };
    let gap = " ".repeat(crate::selectors::COL_GAP);
    // Informational columns read in real palette colors (never grey — grey-on-dark
    // was unreadable per user feedback); the de-emphasis dim is only for archived/
    // empty rows via `patch_line`. A column present in the layout but empty on this
    // row renders as blank padding so the fields stay aligned down the pane.
    // `info` (teal) is timestamps only; other metadata reads in `meta`.
    let info = Style::default().fg(p.info);
    let meta = Style::default().fg(p.meta);
    let warn = Style::default().fg(p.warn);
    let mauve = Style::default().fg(p.mauve);
    let fg = Style::default().fg(p.fg);
    // Anchor: the lead indicator + space, then the front markers — the `±`
    // uncommitted-changes marker and the `⛨` protected marker, each a
    // single-cell slot (statically reserved) so both can show at once — then
    // the accent-colored worktree identity name.
    let mut spans = vec![lead, Span::raw(" ")];
    if layout.dirty_w > 0 {
        if row.dirty == Some(true) {
            spans.push(Span::styled(GLYPH_DIRTY.to_string(), warn));
        } else {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::raw(" "));
    }
    if layout.protected_w > 0 {
        if row.protected {
            spans.push(Span::styled(GLYPH_PROTECTED.to_string(), meta));
        } else {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::raw(" "));
    }
    // Shared front marker slot: `↣` merged-back (ok green — "committed work is in
    // the default branch, safe to clean up") OR, when not merged, `✓` approved (ok
    // green — the PR passed review). Merged wins; blank for neither/unknown/old
    // daemons. The precedence lives in the pure `wt_merge_marker` selector.
    if layout.merged_w > 0 {
        match wt_merge_marker(row) {
            Some(WtMergeMarker::Merged) => {
                spans.push(Span::styled(GLYPH_MERGED.to_string(), Style::default().fg(p.ok)));
            }
            Some(WtMergeMarker::Approved) => {
                spans.push(Span::styled(GLYPH_APPROVED.to_string(), Style::default().fg(p.ok)));
            }
            None => spans.push(Span::raw(" ")),
        }
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(pad_clip(&row.name, layout.name_w), Style::default().fg(p.worktree)));
    // Last finished lane task: status glyph (status-colored) + name (mauve when a
    // def, default grey when a prompt) + relative age (info), padded to width.
    if layout.last_w > 0 {
        spans.push(Span::raw(gap.clone()));
        match &row.last {
            Some((glyph, name, epoch, is_def)) => {
                let age = relative_age_label(*epoch, now_epoch_s);
                let name_budget = layout.last_w.saturating_sub(3 + age.chars().count());
                let shown = crate::selectors::clip(name, name_budget);
                spans.push(Span::styled(glyph.to_string(), glyph_style(*glyph, p)));
                spans.push(Span::raw(" "));
                if *is_def {
                    spans.push(Span::styled(shown.clone(), mauve));
                } else {
                    spans.push(Span::raw(shown.clone())); // prompt = default grey
                }
                // Right-pin the age at the far edge of the column so ages line
                // up vertically regardless of task-name length.
                let used = 2 + shown.chars().count() + 1 + age.chars().count();
                let pad = 1 + layout.last_w.saturating_sub(used);
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(age.clone(), info));
            }
            None => spans.push(Span::raw(pad_clip("", layout.last_w))),
        }
    }
    // Open PR `#<n>` (meta) — the fixed PR column, immediately left of the
    // author. Only the cell layout lives here; the clickable hit rect is
    // registered in `render_rows` (which knows the row's on-screen x/y) using
    // `WtColLayout::pr_col_x`, so the two can't drift. Blank-padded when the row
    // has no open PR so the trailing columns stay aligned down the pane. A row
    // whose PR also carries its url — i.e. the cell is actually clickable —
    // underlines the `#<n>` glyphs (never the pad, so the underline hugs the
    // text) as the link affordance, matching the detail info tab's pr value.
    if layout.pr_w > 0 {
        spans.push(Span::raw(gap.clone()));
        match row.pr_number {
            Some(n) => {
                let text = crate::selectors::clip(&format!("#{n}"), layout.pr_w);
                let pad = layout.pr_w.saturating_sub(text.chars().count());
                let style =
                    if row.pr_url.is_some() { meta.add_modifier(Modifier::UNDERLINED) } else { meta };
                spans.push(Span::styled(text, style));
                if pad > 0 {
                    spans.push(Span::raw(" ".repeat(pad)));
                }
            }
            None => spans.push(Span::raw(pad_clip("", layout.pr_w))),
        }
    }
    // Last-commit author name (plain fg — a full column of teal read as noise);
    // pairs with the commit-age to read `koshea  3d ago` = who · when. Clipped
    // with `…` past AUTHOR_W.
    if layout.author_w > 0 {
        spans.push(Span::raw(gap.clone()));
        match wt_author_text(row) {
            Some(t) => spans.push(Span::raw(pad_clip(&t, layout.author_w))), // author = default grey
            None => spans.push(Span::raw(pad_clip("", layout.author_w))),
        }
    }
    // Last-commit relative age (info).
    if layout.commit_age_w > 0 {
        spans.push(Span::raw(gap.clone()));
        match row.last_commit_epoch {
            Some(e) => spans
                .push(Span::styled(pad_clip(&relative_age_label(e, now_epoch_s), layout.commit_age_w), info)),
            None => spans.push(Span::raw(pad_clip("", layout.commit_age_w))),
        }
    }
    // `next: <name>` FILL column: the `next:` lead in meta, the def/prompt name
    // mauve (def) or fg (prompt). The queued COUNT is deliberately absent — the
    // leading indicator digit already carries it (user request). Blank-padded
    // when the row has no named queued task — the fill still reserves the slack
    // so the trailing live timer stays right-pinned.
    if layout.queued_w > 0 {
        spans.push(Span::raw(gap.clone()));
        match (row.queued, &row.next_name) {
            (0, _) | (_, None) => spans.push(Span::raw(pad_clip("", layout.queued_w))),
            (_, Some(name)) => {
                let lead = "next: ";
                let name_budget = layout.queued_w.saturating_sub(lead.chars().count());
                let shown = crate::selectors::clip(name, name_budget);
                spans.push(Span::styled(lead, meta));
                spans.push(Span::styled(shown.clone(), if row.next_is_def { mauve } else { fg }));
                let used = lead.chars().count() + shown.chars().count();
                if used < layout.queued_w {
                    spans.push(Span::raw(" ".repeat(layout.queued_w - used)));
                }
            }
        }
    }
    // `⏱` live timer of the task running on this lane (warn) — the trailing
    // live/now slot, right-pinned by the queued fill.
    if layout.elapsed_w > 0 {
        spans.push(Span::raw(gap));
        match row.running_elapsed {
            Some(e) => spans
                .push(Span::styled(pad_clip(&elapsed_label(e, now_epoch_s), layout.elapsed_w), warn)),
            None => spans.push(Span::raw(pad_clip("", layout.elapsed_w))),
        }
    }
    Line::from(spans)
}

fn def_line(
    def: &DefinitionSummary,
    layout: &DefColLayout,
    p: &Palette,
    discovering: bool,
) -> Line<'static> {
    let gap = " ".repeat(crate::selectors::COL_GAP);
    // Front marker slot — the `⌕` discovery marker (single-cell glyph +
    // separator, statically reserved pane-wide) — mirrors the WORKTREES `±`
    // dirty-marker slot. While the def's `d`-discover RPC is in flight the
    // glyph yields to a placeholder space and the generic running-row throbber
    // animates in its cell (queue-row parity), so the user sees the search
    // running before the fan-out lands.
    let mut spans = Vec::new();
    if layout.marker_w > 0 {
        if discovering {
            spans.push(Span::raw(" "));
        } else if def.has_discovery {
            spans.push(Span::styled(GLYPH_DISCOVER.to_string(), Style::default().fg(p.info)));
        } else {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::raw(" "));
    }
    // Task/definition names read in mauve — the single semantic color for a def
    // name across QUEUE, TASKS, and the WORKTREES next/last cells.
    spans.push(Span::styled(pad_clip(&def.name, layout.name_w), Style::default().fg(p.mauve)));
    // Model column: the def's model (`claude-` prefix stripped), right after the
    // name (user request — the model matters more than anything else on the
    // row). Padded to the reserved width so a def without a model leaves it
    // blank and the columns never slide; omitted entirely when no visible def
    // carries a model.
    if layout.model_w > 0 {
        spans.push(Span::raw(gap.clone()));
        spans.push(Span::styled(
            pad_clip(&crate::selectors::def_model_text(def), layout.model_w),
            Style::default().fg(p.meta),
        ));
    }
    // Description FILL column: prose in plain fg (like the queue summary), padded
    // to the reserved remainder so the schedule stays right-pinned; blank when a
    // def has no description. Omitted entirely when the pane reserved no fill.
    if layout.desc_w > 0 {
        spans.push(Span::raw(gap.clone()));
        // Task description = default grey (white is reserved for actions/tabs).
        spans.push(Span::raw(pad_clip(&crate::selectors::def_desc_text(def), layout.desc_w)));
    }
    // Schedule column: humanized cron only (see `def_sched_text` — layout and
    // render share it; the `⌕` discovery marker lives in the front slot above).
    // Teal/info like the args column. Blank for a def with none.
    let sched = crate::selectors::def_sched_text(def);
    if !sched.is_empty() {
        spans.push(Span::raw(gap));
        spans.push(Span::styled(pad_clip(&sched, layout.sched_w), Style::default().fg(p.info)));
    }
    Line::from(spans)
}

/// Register a vertical scrollbar hit region (track + proportional thumb) and draw
/// the built-in Scrollbar. `total` rows, `offset` first-visible, `visible` rows.
#[allow(clippy::too_many_arguments)]
fn render_scrollbar(
    frame: &mut ratatui::Frame,
    area: Rect,
    total: usize,
    offset: usize,
    visible: usize,
    pane: PaneId,
    fg: ratatui::style::Color,
    hits: &mut HitMap,
) {
    if total <= visible || area.height == 0 {
        return;
    }
    let mut state = ScrollbarState::new(total.saturating_sub(visible)).position(offset);
    let track = Rect { x: area.right().saturating_sub(1), y: area.y, width: 1, height: area.height };
    hits.push(track, HitTarget::ScrollbarTrack(pane));
    // Proportional thumb: height ≈ visible/total of the track, top ≈ offset/total.
    let h = area.height as usize;
    let thumb_h = ((visible * h) / total).max(1).min(h) as u16;
    // `checked_div` folds in the `max_off == 0` guard (no thumb travel) for free.
    let max_off = total - visible;
    let thumb_top = match (offset * h.saturating_sub(thumb_h as usize)).checked_div(max_off) {
        Some(travel) => area.y + travel as u16,
        None => area.y,
    };
    hits.push(
        Rect { x: track.x, y: thumb_top, width: 1, height: thumb_h },
        HitTarget::ScrollbarThumb(pane),
    );
    // Explicit fg + bg(Reset) on every scrollbar part. Rows paint across the full
    // pane width (including this column): the bg(Reset) stops a selected row's
    // selection bg bleeding through behind the glyphs, and the explicit fg stops
    // the begin/end arrows and thumb from INHERITING the underlying cell's fg
    // (fg: None keeps the cell's existing color) — otherwise the top arrow lands
    // on the first row's colored Live-timer cell and renders in that yellow.
    let sb_style = Style::default().bg(ratatui::style::Color::Reset).fg(fg);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .track_style(sb_style)
            .thumb_style(sb_style)
            .begin_style(sb_style)
            .end_style(sb_style),
        area,
        &mut state,
    );
}

/// Patch every span of a line to `style` (per-span fg colors survive line-level
/// styles, so uniform restyling — selection, archived, spotlight-dim — must
/// patch span by span).
fn patch_line(line: &mut Line<'static>, style: Style) {
    for span in line.spans.iter_mut() {
        span.style = span.style.patch(style);
    }
    *line = std::mem::take(line).style(style); // trailing padding cells
}

/// An inert horizontal-rule display line labelling a section break (e.g. the
/// QUEUE ACTIVE/FINISHED split), styled like the transcript code-fence rules:
/// `──────── finished ───…` — rule chars in the border color (muted grey when
/// the pane is spotlight-dimmed), the label in the de-emphasis grey. Never a
/// selectable row: it registers no hit target and no cursor position.
fn section_divider_line(width: u16, label: &str, p: &Palette, dimmed: bool) -> Line<'static> {
    let width = width as usize;
    let rule_style = if dimmed { p.dim_style() } else { Style::default().fg(p.border) };
    let label_w = label.chars().count() + 2; // a space either side of the label
    let trailing = width
        .saturating_sub(FENCE_RULE_PREFIX + label_w)
        .max(FENCE_RULE_MIN_TRAIL);
    Line::from(vec![
        Span::styled(RULE_CHAR.to_string().repeat(FENCE_RULE_PREFIX), rule_style),
        Span::styled(format!(" {label} "), p.dim_style()),
        Span::styled(RULE_CHAR.to_string().repeat(trailing), rule_style),
    ])
}

/// Label on the QUEUE ACTIVE/FINISHED section divider.
const DIVIDER_LABEL_FINISHED: &str = "finished";

/// Display line for [`render_list_pane`]: either a real data row (index into the
/// `rows` slice) or the inert section divider drawn between two sections.
enum DisplayRow {
    Row(usize),
    Divider,
}

/// Shared renderer for all three list panes: chrome + `PaneBody` hit, empty
/// state, cursor-centered windowing, per-row selection/dim styling + `Row` hit,
/// throbbers over running rows, and the scrollbar. Only the line-builder, the
/// empty message, and the per-row `dim`/`running` predicates differ between
/// panes — keeping the loop here means queue/tasks/worktrees can never drift.
/// `dimmed` is the spotlight: while another pane is being search-typed, this
/// whole pane (header, hint, rows) renders uniformly muted.
#[allow(clippy::too_many_arguments)]
fn render_list_pane<T, C>(
    frame: &mut ratatui::Frame,
    area: Rect,
    hits: &mut HitMap,
    p: &Palette,
    title_base: &str,
    summary: &str,
    search: &str,
    searching: bool,
    focused: bool,
    pane: PaneId,
    list_pane: ListPane,
    sel: &Selection,
    // The pane's MARKED row identities (`Space`). Combined with `sel` into the
    // effective selection by `selected_positions` — see its docs for the rule.
    marks: &HashSet<String>,
    rows: &[T],
    empty_msg: &str,
    now_epoch_s: u64,
    buttons: &[PaneButton],
    // Boundary in `buttons` between the row-scoped and pane-scoped groups (see
    // the `*_ROW_SCOPED` consts); threaded to `build_header` for the `·` divider.
    row_scoped: usize,
    btn_hits: &mut Vec<(Rect, PaneId, PaneButton)>,
    // Per-frame column widths derived from the VISIBLE rows + available width, so
    // fields line up across what is actually on screen (recomputed each frame).
    ctx_of: impl Fn(&[T], usize) -> C,
    line_of: impl Fn(&T, &C, &Palette) -> Line<'static>,
    // Column-header row for the resolved layout, drawn into the bottom-spacer
    // slot under the search hint (skipped on short panes that dropped the
    // spacer, and on empty panes that have no layout).
    header_of: impl Fn(&C, &Palette) -> Line<'static>,
    dim_of: impl Fn(&T) -> bool,
    running_of: impl Fn(&T) -> bool,
    // The row's STABLE identity — the mark key. Must match what
    // `App::row_identity` produces for this pane, or a marked row won't
    // highlight (Queue `task_id`, Tasks `{repo}/{name}`, Worktrees `raw_name`).
    id_of: impl Fn(&T) -> String,
    dimmed: bool,
    // Real-row index after which to splice an inert section divider (the QUEUE
    // ACTIVE/FINISHED break). `None` → no divider (tasks/worktrees always pass it).
    divider_after: Option<usize>,
    // Per-row POST-RENDER buffer decoration, invoked with the row, its on-screen
    // rect, and the resolved column ctx so a pane can rewrite the freshly-drawn
    // glyph cells the generic loop can't know about (the WORKTREES PR-cell OSC 8
    // hyperlink). Called AFTER the paragraph paints. Queue/tasks pass a no-op.
    decorate_row: impl Fn(&T, Rect, &C, &mut ratatui::buffer::Buffer),
) {
    let sel_positions: HashSet<usize> =
        selected_positions(rows, sel, marks, &id_of).into_iter().collect();
    let bulk = is_bulk_selection(sel, marks);
    let title = pane_title(title_base, sel_positions.len(), bulk);
    // The QUEUE `[a]rchive` chip mirrors the verb the `a` key will run on the
    // FIRST (topmost) selected row: `unarchive` when that row is dimmed
    // (archived), `archive` otherwise. `dim_of` is `row.archived` on QUEUE (the
    // only pane with the chip); other panes never read this. The topmost
    // selected position is the min of `sel_positions` — the same anchor
    // `archive_selected` uses (its rows come back in the same ascending order).
    let archive_unarchive =
        sel_positions.iter().min().map(|&i| dim_of(&rows[i])).unwrap_or(false);
    let (mut header, rects) = build_header(
        area,
        &title,
        Some(summary),
        focused,
        pane,
        buttons,
        row_scoped,
        false,
        bulk,
        archive_unarchive,
        p,
    );
    if dimmed {
        patch_line(&mut header, p.dim_style());
    }
    let inner = pane_chrome(frame, area, header, focused, p);
    for (r, b) in rects {
        btn_hits.push((r, pane, b));
    }
    hits.push(inner, HitTarget::PaneBody(pane));

    // Vertical plan: an inert top spacer (gap under the title border), the
    // search-hint/input row, an inert bottom spacer ("line height"), then data
    // rows (see `pane_vplan`). Both spacers register no hit target; every row
    // geometry (hint hit rect, windowing, Row hits, throbbers, scrollbar) is
    // measured off `hint_rect.y` / `rows_area` so it all shifts together.
    let (top_spacer, bottom_spacer, data_cap) = pane_vplan(inner.height);

    // The hint row sits below the top spacer. It is the visual home of the `/`
    // hotkey + live filter input, but registers NO click target (clicks there
    // were only ever accidental focus/search-entry — search is keyboard `/`).
    let hint_rect = Rect {
        x: inner.x,
        y: inner.y.saturating_add(top_spacer as u16),
        width: inner.width,
        height: 1,
    };
    let mut hint = search_hint_line(search, searching, p);
    if dimmed {
        patch_line(&mut hint, p.dim_style());
    }
    frame.render_widget(Paragraph::new(hint), hint_rect);

    let rows_area = Rect {
        x: inner.x,
        y: hint_rect.y.saturating_add(1 + bottom_spacer as u16),
        width: inner.width,
        height: data_cap,
    };

    if rows.is_empty() {
        frame.render_widget(Paragraph::new(empty_msg.to_string()).style(p.dim_style()), rows_area);
        return;
    }

    // Build the display-line plan: every real row, with the inert divider spliced
    // between the two sections when the split is interior (both sections present).
    // Geometry (windowing, Row hits, throbbers, scrollbar) is measured off this
    // display list; selection/cursor stay in REAL-row space, so `HitTarget::Row`
    // indices map 1:1 to real rows and the cursor can never land on the divider.
    let n = rows.len();
    let divider_at = divider_after.filter(|&k| k + 1 < n);
    let mut display: Vec<DisplayRow> = Vec::with_capacity(n + 1);
    for i in 0..n {
        display.push(DisplayRow::Row(i));
        if divider_at == Some(i) {
            display.push(DisplayRow::Divider);
        }
    }
    let total = display.len();

    let cap = rows_area.height as usize;
    // Window in DISPLAY space, centered on the cursor's display line (the divider
    // is never a cursor position, so a Row is always found).
    let cursor = sel.cursor.min(n - 1);
    let cursor_disp = display
        .iter()
        .position(|d| matches!(d, DisplayRow::Row(i) if *i == cursor))
        .unwrap_or(0);
    let offset = window_start(total, cursor_disp, cap);
    let visible = cap.min(total - offset);

    // The visible real rows are contiguous (the divider is the only gap), so size
    // the columns from that slice — `ctx_of` still sees only real rows.
    let real_span = || {
        display[offset..offset + visible]
            .iter()
            .filter_map(|d| if let DisplayRow::Row(i) = d { Some(*i) } else { None })
    };
    let ctx_rows: &[T] = match (real_span().min(), real_span().max()) {
        (Some(lo), Some(hi)) => &rows[lo..=hi],
        _ => &[],
    };
    let ctx = ctx_of(ctx_rows, rows_area.width as usize);

    // Column headers live in the bottom-spacer row (between the hint and the
    // data rows). Inert — no hit target — and already dim, so the spotlight
    // patch is a no-op on top.
    if bottom_spacer {
        let header_rect =
            Rect { x: inner.x, y: hint_rect.y.saturating_add(1), width: inner.width, height: 1 };
        frame.render_widget(Paragraph::new(header_of(&ctx, p)), header_rect);
    }

    let mut lines: Vec<Line> = Vec::with_capacity(visible);
    for vd in 0..visible {
        let y = rows_area.y + vd as u16;
        match &display[offset + vd] {
            DisplayRow::Row(idx) => {
                let idx = *idx;
                let mut line = line_of(&rows[idx], &ctx, p);
                // Two-tone selection. The CURSOR row (and any row inside an
                // anchored Shift+Arrow range) gets the bright bar; a MARKED row
                // that is neither the cursor nor in the range gets the dimmer
                // muted bar. `sel_positions` (the acted-on set) intentionally
                // drops the bare cursor row once marks exist, so highlight the
                // cursor separately here to keep it visible in marks-only mode.
                let (r_start, r_end) = selection_range(sel);
                let in_range = sel.anchor.is_some() && idx >= r_start && idx <= r_end;
                let bright = focused && (idx == cursor || in_range);
                let muted = focused && !bright && marks.contains(&id_of(&rows[idx]));
                if dimmed {
                    // Spotlight: another pane is being search-typed — mute this whole
                    // pane (a dimmed pane is never focused, so no selection to show).
                    patch_line(&mut line, p.dim_style());
                } else if bright || muted {
                    // Pad the line's spans out to the full row width BEFORE patching
                    // so the bar spans the WHOLE row even when the line's own spans
                    // end early — `Line::style` alone tints only covered cells.
                    let w = line.width();
                    if w < rows_area.width as usize {
                        line.spans.push(Span::raw(" ".repeat(rows_area.width as usize - w)));
                    }
                    // Patch every span so per-span fg colors don't survive over the
                    // bar (a colored name on the bar would be unreadable).
                    patch_line(&mut line, if bright { p.selection() } else { p.selection_muted() });
                } else if dim_of(&rows[idx]) {
                    // Same reason: force the archived row uniformly dim rather than
                    // leaving colored glyphs/worktree names.
                    patch_line(&mut line, p.dim_style());
                }
                lines.push(line);
                let row_rect = Rect { x: rows_area.x, y, width: rows_area.width, height: 1 };
                hits.push(row_rect, HitTarget::Row(list_pane, idx));
            }
            DisplayRow::Divider => {
                // Inert: no hit target and no cursor position.
                lines.push(section_divider_line(
                    rows_area.width,
                    DIVIDER_LABEL_FINISHED,
                    p,
                    dimmed,
                ));
            }
        }
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), rows_area);

    // Post-render row decoration (the WORKTREES PR-cell OSC 8 hyperlink). Runs
    // AFTER the paragraph paints so it can rewrite the freshly-drawn glyph
    // cells' symbols; queue/tasks pass a no-op. Scoped so the `buffer_mut`
    // borrow is released before the throbber/scrollbar frame renders below.
    {
        let buf = frame.buffer_mut();
        for vd in 0..visible {
            if let DisplayRow::Row(idx) = &display[offset + vd] {
                let row_rect =
                    Rect { x: rows_area.x, y: rows_area.y + vd as u16, width: rows_area.width, height: 1 };
                decorate_row(&rows[*idx], row_rect, &ctx, buf);
            }
        }
    }

    // Throbbers over running rows (seeded from the wall clock so they animate on
    // the 1 s tick without App holding throbber state). Panes with no running
    // rows paint nothing here.
    let mut tstate = ThrobberState::default();
    for _ in 0..(now_epoch_s % 8) {
        tstate.calc_next();
    }
    for vd in 0..visible {
        if let DisplayRow::Row(idx) = &display[offset + vd]
            && running_of(&rows[*idx])
        {
            let mut st = tstate.clone();
            let spinner = if dimmed { p.dim_style() } else { Style::default().fg(p.warn) };
            frame.render_stateful_widget(
                Throbber::default().throbber_style(spinner),
                Rect { x: rows_area.x, y: rows_area.y + vd as u16, width: 1, height: 1 },
                &mut st,
            );
        }
    }
    render_scrollbar(frame, rows_area, total, offset, visible, pane, p.border, hits);
}

pub fn render(app: &App, c: &Computed, frame: &mut ratatui::Frame, area: Rect, hits: &mut HitMap) {
    let p = &c.palette;
    let collapsed = app.collapsed;
    let layout = pane_layout(area.height, app.queue_h_override, app.tasks_h_override, collapsed);
    let regions = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(layout.queue_h),
            ratatui::layout::Constraint::Length(layout.tasks_h),
            ratatui::layout::Constraint::Length(layout.worktrees_h),
        ])
        .split(area);

    // Real local UTC offset (seconds), computed once for the queue timestamps.
    let tz_offset = chrono::Local::now().offset().local_minus_utc();

    // Spotlight: while one pane is being search-typed, every OTHER pane renders
    // uniformly muted so the active filter is unmistakable.
    let spotlight = c.searching.iter().any(|&s| s);

    // Button hit rects are collected here and registered LAST
    // so a button click wins its sub-rect while the rest of the title row falls
    // through to the collapse toggle.
    let mut btn_hits: Vec<(Rect, PaneId, PaneButton)> = Vec::new();

    // A collapsed pane draws only its title bar; an expanded pane draws the full
    // list. Selection/search are threaded to both so the collapsed title keeps
    // its "· N selected" / "/filter" decorations.
    // Title-bar summaries: at-a-glance counts pinned next to each pane title
    // (queued/running for QUEUE, def count for TASKS, busy/total for
    // WORKTREES). Computed from the pane's rows, so an active filter
    // summarizes what is shown.
    let queue_summary = crate::selectors::queue_pane_summary(&c.queue);
    let tasks_summary = crate::selectors::tasks_pane_summary(&c.defs);
    let wt_summary = crate::selectors::wt_pane_summary(&c.worktrees);

    if collapsed[ListPane::Queue.idx()] {
        let marks = &c.ui.marks[ListPane::Queue.idx()];
        let n = selected_positions(&c.queue, &c.queue_sel, marks, |r| r.task_id.clone()).len();
        let bulk = is_bulk_selection(&c.queue_sel, marks);
        let title = pane_title(TITLE_QUEUE, n, bulk);
        render_collapsed_pane(
            frame,
            regions[0],
            &title,
            &queue_summary,
            matches!(c.ui.focus, PaneId::Queue),
            PaneId::Queue,
            pane_buttons(PaneId::Queue),
            &mut btn_hits,
            p,
            spotlight && !c.searching[0],
            bulk,
        );
    } else {
        render_list_pane(
            frame,
            regions[0],
            hits,
            p,
            TITLE_QUEUE,
            &queue_summary,
            &c.ui.search[0],
            c.searching[0],
            matches!(c.ui.focus, PaneId::Queue),
            PaneId::Queue,
            ListPane::Queue,
            &c.queue_sel,
            &c.ui.marks[ListPane::Queue.idx()],
            &c.queue,
            "queue empty — [c]reate a task",
            app.now_epoch_s,
            pane_buttons(PaneId::Queue),
            QUEUE_ROW_SCOPED,
            &mut btn_hits,
            |rows, avail| queue_col_layout(rows, avail, app.now_epoch_s),
            |row, layout, p| queue_line(row, layout, p, app.now_epoch_s, tz_offset),
            queue_header,
            |row| row.archived,
            |row| row.running,
            |row| row.task_id.clone(),
            spotlight && !c.searching[0],
            // ACTIVE/FINISHED section divider (drawn only when both sections exist).
            queue_divider_after(&c.queue),
            |_, _, _, _| {}, // no per-row extra hits on QUEUE
        );
    }
    if collapsed[ListPane::Tasks.idx()] {
        let marks = &c.ui.marks[ListPane::Tasks.idx()];
        let n =
            selected_positions(&c.defs, &c.tasks_sel, marks, |d| format!("{}/{}", d.repo, d.name))
                .len();
        let bulk = is_bulk_selection(&c.tasks_sel, marks);
        let title = pane_title(TITLE_TASKS, n, bulk);
        render_collapsed_pane(
            frame,
            regions[1],
            &title,
            &tasks_summary,
            matches!(c.ui.focus, PaneId::Tasks),
            PaneId::Tasks,
            pane_buttons(PaneId::Tasks),
            &mut btn_hits,
            p,
            spotlight && !c.searching[1],
            bulk,
        );
    } else {
        render_list_pane(
            frame,
            regions[1],
            hits,
            p,
            TITLE_TASKS,
            &tasks_summary,
            &c.ui.search[1],
            c.searching[1],
            matches!(c.ui.focus, PaneId::Tasks),
            PaneId::Tasks,
            ListPane::Tasks,
            &c.tasks_sel,
            &c.ui.marks[ListPane::Tasks.idx()],
            &c.defs,
            "no task definitions",
            app.now_epoch_s,
            pane_buttons(PaneId::Tasks),
            TASKS_ROW_SCOPED,
            &mut btn_hits,
            def_col_layout,
            |d, layout, p| {
                def_line(d, layout, p, app.discovering.contains(&format!("{}/{}", d.repo, d.name)))
            },
            def_header,
            |_| false,
            // "Running" here = the def's `d`-discover RPC is in flight — the
            // generic throbber paints over the row's front `⌕`-marker cell.
            |d| app.discovering.contains(&format!("{}/{}", d.repo, d.name)),
            |d| format!("{}/{}", d.repo, d.name),
            spotlight && !c.searching[1],
            None, // tasks pane has no section divider
            |_, _, _, _| {}, // no per-row extra hits on TASKS
        );
    }
    if collapsed[ListPane::Worktrees.idx()] {
        let marks = &c.ui.marks[ListPane::Worktrees.idx()];
        let n = selected_positions(&c.worktrees, &c.wt_sel, marks, |r| r.raw_name.clone()).len();
        let bulk = is_bulk_selection(&c.wt_sel, marks);
        let title = pane_title(TITLE_WORKTREES, n, bulk);
        render_collapsed_pane(
            frame,
            regions[2],
            &title,
            &wt_summary,
            matches!(c.ui.focus, PaneId::Worktrees),
            PaneId::Worktrees,
            pane_buttons(PaneId::Worktrees),
            &mut btn_hits,
            p,
            spotlight && !c.searching[2],
            bulk,
        );
    } else {
        render_list_pane(
            frame,
            regions[2],
            hits,
            p,
            TITLE_WORKTREES,
            &wt_summary,
            &c.ui.search[2],
            c.searching[2],
            matches!(c.ui.focus, PaneId::Worktrees),
            PaneId::Worktrees,
            ListPane::Worktrees,
            &c.wt_sel,
            &c.ui.marks[ListPane::Worktrees.idx()],
            &c.worktrees,
            "no worktrees",
            app.now_epoch_s,
            pane_buttons(PaneId::Worktrees),
            WORKTREE_ROW_SCOPED,
            &mut btn_hits,
            wt_col_layout,
            |row, layout, p| worktree_line(row, layout, p, app.now_epoch_s),
            wt_header,
            |_| false,
            |_| false,
            |row| row.raw_name.clone(),
            spotlight && !c.searching[2],
            None, // worktrees pane has no section divider
            // PR-link cell: when the PR column is present and the row has BOTH a
            // number and its url, wrap the painted `#<n>` glyphs in an OSC 8
            // terminal hyperlink so cmd+click opens it (handled by the terminal,
            // not the app). The x comes from the shared `pr_col_x` so it tracks
            // the exact cell the line builder drew; the width is the visible
            // `#<n>` glyph count. Rows without a url render an inert plain chip.
            |row, rect, layout, buf| {
                if layout.pr_w == 0 {
                    return;
                }
                if let (Some(n), Some(url)) = (row.pr_number, row.pr_url.as_deref()) {
                    let w = crate::selectors::clip(&format!("#{n}"), layout.pr_w).chars().count();
                    crate::view::apply_osc8(buf, rect.x + layout.pr_col_x() as u16, rect.y, w as u16, url);
                }
            },
        );
    }

    // Draggable horizontal dividers: each occupies the two border rows shared by
    // adjacent panes (bottom border of the upper pane + top border of the lower).
    // Registered AFTER all pane chrome/bodies so they win the reverse hit scan on
    // those border rows (pane bodies never include border cells, so no conflict).
    // The drag handler no-ops a boundary adjacent to a collapsed pane.
    let b0 = area.y + layout.queue_h; // first row of the tasks pane
    hits.push(
        Rect { x: area.x, y: b0.saturating_sub(1), width: area.width, height: 2 },
        HitTarget::PaneDividerH(0),
    );
    let b1 = area.y + layout.queue_h + layout.tasks_h; // first row of the worktrees pane
    hits.push(
        Rect { x: area.x, y: b1.saturating_sub(1), width: area.width, height: 2 },
        HitTarget::PaneDividerH(1),
    );

    // Title-bar buttons register LAST of all so each chip wins its sub-rect over
    // the divider band sharing the border row. There is deliberately NO
    // whole-title-row collapse target: it swallowed divider drags starting on
    // the border (collapse stays on the [z] chip and the `z` key).
    for (rect, pane, button) in btn_hits {
        hits.push(rect, HitTarget::PaneButton(pane, button));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_line_shows_protected_marker_in_its_own_slot_beside_dirty() {
        use crate::selectors::{wt_col_layout, WorktreeRow};
        let p = Palette::default();
        // Protected AND dirty: both single-cell front markers show at once
        // (the old 🔒 replaced the ± slot; the ⛨ column is independent).
        let protected = WorktreeRow {
            name: "legal-lake".into(),
            raw_name: "legal-lake".into(),
            path: "/x".into(),
            branch: "legal-lake".into(),
            protected: true,
            dirty: Some(true),
            merged: Some(true),
            ..Default::default()
        };
        let plain = WorktreeRow {
            name: "JUS-1".into(),
            raw_name: "JUS-1".into(),
            path: "/y".into(),
            branch: "JUS-1".into(),
            ..Default::default()
        };
        let rows = vec![protected.clone(), plain.clone()];
        let layout = wt_col_layout(&rows, 120);
        let text = |r: &WorktreeRow| {
            worktree_line(r, &layout, &p, 0)
                .spans
                .iter()
                .map(|s| s.content.clone())
                .collect::<String>()
        };
        let prot_text = text(&protected);
        assert!(prot_text.contains(GLYPH_DIRTY));
        assert!(prot_text.contains(GLYPH_PROTECTED));
        assert!(prot_text.contains(GLYPH_MERGED));
        let plain_text = text(&plain);
        assert!(!plain_text.contains(GLYPH_DIRTY));
        assert!(!plain_text.contains(GLYPH_PROTECTED));
        assert!(!plain_text.contains(GLYPH_MERGED));
        // Both markers are single-width and the slots are statically reserved,
        // so the name lands at the same char offset on every row.
        assert_eq!(
            prot_text.chars().position(|c| c == 'l').unwrap(),
            plain_text.chars().position(|c| c == 'J').unwrap()
        );
    }

    #[test]
    fn worktree_line_renders_approved_marker_in_the_merged_slot_and_merged_wins() {
        use crate::selectors::{wt_col_layout, WorktreeRow};
        let p = Palette::default();
        // Approved but not merged → the ✓ marker shows (and no ↣).
        let approved = WorktreeRow {
            name: "JUS-2".into(),
            raw_name: "JUS-2".into(),
            path: "/a".into(),
            branch: "JUS-2".into(),
            merged: Some(false),
            approved: Some(true),
            ..Default::default()
        };
        // Merged AND approved → merged wins; ↣ shows, ✓ does not.
        let merged = WorktreeRow {
            name: "JUS-3".into(),
            raw_name: "JUS-3".into(),
            path: "/m".into(),
            branch: "JUS-3".into(),
            merged: Some(true),
            approved: Some(true),
            ..Default::default()
        };
        let rows = vec![approved.clone(), merged.clone()];
        let layout = wt_col_layout(&rows, 120);
        let text = |r: &WorktreeRow| {
            worktree_line(r, &layout, &p, 0)
                .spans
                .iter()
                .map(|s| s.content.clone())
                .collect::<String>()
        };
        let approved_text = text(&approved);
        assert!(approved_text.contains(GLYPH_APPROVED));
        assert!(!approved_text.contains(GLYPH_MERGED));
        let merged_text = text(&merged);
        assert!(merged_text.contains(GLYPH_MERGED));
        assert!(!merged_text.contains(GLYPH_APPROVED));
    }

    #[test]
    fn build_header_keeps_all_buttons_right_aligned_against_the_corner() {
        let p = Palette::default();
        // width 30 → avail 28: the labeled strip can't fit alongside the title,
        // so all three compact chips survive right-aligned, in scope order
        // (row-scoped [g]oto first, then [c]reate [z]).
        let area = Rect { x: 0, y: 0, width: 30, height: 8 };
        let (_line, rects) =
            build_header(area, "Q", None, false, PaneId::Queue, &[PaneButton::Goto, PaneButton::Create, PaneButton::Collapse], 1, false, false, false, &p);
        assert_eq!(rects.len(), 3);
        // avail=28, title "Q"=1, each compact chip=3 cells, base strip=3*(1+3)=12,
        // plus the ` · ` divider (+2, both groups shown) = 14. filler=28-1-14=13.
        // Chips start at x0(1)+title(1)+filler(13)=15; the first (goto) sits one
        // separator space in, at x=16 with width 3.
        assert_eq!(rects[0].0.x, 16);
        assert_eq!(rects[0].0.width, 3);
        assert_eq!(rects[0].1, PaneButton::Goto);
        assert_eq!(rects[2].1, PaneButton::Collapse);
        // The last chip ends flush against the right corner (x0 + avail = 29).
        let last = rects[2].0;
        assert_eq!(last.x + last.width, area.x + 1 + 28);
    }

    #[test]
    fn build_header_uses_labeled_chips_when_wide() {
        let p = Palette::default();
        // width 60 → avail 58 ≥ title(1) + labeled strip + divider: the labeled
        // form fits, so chips carry their label words at full width, in scope
        // order (row-scoped [g]oto first, then [c]reate [z]).
        let area = Rect { x: 0, y: 0, width: 60, height: 8 };
        let (line, rects) =
            build_header(area, "Q", None, false, PaneId::Queue, &[PaneButton::Goto, PaneButton::Create, PaneButton::Collapse], 1, false, false, false, &p);
        assert_eq!(rects.len(), 3);
        // Labeled widths — `[g]oto`/`[c]reate` merge the key into the word
        // (goto 3+3=6, create 3+5=8); collapse fuses the same way
        // (`[z]collapse`, 3+8=11).
        assert_eq!(rects[0].0.width, 6);
        assert_eq!(rects[1].0.width, 8);
        assert_eq!(rects[2].0.width, 11);
        let text = line.spans.iter().map(|s| s.content.clone()).collect::<String>();
        assert!(text.contains("[g]oto"));
        assert!(text.contains("[c]reate"));
        assert!(text.contains("[z]collapse"));
        // The `·` scope divider sits between the two groups.
        assert!(text.contains('·'), "group divider renders: {text}");
        // Still right-aligned flush against the corner.
        let last = rects[2].0;
        assert_eq!(last.x + last.width, area.x + 1 + 58);
    }

    #[test]
    fn bulk_dims_only_the_not_bulk_doable_chips() {
        let p = Palette::default();
        let area = Rect { x: 0, y: 0, width: 60, height: 8 };
        // QUEUE's doable bulk verbs (Run/Cancel) keep their normal accent key
        // style even under a bulk selection; Goto (not doable) dims.
        let (line, _) = build_header(
            area,
            "Q",
            None,
            false,
            PaneId::Queue,
            &[PaneButton::Run, PaneButton::Cancel, PaneButton::Goto],
            2,
            false,
            true, // bulk
            false,
            &p,
        );
        let accent_key = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
        let dim = p.dim_style();
        let key_spans: Vec<&Span> = line.spans.iter().filter(|s| s.content.starts_with('[')).collect();
        assert_eq!(key_spans.len(), 3);
        assert_eq!(key_spans[0].style, accent_key, "rerun stays live (bulk-doable)");
        assert_eq!(key_spans[1].style, accent_key, "stop stays live (bulk-doable)");
        assert_eq!(key_spans[2].style, dim, "goto dims (not bulk-doable)");

        // The same buttons WITHOUT a bulk selection all stay live.
        let (line, _) = build_header(
            area,
            "Q",
            None,
            false,
            PaneId::Queue,
            &[PaneButton::Run, PaneButton::Cancel, PaneButton::Goto],
            2,
            false,
            false, // no bulk selection
            false,
            &p,
        );
        let key_spans: Vec<&Span> = line.spans.iter().filter(|s| s.content.starts_with('[')).collect();
        assert!(key_spans.iter().all(|s| s.style == accent_key), "no dimming without a bulk selection");
    }

    #[test]
    fn build_header_worktrees_includes_row_verbs_and_tasks_chip() {
        let p = Palette::default();
        // Wide enough for the labeled form: the worktrees strip carries six chips
        // in scope order — row-scoped [r]un [g]oto [x]remove [t]asks, then the `·`
        // divider, then pane-scoped [c]reate [z]collapse.
        let area = Rect { x: 0, y: 0, width: 110, height: 8 };
        let (line, rects) =
            build_header(area, "WORKTREES", None, false, PaneId::Worktrees, pane_buttons(PaneId::Worktrees), WORKTREE_ROW_SCOPED, false, false, false, &p);
        assert_eq!(rects.len(), 6);
        assert_eq!(rects[0].1, PaneButton::Run);
        assert_eq!(rects[1].1, PaneButton::Goto);
        assert_eq!(rects[2].1, PaneButton::Remove);
        assert_eq!(rects[3].1, PaneButton::Tasks);
        assert_eq!(rects[4].1, PaneButton::Create);
        assert_eq!(rects[5].1, PaneButton::Collapse);
        let text = line.spans.iter().map(|s| s.content.clone()).collect::<String>();
        assert!(text.contains("[g]oto"), "labeled goto chip renders: {text}");
        assert!(text.contains("[x]remove"), "labeled remove chip renders: {text}");
        assert!(text.contains("[t]asks"), "labeled tasks chip renders: {text}");
        assert!(text.contains("[c]reate"), "labeled create chip renders: {text}");
    }

    #[test]
    fn fit_chip_strip_drops_labels_before_dropping_chips() {
        // Three chips: labeled widths [13,14,15], compact widths [6,6,6]. Boundary
        // at 3 (== chip count) so the divider condition (kept > group1) never
        // fires — this pins the label/chip degradation in isolation.
        let labeled = [13usize, 14, 15];
        let compact = [6usize, 6, 6];
        // Wide: labeled strip 45 + title 1 = 46 ≤ 58 → labeled, all kept.
        assert_eq!(fit_chip_strip(1, 58, &labeled, &compact, 3, 2), (true, 3));
        // Narrower: labeled doesn't fit but all compact chips do → compact, 3.
        assert_eq!(fit_chip_strip(1, 38, &labeled, &compact, 3, 2), (false, 3));
        // Narrow: compact chips drop from the right (collapse first).
        assert_eq!(fit_chip_strip(1, 15, &labeled, &compact, 3, 2), (false, 2));
        assert_eq!(fit_chip_strip(1, 8, &labeled, &compact, 3, 2), (false, 1));
        assert_eq!(fit_chip_strip(1, 6, &labeled, &compact, 3, 2), (false, 0));
    }

    #[test]
    fn fit_chip_strip_accounts_for_group_separator() {
        // Compact widths [6,6,6], labeled forced never to fit, boundary after 1
        // (row-scoped group of one). The `·` divider adds `sep_extra` cells the
        // moment the kept run crosses into the second group, so it can force an
        // extra drop compared with a boundary that never divides.
        let labeled = [20usize, 20, 20];
        let compact = [6usize, 6, 6];
        // avail 15, boundary 1: kept=2 costs (1+6)+(1+6)+2 = 16 > 15 → drop to 1.
        assert_eq!(fit_chip_strip(0, 15, &labeled, &compact, 1, 2), (false, 1));
        // Same width, boundary 3 (no divider ever): kept=2 costs 14 ≤ 15 → 2.
        assert_eq!(fit_chip_strip(0, 15, &labeled, &compact, 3, 2), (false, 2));
    }

    #[test]
    fn pane_vplan_thresholds() {
        // Below 5: no spacers (density preserved).
        assert_eq!(pane_vplan(1), (false, false, 0)); // hint only, no data
        assert_eq!(pane_vplan(3), (false, false, 2)); // hint + 2 data rows
        assert_eq!(pane_vplan(4), (false, false, 3)); // hint + 3 data rows
        // At 5: BOTTOM spacer only (the HEADER row wins over the cosmetic top
        // gap), ≥3 data rows.
        assert_eq!(pane_vplan(5), (false, true, 3)); // hint + headers + 3 data rows
        // From 6 up: both spacers, still ≥3 data rows.
        assert_eq!(pane_vplan(6), (true, true, 3)); // top + hint + bottom + 3 data
        assert_eq!(pane_vplan(10), (true, true, 7));
    }

    #[test]
    fn build_header_drops_buttons_from_the_right_when_narrow() {
        let p = Palette::default();
        // avail = width-2 = 8. Labeled can't fit → compact. At kept=1 there is no
        // divider (single group shown): title "Q"(1) + chip(1+3)=4 → 5 ≤ 8. At
        // kept=2 both groups show, so + the divider: 1 + (1+3)+(1+3) + 2 = 11 > 8.
        // Only the leftmost (row-scoped [g]oto) stays.
        let area = Rect { x: 0, y: 0, width: 10, height: 8 };
        let (_line, rects) =
            build_header(area, "Q", None, false, PaneId::Queue, &[PaneButton::Goto, PaneButton::Create, PaneButton::Collapse], 1, false, false, false, &p);
        assert_eq!(rects.len(), 1, "only the leftmost (goto) button survives");
        assert_eq!(rects[0].1, PaneButton::Goto);
    }

    #[test]
    fn build_header_drops_all_buttons_when_title_fills_width() {
        let p = Palette::default();
        // Title alone overflows the 6-cell interior → no buttons; title truncates.
        let area = Rect { x: 0, y: 0, width: 8, height: 8 };
        let (_line, rects) =
            build_header(area, "WORKTREES", None, false, PaneId::Worktrees, pane_buttons(PaneId::Worktrees), WORKTREE_ROW_SCOPED, false, false, false, &p);
        assert!(rects.is_empty());
    }

    #[test]
    fn build_header_collapse_label_flips_with_state() {
        let p = Palette::default();
        let area = Rect { x: 0, y: 0, width: 40, height: 8 };
        let (expanded, _) =
            build_header(area, "Q", None, false, PaneId::Queue, &[PaneButton::Goto, PaneButton::Create, PaneButton::Collapse], 1, false, false, false, &p);
        let (collapsed, _) =
            build_header(area, "Q", None, false, PaneId::Queue, &[PaneButton::Goto, PaneButton::Create, PaneButton::Collapse], 1, true, false, false, &p);
        let text = |l: &Line| l.spans.iter().map(|s| s.content.clone()).collect::<String>();
        assert!(text(&expanded).contains(BTN_LABEL_COLLAPSE));
        assert!(text(&collapsed).contains(BTN_LABEL_EXPAND));
    }

    #[test]
    fn build_header_archive_label_flips_with_first_selected_row() {
        // The `[a]rchive` chip reads `archive` by default and `unarchive` when
        // the first selected row is archived (the `archive_unarchive` flag).
        let p = Palette::default();
        let area = Rect { x: 0, y: 0, width: 60, height: 8 };
        // The key `a` renders inside `[…]`, so the chip reads `[a]rchive` /
        // `[a]unarchive` (assert the rendered forms, not the bare labels — the
        // bracketed key splits the label so "archive" isn't a raw substring).
        let text = |l: &Line| l.spans.iter().map(|s| s.content.clone()).collect::<String>();
        let (archive, _) =
            build_header(area, "Q", None, false, PaneId::Queue, &[PaneButton::Archive], 1, false, false, false, &p);
        assert!(text(&archive).contains("[a]rchive"), "default: {}", text(&archive));
        assert!(!text(&archive).contains("[a]unarchive"));
        let (unarchive, _) =
            build_header(area, "Q", None, false, PaneId::Queue, &[PaneButton::Archive], 1, false, false, true, &p);
        assert!(text(&unarchive).contains("[a]unarchive"), "flipped: {}", text(&unarchive));
    }
}
