use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use throbber_widgets_tui::{Throbber, ThrobberState};

use crate::app::{App, ListPane, PaneId, Selection};
use crate::hit::{HitMap, HitTarget, PaneButton};
use crate::ipc::types::DefinitionSummary;
use crate::selectors::{
    COLLAPSED_H, DefColLayout, QueueColLayout, QueueRow, WorktreeRow, WtColLayout, WtState,
    absolute_local_label, cron_human, def_col_layout, elapsed_label, pad_clip,
    pane_layout, pane_title, queue_col_layout, queue_divider_after, relative_age_label,
    wt_author_text, wt_col_layout,
};
use crate::view::theme::{
    BTN_ACTIONS, BTN_COLLAPSE, BTN_CREATE, BTN_EXPAND, BTN_LABEL_ACTIONS, BTN_LABEL_COLLAPSE,
    BTN_LABEL_CREATE, BTN_LABEL_EXPAND, FENCE_RULE_MIN_TRAIL, FENCE_RULE_PREFIX, GLYPH_CURSOR,
    GLYPH_DIRTY, GLYPH_DISCOVERY, GLYPH_DOT, GLYPH_MAIN_SESSION, GLYPH_SEARCH, Palette, RULE_CHAR,
    SEARCH_HINT_IDLE, TITLE_QUEUE, TITLE_TASKS, TITLE_WORKTREES, glyph_style,
};

/// Title-bar buttons per pane, in left-to-right order. Narrow panes drop them
/// from the right (collapse first), so the create/actions chips survive longest;
/// collapse always keeps its `z` key binding.
const QUEUE_BUTTONS: &[PaneButton] = &[PaneButton::Create, PaneButton::Actions, PaneButton::Collapse];
const TASKS_BUTTONS: &[PaneButton] = &[PaneButton::Actions, PaneButton::Collapse];
const WORKTREE_BUTTONS: &[PaneButton] =
    &[PaneButton::Create, PaneButton::Actions, PaneButton::Collapse];
use crate::view::{Computed, selection_range, window_start};

/// The bold pane title, accent-colored when focused. Shared by the border-title
/// on both the expanded chrome and the collapsed bar.
fn title_span(title: &str, focused: bool, p: &Palette) -> Span<'static> {
    Span::styled(
        title.to_string(),
        Style::default()
            .fg(if focused { p.accent } else { p.fg })
            .add_modifier(Modifier::BOLD),
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
/// Labeled form is `{icon} [{key}] {label}` (icon in fg, `[k]` accent+bold, a
/// space, then the lowercase label word in fg); the compact form drops the ` {label}`
/// tail, leaving `{icon} [{key}]`. Hotkeys always render in square brackets —
/// the global convention across chips, hint rows, footer, and help. The icon is
/// a double-width emoji, so width is measured with `Span::width()` (ratatui's
/// unicode-width) rather than assumed, so a future glyph or renamed label can't
/// silently desync the border-fill / right-alignment / hit-rect math. The
/// collapse chip flips both glyph and label on `collapsed`.
fn button_chip(b: PaneButton, collapsed: bool, labeled: bool, p: &Palette) -> (Vec<Span<'static>>, u16) {
    let (icon, key, label) = match b {
        PaneButton::Create => (BTN_CREATE, 'c', BTN_LABEL_CREATE),
        PaneButton::Actions => (BTN_ACTIONS, 'a', BTN_LABEL_ACTIONS),
        PaneButton::Collapse => {
            let (icon, label) = if collapsed {
                (BTN_EXPAND, BTN_LABEL_EXPAND)
            } else {
                (BTN_COLLAPSE, BTN_LABEL_COLLAPSE)
            };
            (icon, 'z', label)
        }
    };
    let key_style = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let mut spans = vec![
        Span::styled(icon.to_string(), Style::default().fg(p.fg)),
        Span::raw(" "),
        Span::styled(format!("[{key}]"), key_style),
    ];
    if labeled {
        // `[c]reate` / `[a]ctions` when the key is the label's first letter
        // (the footer's `[q]uit` pattern); otherwise `[z] collapse`.
        match label.strip_prefix(key) {
            Some(rest) => spans.push(Span::styled(rest.to_string(), Style::default().fg(p.fg))),
            None => {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(label.to_string(), Style::default().fg(p.fg)));
            }
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
/// space plus its cell width. Returns `(labeled, kept)`.
fn fit_chip_strip(
    title_w: usize,
    avail: usize,
    labeled_widths: &[usize],
    compact_widths: &[usize],
) -> (bool, usize) {
    let strip = |widths: &[usize], kept: usize| -> usize {
        widths[..kept].iter().map(|w| 1 + w).sum()
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
/// then its action-button chips pushed to the RIGHT END of the top border (just
/// before the corner), with a run of border characters filling the gap between.
/// Chips drop from the right until the title + strip fits between the corners
/// (each chip costs a leading space + its cell width); when even zero chips fit
/// the title is left to be clipped by `Block::title`. Returns the border `title`
/// Line to draw and the absolute hit rects for the surviving chips at their
/// right-aligned coordinates. All widths are cell widths (`Span::width`), so the
/// double-width emoji icons stay aligned.
fn build_header(
    area: Rect,
    title: &str,
    focused: bool,
    buttons: &[PaneButton],
    collapsed: bool,
    p: &Palette,
) -> (Line<'static>, Vec<(Rect, PaneButton)>) {
    let x0 = area.x + 1;
    let avail = area.width.saturating_sub(2) as usize; // cells between the corners
    let title_span = title_span(title, focused, p);
    let title_w = title_span.width();

    // Two-stage degradation: build both the labeled and the compact chip sets,
    // then let `fit_chip_strip` decide which form fits and how many survive.
    // Labels are all-or-nothing (dropped together to stay visually consistent);
    // chip-dropping happens only in the compact form.
    let labeled: Vec<(Vec<Span<'static>>, usize)> = buttons
        .iter()
        .map(|&b| {
            let (spans, w) = button_chip(b, collapsed, true, p);
            (spans, w as usize)
        })
        .collect();
    let compact: Vec<(Vec<Span<'static>>, usize)> = buttons
        .iter()
        .map(|&b| {
            let (spans, w) = button_chip(b, collapsed, false, p);
            (spans, w as usize)
        })
        .collect();
    let labeled_w: Vec<usize> = labeled.iter().map(|(_, w)| *w).collect();
    let compact_w: Vec<usize> = compact.iter().map(|(_, w)| *w).collect();
    let (use_labeled, kept) = fit_chip_strip(title_w, avail, &labeled_w, &compact_w);
    let chips = if use_labeled { labeled } else { compact };
    let strip_cost = |kept: usize| -> usize {
        chips[..kept].iter().map(|(_, w)| 1 + w).sum()
    };

    // No chips fit: leave the border to draw itself and let Block clip the title.
    if kept == 0 {
        return (Line::from(vec![title_span]), Vec::new());
    }

    let strip_w = strip_cost(kept);
    // Border-character run between the title and the right-aligned chip strip.
    let filler_w = avail.saturating_sub(title_w + strip_w);
    let mut spans = vec![title_span];
    if filler_w > 0 {
        spans.push(Span::styled("─".repeat(filler_w), p.border_style(focused)));
    }
    // Chips begin at the first cell after title + filler; the strip ends flush
    // against the corner at x0 + avail.
    let mut x = x0.saturating_add((title_w + filler_w) as u16);
    let mut rects = Vec::new();
    for (&b, (chip, w)) in buttons.iter().zip(chips).take(kept) {
        spans.push(Span::raw(" ")); // separator
        x = x.saturating_add(1);
        rects.push((Rect { x, y: area.y, width: w as u16, height: 1 }, b));
        spans.extend(chip);
        x = x.saturating_add(w as u16);
    }
    (Line::from(spans), rects)
}

/// An expanded list pane's vertical layout plan: a blank spacer row above the
/// search-hint row (gap under the title border), the hint row itself (always
/// present, one row), an optional blank spacer below the hint (simulated "line
/// height"), then the data rows. Both spacers are inert — no hit target, not
/// selectable — they only shift the data rows (and every geometry derived from
/// `rows_area`) down. `inner_height` is the pane's inner (inside-border) row
/// count. Degradation prioritizes the gap under the title:
///
/// - `inner_height ≥ 6`: both spacers (hint + 2 spacers + ≥3 data rows).
/// - `inner_height == 5`: TOP spacer only (hint + top spacer + 3 data rows).
/// - `inner_height < 5`: no spacers (density preserved).
///
/// Returns `(top_spacer, bottom_spacer, data_capacity)`.
fn pane_vplan(inner_height: u16) -> (bool, bool, u16) {
    let top_spacer = inner_height >= 5;
    let bottom_spacer = inner_height >= 6;
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
    focused: bool,
    pane: PaneId,
    buttons: &[PaneButton],
    btn_hits: &mut Vec<(Rect, PaneId, PaneButton)>,
    p: &Palette,
    dimmed: bool,
) {
    // A collapsed bar shows ONLY the expand chip: it is the bar's primary
    // affordance and must never be the one dropped for width (create/actions
    // return with the pane).
    let _ = buttons;
    let (mut header, rects) =
        build_header(area, title, focused, &[PaneButton::Collapse], true, p);
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
/// `🔍 [/] filter` with the hotkey accent+bold (hotkeys always in square
/// brackets, never grey — the visible-hotkey convention). Searching that pane:
/// `🔍 /{query}█` with a colored query + block cursor. Filter set but not
/// searching: `🔍 /{query}` colored, no cursor.
fn search_hint_line(query: &str, searching: bool, p: &Palette) -> Line<'static> {
    // 🔍 is double-width but it is the first column, so nothing after it shifts.
    let mut spans = vec![Span::raw(format!("{GLYPH_SEARCH} "))];
    if searching {
        spans.push(Span::styled(format!("/{query}"), Style::default().fg(p.info)));
        spans.push(Span::styled(GLYPH_CURSOR.to_string(), Style::default().fg(p.info)));
    } else if query.is_empty() {
        spans.push(Span::styled(
            "[/]".to_string(),
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {SEARCH_HINT_IDLE}"), Style::default().fg(p.fg)));
    } else {
        spans.push(Span::styled(format!("/{query}"), Style::default().fg(p.info)));
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
    // Chain column (fixed slot when any visible row resumes its main session).
    if layout.has_chain {
        spans.push(Span::raw(" "));
        if row.main_session {
            spans.push(Span::styled(GLYPH_MAIN_SESSION.to_string(), Style::default().fg(p.info)));
        } else {
            spans.push(Span::raw(" "));
        }
    }
    let gap = " ".repeat(crate::selectors::COL_GAP);
    if layout.worktree_w > 0 {
        spans.push(Span::raw(gap.clone()));
        spans.push(Span::styled(pad_clip(&row.worktree, layout.worktree_w), Style::default().fg(p.accent)));
    }
    if layout.def_w > 0 {
        spans.push(Span::raw(gap.clone()));
        let def = row.def_name.as_deref().unwrap_or("");
        spans.push(Span::styled(pad_clip(def, layout.def_w), Style::default().fg(p.mauve)));
    }
    spans.push(Span::raw(gap.clone()));
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

fn worktree_line(
    row: &WorktreeRow,
    layout: &WtColLayout,
    p: &Palette,
    now_epoch_s: u64,
) -> Line<'static> {
    let dot = match row.state {
        WtState::Free => p.ok,
        WtState::Busy | WtState::You => p.warn,
        WtState::Failed => p.error,
    };
    let gap = " ".repeat(crate::selectors::COL_GAP);
    // Informational columns read in real palette colors (never grey — grey-on-dark
    // was unreadable per user feedback); the de-emphasis dim is only for archived/
    // empty rows via `patch_line`. A column present in the layout but empty on this
    // row renders as blank padding so the fields stay aligned down the pane.
    let info = Style::default().fg(p.info);
    let warn = Style::default().fg(p.warn);
    let mauve = Style::default().fg(p.mauve);
    let fg = Style::default().fg(p.fg);
    // Anchor: `●` + space, then the front marker cluster — the ⌂ main-session
    // marker and the `±` uncommitted-changes marker (each a single-cell slot,
    // present when any visible row carries the value) — then the accent-colored
    // worktree identity name.
    let mut spans = vec![Span::styled(GLYPH_DOT.to_string(), Style::default().fg(dot)), Span::raw(" ")];
    if layout.has_chain {
        if row.has_main_session {
            spans.push(Span::styled(GLYPH_MAIN_SESSION.to_string(), info));
        } else {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::raw(" "));
    }
    if layout.dirty_w > 0 {
        if row.dirty == Some(true) {
            spans.push(Span::styled(GLYPH_DIRTY.to_string(), warn));
        } else {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(pad_clip(&row.name, layout.name_w), Style::default().fg(p.accent)));
    // Last finished lane task: status glyph (status-colored) + name (mauve when a
    // def, fg when a prompt) + relative age (info), padded to the reserved width.
    if layout.last_w > 0 {
        spans.push(Span::raw(gap.clone()));
        match &row.last {
            Some((glyph, name, epoch, is_def)) => {
                let age = relative_age_label(*epoch, now_epoch_s);
                let name_budget = layout.last_w.saturating_sub(3 + age.chars().count());
                let shown = crate::selectors::clip(name, name_budget);
                spans.push(Span::styled(glyph.to_string(), glyph_style(*glyph, p)));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(shown.clone(), if *is_def { mauve } else { fg }));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(age.clone(), info));
                let used = 2 + shown.chars().count() + 1 + age.chars().count();
                if used < layout.last_w {
                    spans.push(Span::raw(" ".repeat(layout.last_w - used)));
                }
            }
            None => spans.push(Span::raw(pad_clip("", layout.last_w))),
        }
    }
    // Last-commit author name (info); pairs with the commit-age to read
    // `koshea  3d ago` = who · when. Clipped with `…` past AUTHOR_W.
    if layout.author_w > 0 {
        spans.push(Span::raw(gap.clone()));
        match wt_author_text(row) {
            Some(t) => spans.push(Span::styled(pad_clip(&t, layout.author_w), info)),
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
    // `N queued · next: <name>` FILL column: the `N queued · next:` lead in info
    // (the queued-count text), the def/prompt name mauve (def) or fg (prompt).
    // Blank-padded when the row has no queued task — the fill still reserves the
    // slack so the trailing live timer stays right-pinned.
    if layout.queued_w > 0 {
        spans.push(Span::raw(gap.clone()));
        match (row.queued, &row.next_name) {
            (0, _) => spans.push(Span::raw(pad_clip("", layout.queued_w))),
            (n, Some(name)) => {
                let lead = format!("{n} queued · next: ");
                let name_budget = layout.queued_w.saturating_sub(lead.chars().count());
                let shown = crate::selectors::clip(name, name_budget);
                spans.push(Span::styled(lead.clone(), info));
                spans.push(Span::styled(shown.clone(), if row.next_is_def { mauve } else { fg }));
                let used = lead.chars().count() + shown.chars().count();
                if used < layout.queued_w {
                    spans.push(Span::raw(" ".repeat(layout.queued_w - used)));
                }
            }
            (n, None) => {
                spans.push(Span::styled(pad_clip(&format!("{n} queued"), layout.queued_w), info))
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

fn def_line(def: &DefinitionSummary, layout: &DefColLayout, p: &Palette) -> Line<'static> {
    let gap = " ".repeat(crate::selectors::COL_GAP);
    // Task/definition names read in mauve — the single semantic color for a def
    // name across QUEUE, TASKS, and the WORKTREES next/last cells.
    let mut spans = vec![Span::styled(pad_clip(&def.name, layout.name_w), Style::default().fg(p.mauve))];
    // Args column: padded to the widest visible args cell so the schedule column
    // never slides left when a def has no args. Teal — informational columns get
    // real colors, not grey.
    if layout.args_w > 0 {
        spans.push(Span::raw(gap.clone()));
        spans.push(Span::styled(
            pad_clip(&crate::selectors::def_args_text(def), layout.args_w),
            Style::default().fg(p.info),
        ));
    }
    // Description FILL column: prose in plain fg (like the queue summary), padded
    // to the reserved remainder so the schedule stays right-pinned; blank when a
    // def has no description. Omitted entirely when the pane reserved no fill.
    if layout.desc_w > 0 {
        spans.push(Span::raw(gap.clone()));
        spans.push(Span::styled(
            pad_clip(&crate::selectors::def_desc_text(def), layout.desc_w),
            Style::default().fg(p.fg),
        ));
    }
    // Schedule column: the ⏰ icon plus the humanized cron when the def has one;
    // a def with a discovery command but no cron keeps the bare ⏰ marker; a def
    // with neither leaves the column blank. Teal/info like the args column.
    match def.cron.as_deref().and_then(cron_human) {
        Some(human) => {
            spans.push(Span::raw(gap));
            spans.push(Span::styled(
                format!("{GLYPH_DISCOVERY} {}", pad_clip(&human, layout.sched_w)),
                Style::default().fg(p.info),
            ));
        }
        None if def.has_discovery => {
            spans.push(Span::raw(gap));
            spans.push(Span::styled(GLYPH_DISCOVERY.to_string(), Style::default().fg(p.info)));
        }
        None => {}
    }
    Line::from(spans)
}

/// Register a vertical scrollbar hit region (track + proportional thumb) and draw
/// the built-in Scrollbar. `total` rows, `offset` first-visible, `visible` rows.
fn render_scrollbar(
    frame: &mut ratatui::Frame,
    area: Rect,
    total: usize,
    offset: usize,
    visible: usize,
    pane: PaneId,
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
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight),
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
    search: &str,
    searching: bool,
    focused: bool,
    pane: PaneId,
    list_pane: ListPane,
    sel: &Selection,
    rows: &[T],
    empty_msg: &str,
    now_epoch_s: u64,
    buttons: &[PaneButton],
    btn_hits: &mut Vec<(Rect, PaneId, PaneButton)>,
    // Per-frame column widths derived from the VISIBLE rows + available width, so
    // fields line up across what is actually on screen (recomputed each frame).
    ctx_of: impl Fn(&[T], usize) -> C,
    line_of: impl Fn(&T, &C, &Palette) -> Line<'static>,
    dim_of: impl Fn(&T) -> bool,
    running_of: impl Fn(&T) -> bool,
    dimmed: bool,
    // Real-row index after which to splice an inert section divider (the QUEUE
    // ACTIVE/FINISHED break). `None` → no divider (tasks/worktrees always pass it).
    divider_after: Option<usize>,
) {
    let title = pane_title(title_base, sel);
    let (mut header, rects) = build_header(area, &title, focused, buttons, false, p);
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

    let (start_i, end_i) = selection_range(sel);
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

    let mut lines: Vec<Line> = Vec::with_capacity(visible);
    for vd in 0..visible {
        let y = rows_area.y + vd as u16;
        match &display[offset + vd] {
            DisplayRow::Row(idx) => {
                let idx = *idx;
                let mut line = line_of(&rows[idx], &ctx, p);
                let selected = focused && idx >= start_i && idx <= end_i;
                if dimmed {
                    // Spotlight: another pane is being search-typed — mute this whole
                    // pane (a dimmed pane is never focused, so no selection to show).
                    patch_line(&mut line, p.dim_style());
                } else if selected {
                    // Patch every span so per-span fg colors don't survive over the
                    // selection bg (a blue worktree name on blue bg would be unreadable).
                    patch_line(&mut line, p.selection());
                } else if dim_of(&rows[idx]) {
                    // Same reason: force the archived row uniformly dim rather than
                    // leaving colored glyphs/worktree names.
                    patch_line(&mut line, p.dim_style());
                }
                lines.push(line);
                hits.push(
                    Rect { x: rows_area.x, y, width: rows_area.width, height: 1 },
                    HitTarget::Row(list_pane, idx),
                );
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
    render_scrollbar(frame, rows_area, total, offset, visible, pane, hits);
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
    if collapsed[ListPane::Queue.idx()] {
        let title = pane_title(TITLE_QUEUE, &c.queue_sel);
        render_collapsed_pane(
            frame,
            regions[0],
            &title,
            matches!(c.ui.focus, PaneId::Queue),
            PaneId::Queue,
            QUEUE_BUTTONS,
            &mut btn_hits,
            p,
            spotlight && !c.searching[0],
        );
    } else {
        render_list_pane(
            frame,
            regions[0],
            hits,
            p,
            TITLE_QUEUE,
            &c.ui.search[0],
            c.searching[0],
            matches!(c.ui.focus, PaneId::Queue),
            PaneId::Queue,
            ListPane::Queue,
            &c.queue_sel,
            &c.queue,
            "queue empty — [a] on a worktree to add a task",
            app.now_epoch_s,
            QUEUE_BUTTONS,
            &mut btn_hits,
            |rows, avail| queue_col_layout(rows, avail, app.now_epoch_s),
            |row, layout, p| queue_line(row, layout, p, app.now_epoch_s, tz_offset),
            |row| row.archived,
            |row| row.running,
            spotlight && !c.searching[0],
            // ACTIVE/FINISHED section divider (drawn only when both sections exist).
            queue_divider_after(&c.queue),
        );
    }
    if collapsed[ListPane::Tasks.idx()] {
        let title = pane_title(TITLE_TASKS, &c.tasks_sel);
        render_collapsed_pane(
            frame,
            regions[1],
            &title,
            matches!(c.ui.focus, PaneId::Tasks),
            PaneId::Tasks,
            TASKS_BUTTONS,
            &mut btn_hits,
            p,
            spotlight && !c.searching[1],
        );
    } else {
        render_list_pane(
            frame,
            regions[1],
            hits,
            p,
            TITLE_TASKS,
            &c.ui.search[1],
            c.searching[1],
            matches!(c.ui.focus, PaneId::Tasks),
            PaneId::Tasks,
            ListPane::Tasks,
            &c.tasks_sel,
            &c.defs,
            "no task definitions",
            app.now_epoch_s,
            TASKS_BUTTONS,
            &mut btn_hits,
            def_col_layout,
            def_line,
            |_| false,
            |_| false,
            spotlight && !c.searching[1],
            None, // tasks pane has no section divider
        );
    }
    if collapsed[ListPane::Worktrees.idx()] {
        let title = pane_title(TITLE_WORKTREES, &c.wt_sel);
        render_collapsed_pane(
            frame,
            regions[2],
            &title,
            matches!(c.ui.focus, PaneId::Worktrees),
            PaneId::Worktrees,
            WORKTREE_BUTTONS,
            &mut btn_hits,
            p,
            spotlight && !c.searching[2],
        );
    } else {
        render_list_pane(
            frame,
            regions[2],
            hits,
            p,
            TITLE_WORKTREES,
            &c.ui.search[2],
            c.searching[2],
            matches!(c.ui.focus, PaneId::Worktrees),
            PaneId::Worktrees,
            ListPane::Worktrees,
            &c.wt_sel,
            &c.worktrees,
            "no worktrees",
            app.now_epoch_s,
            WORKTREE_BUTTONS,
            &mut btn_hits,
            wt_col_layout,
            |row, layout, p| worktree_line(row, layout, p, app.now_epoch_s),
            |_| false,
            |_| false,
            spotlight && !c.searching[2],
            None, // worktrees pane has no section divider
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
    // the border (collapse stays on the 🔽 [z] chip and the `z` key).
    for (rect, pane, button) in btn_hits {
        hits.push(rect, HitTarget::PaneButton(pane, button));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_header_keeps_all_buttons_right_aligned_against_the_corner() {
        let p = Palette::default();
        // width 40 → avail 38: the labeled strip (45) can't fit alongside the
        // title, so all three compact chips survive right-aligned.
        let area = Rect { x: 0, y: 0, width: 40, height: 8 };
        let (_line, rects) = build_header(area, "Q", false, QUEUE_BUTTONS, false, &p);
        assert_eq!(rects.len(), 3);
        // avail=38, title "Q"=1, each compact chip=6 cells, strip=3*(1+6)=21,
        // filler=38-1-21=16. Chips start at x0(1)+title(1)+filler(16)=18; first
        // chip after its separator sits at x=19 with width 6.
        assert_eq!(rects[0].0.x, 19);
        assert_eq!(rects[0].0.width, 6);
        assert_eq!(rects[0].1, PaneButton::Create);
        assert_eq!(rects[2].1, PaneButton::Collapse);
        // The last chip ends flush against the right corner (x0 + avail = 39).
        let last = rects[2].0;
        assert_eq!(last.x + last.width, area.x + 1 + 38);
    }

    #[test]
    fn build_header_uses_labeled_chips_when_wide() {
        let p = Palette::default();
        // width 60 → avail 58 ≥ title(1) + labeled strip(41): the labeled form
        // fits, so chips carry their label words at full width.
        let area = Rect { x: 0, y: 0, width: 60, height: 8 };
        let (line, rects) = build_header(area, "Q", false, QUEUE_BUTTONS, false, &p);
        assert_eq!(rects.len(), 3);
        // Labeled widths — `[c]reate`/`[a]ctions` merge the key into the word
        // (create 2+1+3+5=11, actions 2+1+3+6=12); collapse keeps the spaced
        // `[z] collapse` form (2+1+3+1+8=15).
        assert_eq!(rects[0].0.width, 11);
        assert_eq!(rects[1].0.width, 12);
        assert_eq!(rects[2].0.width, 15);
        let text = line.spans.iter().map(|s| s.content.clone()).collect::<String>();
        assert!(text.contains("[c]reate"));
        assert!(text.contains("[a]ctions"));
        assert!(text.contains("[z] collapse"));
        // Still right-aligned flush against the corner.
        let last = rects[2].0;
        assert_eq!(last.x + last.width, area.x + 1 + 58);
    }

    #[test]
    fn fit_chip_strip_drops_labels_before_dropping_chips() {
        // Three chips: labeled widths [13,14,15], compact widths [6,6,6].
        let labeled = [13usize, 14, 15];
        let compact = [6usize, 6, 6];
        // Wide: labeled strip 45 + title 1 = 46 ≤ 58 → labeled, all kept.
        assert_eq!(fit_chip_strip(1, 58, &labeled, &compact), (true, 3));
        // Narrower: labeled doesn't fit but all compact chips do → compact, 3.
        assert_eq!(fit_chip_strip(1, 38, &labeled, &compact), (false, 3));
        // Narrow: compact chips drop from the right (collapse first).
        assert_eq!(fit_chip_strip(1, 15, &labeled, &compact), (false, 2));
        assert_eq!(fit_chip_strip(1, 8, &labeled, &compact), (false, 1));
        assert_eq!(fit_chip_strip(1, 6, &labeled, &compact), (false, 0));
    }

    #[test]
    fn pane_vplan_thresholds() {
        // Below 5: no spacers (density preserved).
        assert_eq!(pane_vplan(1), (false, false, 0)); // hint only, no data
        assert_eq!(pane_vplan(3), (false, false, 2)); // hint + 2 data rows
        assert_eq!(pane_vplan(4), (false, false, 3)); // hint + 3 data rows
        // At 5: TOP spacer only (gap under the title wins), ≥3 data rows.
        assert_eq!(pane_vplan(5), (true, false, 3)); // top + hint + 3 data rows
        // From 6 up: both spacers, still ≥3 data rows.
        assert_eq!(pane_vplan(6), (true, true, 3)); // top + hint + bottom + 3 data
        assert_eq!(pane_vplan(10), (true, true, 7));
    }

    #[test]
    fn build_header_drops_buttons_from_the_right_when_narrow() {
        let p = Palette::default();
        // avail = width-2 = 8. Labeled can't fit → compact. Each compact chip
        // costs 7 (sep+6); title "Q" = 1. 1+7=8 ≤ 8 (one fits), 1+14=15 > 8 (two
        // don't) → only the leftmost stays.
        let area = Rect { x: 0, y: 0, width: 10, height: 8 };
        let (_line, rects) = build_header(area, "Q", false, QUEUE_BUTTONS, false, &p);
        assert_eq!(rects.len(), 1, "only the leftmost (create) button survives");
        assert_eq!(rects[0].1, PaneButton::Create);
    }

    #[test]
    fn build_header_drops_all_buttons_when_title_fills_width() {
        let p = Palette::default();
        // Title alone overflows the 6-cell interior → no buttons; title truncates.
        let area = Rect { x: 0, y: 0, width: 8, height: 8 };
        let (_line, rects) = build_header(area, "WORKTREES", false, WORKTREE_BUTTONS, false, &p);
        assert!(rects.is_empty());
    }

    #[test]
    fn build_header_collapse_glyph_flips_with_state() {
        let p = Palette::default();
        let area = Rect { x: 0, y: 0, width: 40, height: 8 };
        let (expanded, _) = build_header(area, "Q", false, QUEUE_BUTTONS, false, &p);
        let (collapsed, _) = build_header(area, "Q", false, QUEUE_BUTTONS, true, &p);
        let text = |l: &Line| l.spans.iter().map(|s| s.content.clone()).collect::<String>();
        assert!(text(&expanded).contains(BTN_COLLAPSE));
        assert!(text(&collapsed).contains(BTN_EXPAND));
    }
}
