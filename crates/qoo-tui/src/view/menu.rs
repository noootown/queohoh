//! Big lazyvim/snacks-style task/def picker: TWO separate bordered panels side
//! by side:
//! the left panel holds the search prompt (with a right-aligned match count)
//! and the FILTERED rows; the right panel is a scrollable preview (a bold
//! "Description"/"Prompt" heading pair, then the def's markdown-styled prompt)
//! titled with the selected row's label. Both panels register `Modal` hit
//! targets (clicks can't leak to the panes beneath), the left panel registers
//! one `MenuItem(i)` per visible row (`i` = FILTERED display index), and the
//! right panel's interior registers `MenuPreview` so the mouse wheel can
//! scroll the preview. No bottom key hint — arrow-move/enter-run/esc-close are
//! the picker's own established convention.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use crate::event::SessionChoice;
use crate::hit::{ButtonKind, HitMap, HitTarget};
use crate::ipc::types::{DefinitionSummary, TaskDefinition};
use crate::markup::{fence_states, style_display_line, wrap_lines, LineCtx};
use crate::selectors::{absolute_local_label, arg_summary, clip, filter_rows, pad_clip};
use crate::view::modal::{render_button_row, DIALOG_WIDTH, MODAL_PADDING};
use crate::view::theme::{
    GLYPH_CREATE_WORKTREE, GLYPH_CURSOR, GLYPH_NEW_SESSION, Palette,
    RULE_CHAR,
};

/// Render-feedback for the preview scroll: the wheel handler clamps
/// `preview_scroll` against `max_scroll`, measured from the last draw (same
/// freshness argument as `detail_max_scroll`: every state change redraws
/// before the next event).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviewMetrics {
    pub max_scroll: usize,
}

/// The picker's interactive state slice (out of `Mode::DefPick`): the
/// highlighted FILTERED index, the label/name filter, and the preview panel's
/// scroll offset.
#[derive(Debug, Clone, Copy)]
pub struct PickerState<'a> {
    pub index: usize,
    pub query: &'a str,
    pub preview_scroll: usize,
}

/// Picker geometry: one big centered area split into two adjacent bordered
/// panels (left ≈ 45% for the list, right = the preview). Shared with the run
/// form (`args_form::render_run_form`), which reuses the same two-panel shell.
pub(crate) struct PickerLayout {
    /// Left panel rect (borders included) and its interior.
    pub left: Rect,
    pub left_inner: Rect,
    /// Right panel rect (borders included) and its interior.
    pub right: Rect,
    pub right_inner: Rect,
}

/// Big picker: width = 4/5 of the terminal clamped to [60, cols−4], height =
/// 4/5 clamped to [15, rows−2] — degrading gracefully below the floors (the
/// `min` folds "use cols−4 when < 60" into the clamp bounds).
pub(crate) fn picker_layout(area: Rect) -> PickerLayout {
    let max_w = area.width.saturating_sub(4).max(1);
    let width = (area.width as u32 * 4 / 5) as u16;
    let width = width.clamp(60.min(max_w), max_w);
    let max_h = area.height.saturating_sub(2).max(1);
    let height = (area.height as u32 * 4 / 5) as u16;
    let height = height.clamp(15.min(max_h), max_h);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let left_w = (width as u32 * 45 / 100) as u16;
    let left = Rect { x, y, width: left_w, height };
    let right = Rect { x: x + left_w, y, width: width - left_w, height };
    // Border (1) + MODAL_PADDING on every side — same interior inset as the
    // confirm dialog and every other modal, so the picker's two panels read as
    // the same dialog family instead of a flush, un-padded one.
    let inner = |r: Rect| Rect {
        x: r.x + 1 + MODAL_PADDING.left,
        y: r.y + 1 + MODAL_PADDING.top,
        width: r.width.saturating_sub(2 + MODAL_PADDING.left + MODAL_PADDING.right),
        height: r.height.saturating_sub(2 + MODAL_PADDING.top + MODAL_PADDING.bottom),
    };
    PickerLayout { left, left_inner: inner(left), right, right_inner: inner(right) }
}

/// Windowed slice of a filtered list so the highlighted `index` is always in
/// view: returns the first visible filtered position.
fn window_start(index: usize, len: usize, visible: usize) -> usize {
    if visible == 0 || len <= visible {
        0
    } else {
        index.min(len - 1).saturating_sub(visible - 1).min(len - visible)
    }
}

/// Draw both panel frames + the left panel's search prompt, register the Modal
/// targets, and return the layout. `right_title` is the selected row's label
/// (`None` — e.g. no filter match — leaves the right border untitled).
fn render_picker_chrome(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    title: &str,
    right_title: Option<&str>,
    query: &str,
    filtered: usize,
    total: usize,
) -> PickerLayout {
    let p = &Palette::default();
    let layout = picker_layout(frame.area());
    for r in [layout.left, layout.right] {
        frame.render_widget(Clear, r);
        hit.push(r, HitTarget::Modal); // both panels: opaque to clicks
    }

    // Left panel: menu title in the top border. No bottom hint — type-to-filter,
    // arrow-move, and enter-to-run are the picker's own established convention
    // and don't need spelling out (same call as dropping "esc cancel" elsewhere).
    let left_block = Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent));
    frame.render_widget(left_block, layout.left);

    // Right panel: the selected row's label as the title (none when no match).
    let mut right_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent));
    if let Some(t) = right_title {
        right_block = right_block.title(Span::styled(
            format!(" {t} "),
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ));
    }
    frame.render_widget(right_block, layout.right);

    // Search prompt on the left interior's first line: accent `> `, the query,
    // a block cursor, then a right-aligned dim `{filtered}/{total}` count.
    let inner = layout.left_inner;
    if inner.width > 0 && inner.height > 0 {
        let mut spans = vec![
            Span::styled("> ", Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
            Span::styled(query.to_string(), Style::default().fg(p.fg)),
            Span::styled(GLYPH_CURSOR.to_string(), Style::default().fg(p.accent)),
        ];
        let used = 3 + query.chars().count(); // "> " + query + cursor
        let count = format!("{filtered}/{total}");
        let pad = (inner.width as usize).saturating_sub(used + count.chars().count());
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(count, p.dim_style()));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
        );
    }
    layout
}

/// Draw the windowed filtered rows into the left panel (below the search
/// prompt), register their `MenuItem` targets, and draw the list scrollbar when
/// the filtered set overflows. `line_of(pos)` yields the row text for filtered
/// position `pos`; `style_of(pos)` its style. Renders the dim "no matches"
/// placeholder when the filter kills everything.
fn render_picker_rows(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    layout: &PickerLayout,
    filtered_len: usize,
    index: usize,
    line_of: impl Fn(usize) -> String,
    style_of: impl Fn(usize) -> Style,
) {
    let p = &Palette::default();
    let inner = layout.left_inner;
    if inner.width == 0 || inner.height <= 1 {
        return;
    }
    let rows_area =
        Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: inner.height - 1 };
    if filtered_len == 0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" no matches", p.dim_style()))),
            Rect { x: rows_area.x, y: rows_area.y, width: rows_area.width, height: 1 },
        );
        return;
    }
    let visible = rows_area.height as usize;
    let overflow = filtered_len > visible;
    // Reserve the last column for the scrollbar when the list overflows.
    let row_w = if overflow { rows_area.width.saturating_sub(1) } else { rows_area.width };
    let start = window_start(index, filtered_len, visible);
    for row in 0..rows_area.height {
        let pos = start + row as usize;
        if pos >= filtered_len {
            break;
        }
        let row_rect = Rect { x: rows_area.x, y: rows_area.y + row, width: row_w, height: 1 };
        hit.push(row_rect, HitTarget::MenuItem(pos));
        // Pad to the full row width so the selection bar spans the panel.
        let text = pad_clip(&line_of(pos), row_w as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, style_of(pos)))),
            row_rect,
        );
    }
    if overflow {
        let mut state =
            ScrollbarState::new(filtered_len.saturating_sub(visible)).position(start);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            rows_area,
            &mut state,
        );
    }
}

/// Word-wrap styled source lines to `width` cells, one style per source line
/// (continuation segments and embedded `\n` splits inherit it). Mirrors the
/// preview's actual layout so scroll clamping is exact.
fn wrap_styled(lines: &[(String, Style)], width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (text, style) in lines {
        for part in text.split('\n') {
            if part.is_empty() {
                out.push(Line::from(""));
                continue;
            }
            for dl in wrap_lines(&[part.to_string()], &[LineCtx::Text], width) {
                out.push(Line::from(Span::styled(dl.text, *style)));
            }
        }
    }
    out
}

/// Core of the right preview panel: the two-pass wrap / scroll-clamp / scrollbar
/// / metrics machinery, parameterized on a `wrap` closure that produces the
/// styled display lines for a given text width. Registers the `MenuPreview`
/// wheel target over the interior and returns the scroll metrics the key handler
/// needs. The two passes (same chicken-and-egg fold as detail's
/// `wrap_for_viewport`) exist because whether the scrollbar column is reserved
/// depends on the wrapped count, and the wrap width depends on that reservation —
/// so `wrap` is called once at full width and, on overflow, again one narrower.
fn render_preview_wrapped(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    layout: &PickerLayout,
    wrap: &dyn Fn(usize) -> Vec<Line<'static>>,
    preview_scroll: usize,
) -> PreviewMetrics {
    let inner = layout.right_inner;
    if inner.width == 0 || inner.height == 0 {
        return PreviewMetrics::default();
    }
    hit.push(inner, HitTarget::MenuPreview);
    let mut wrapped = wrap(inner.width as usize);
    let h = inner.height as usize;
    let overflow = wrapped.len() > h;
    let text_w = if overflow && inner.width > 1 {
        wrapped = wrap((inner.width - 1) as usize);
        inner.width - 1
    } else {
        inner.width
    };
    let max_scroll = wrapped.len().saturating_sub(h);
    let scroll = preview_scroll.min(max_scroll);
    let visible: Vec<Line> = wrapped.into_iter().skip(scroll).take(h).collect();
    frame.render_widget(
        Paragraph::new(Text::from(visible)),
        Rect { x: inner.x, y: inner.y, width: text_w, height: inner.height },
    );
    if overflow {
        let mut state = ScrollbarState::new(max_scroll).position(scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            inner,
            &mut state,
        );
    }
    PreviewMetrics { max_scroll }
}

/// Render the right preview panel from plain pre-styled `content` (one style per
/// source line, wrapped via [`wrap_styled`]). Shared with the menu description
/// panel and the run form / def-pick loading placeholders.
pub(crate) fn render_preview(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    layout: &PickerLayout,
    content: &[(String, Style)],
    preview_scroll: usize,
) -> PreviewMetrics {
    render_preview_wrapped(frame, hit, layout, &|w| wrap_styled(content, w), preview_scroll)
}

/// Render the right preview panel with a plain styled `prefix` (e.g. a
/// description + a "Prompt" heading) followed by `markup` rendered through the
/// SAME markdown pipeline as the DETAIL pane's prompt tab — `fence_states` →
/// `wrap_lines` → `style_transcript_line`, so headings, code fences, inline
/// code / bold / URLs and `{{jinja}}` all style identically. Fence state spans
/// the whole body, so it is computed once outside the width closure (it never
/// depends on wrap width). Used by the def-pick and run-form prompt panels.
pub(crate) fn render_preview_markup(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    layout: &PickerLayout,
    prefix: &[(String, Style)],
    markup: &str,
    preview_scroll: usize,
) -> PreviewMetrics {
    let p = Palette::default();
    let lines: Vec<String> = markup.split('\n').map(str::to_string).collect();
    let ctxs = fence_states(&lines);
    render_preview_wrapped(
        frame,
        hit,
        layout,
        &|w| {
            // The plain prefix first, then the markdown body wrapped to the same
            // width; an empty wrapped segment renders as one blank line (mirrors
            // the detail pane's `Line::from(" ")` for empty segments).
            let mut out = wrap_styled(prefix, w);
            for seg in wrap_lines(&lines, &ctxs, w) {
                out.push(style_display_line(&seg, w as u16, &p));
            }
            out
        },
        preview_scroll,
    )
}

/// Human "N{s,m,h,d} ago" for a wall-clock `mtime_ms` relative to `now_ms`
/// (both epoch-milliseconds). Pure — the session picker's row age label.
/// Saturating, so a future/equal timestamp reads "0s ago".
pub fn relative_age(mtime_ms: u64, now_ms: u64) -> String {
    let secs = now_ms.saturating_sub(mtime_ms) / 1000;
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Compact launcher popup (session picker + worktree creation). Row 0 is the
/// synthetic `✦ New session`, row 1 the `⊕ Create Worktree…` entry; a thin rule
/// separates them from the query-filtered loaded sessions, each rendered as
/// `{clipped label} · {relative_age}` — the age is ALWAYS shown (the label is
/// clipped to fit, never the suffix). While `loading`, a dim `loading sessions…`
/// placeholder stands in for the (not-yet-arrived) session rows. Under a thin
/// rule near the bottom sits the highlighted row's description; the last
/// interior line is the `[ Next ] [ Cancel ]` button row (`focus` picks the
/// highlighted button). Accent border + `MODAL_PADDING` (yellow is reserved for
/// destructive confirms). Registers `Modal` over the body, one
/// `MenuItem(view_ix)` per selectable row (`0` = New session, `1` = Create
/// Worktree, `2..` = sessions), and the two `Button` targets; all route through
/// `App::route_session_pick_click`. `now_ms` feeds the relative-age labels.
#[allow(clippy::too_many_arguments)]
pub fn render_session_pick(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    title: &str,
    items: &[SessionChoice],
    loading: bool,
    index: usize,
    query: &str,
    now_ms: u64,
    focus: ButtonKind,
    active_provider: &str,
) {
    let p = Palette::default();
    let filtered = filter_rows(items, query, |s| s.label.clone());
    let area = frame.area();

    // `✦ New session (<active provider>)` — the ACTIVE provider dictates what
    // kind of agent the fresh session spawns, so the label names it live. Falls
    // back to a bare `New session` when no provider is known (old daemon).
    let new_session_label = if active_provider.is_empty() {
        "New session".to_string()
    } else {
        format!("New session ({active_provider})")
    };
    const CREATE_WT: &str = "Create Worktree…";

    // Selectable view rows: New session (0), Create Worktree (1), then the loaded
    // sessions (2..). Row 0/1 are always selectable; the loading placeholder is
    // inert. The highlight clamps into `selectable`.
    let selectable = 2 + if loading { 0 } else { filtered.len() };
    let index = index.min(selectable - 1);

    // Width scales with the terminal (≈47%), floored at DIALOG_WIDTH so it never
    // shrinks below the old size and CAPPED at `SESSION_PICKER_MAX_W` so a very
    // wide monitor doesn't leave a huge dead gap between the prompt and the
    // right-aligned provider/age column. Rows still show more prompt than the
    // old fixed 60, without the dialog spanning the whole screen. It never
    // resizes with its CONTENT — the same dialog whether the filter matches 5
    // rows or none. inner_w = width − border(2) − padding(4).
    const SESSION_PICKER_MAX_W: u16 = 110;
    let want =
        (area.width.saturating_mul(7) / 15).clamp(DIALOG_WIDTH, SESSION_PICKER_MAX_W);
    let width = want.clamp(50.min(area.width.max(1)), area.width.saturating_sub(4).max(1));
    let inner_w = width.saturating_sub(6).max(1) as usize;

    // Bottom description for the highlighted row (dim, under a thin rule): a hint
    // for New session / Create Worktree, else the picked session's id + time.
    let mut desc: Vec<Line<'static>> = Vec::new();
    {
        let body: Vec<Line<'static>> = if index == 0 {
            let hint = if active_provider.is_empty() {
                "Start a fresh session in this worktree.".to_string()
            } else {
                format!("Start a fresh {active_provider} session in this worktree.")
            };
            wrap_styled(&[(hint, p.dim_style())], inner_w)
        } else if index == 1 {
            wrap_styled(
                &[("Create a new worktree, then run a task in it.".into(), p.dim_style())],
                inner_w,
            )
        } else if let Some(&i) = filtered.get(index - 2) {
            let s = &items[i];
            // Session id dim; datetime uses shared timestamp teal (`info`) —
            // same slot as relative ages and pane commit-age columns.
            let tz_offset_s = chrono::Local::now().offset().local_minus_utc();
            let when = absolute_local_label(s.mtime_ms / 1000, tz_offset_s);
            wrap_styled(
                &[
                    (s.session_id.clone(), p.dim_style()),
                    ("  ·  ".into(), p.dim_style()),
                    (when, p.timestamp_style()),
                ],
                inner_w,
            )
        } else {
            Vec::new()
        };
        if !body.is_empty() {
            desc.push(Line::from(Span::styled(
                RULE_CHAR.to_string().repeat(inner_w),
                Style::default().fg(p.border),
            )));
            desc.extend(body);
        }
    }

    // Height: search(1) + rows + desc + blank(1) + button(1), plus the border
    // ring(2) + vertical padding(2), clamped to the frame. Rows = New + Create +
    // rule + (loading placeholder | sessions).
    let session_display = if loading { 1 } else { filtered.len() };
    let display_rows = 2 + 1 + session_display;
    let want_inner = 1 + display_rows + desc.len() + 2;
    let height = ((want_inner + 4) as u16).min(area.height.max(1));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect { x, y, width, height };

    frame.render_widget(Clear, rect);
    hit.push(rect, HitTarget::Modal);

    let block = Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent))
        .padding(MODAL_PADDING);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Search prompt: `> query█` + right-aligned `{filtered}/{total}` session count
    // (the New-session / Create-Worktree rows are not counted).
    let mut spans = vec![
        Span::styled("> ", Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Span::styled(query.to_string(), Style::default().fg(p.fg)),
        Span::styled(GLYPH_CURSOR.to_string(), Style::default().fg(p.accent)),
    ];
    let used = 3 + query.chars().count();
    let count = format!("{}/{}", filtered.len(), items.len());
    let pad = (inner.width as usize).saturating_sub(used + count.chars().count());
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(count, p.dim_style()));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );

    // Button row on the last interior line; description block pinned just above
    // it with one blank separator line between.
    let btn_y = inner.y + inner.height.saturating_sub(1);
    render_button_row(
        frame,
        hit,
        Rect { x: inner.x, y: btn_y, width: inner.width, height: 1 },
        "Next",
        Some(focus),
        p.accent,
    );
    let desc_h = (desc.len() as u16).min(inner.height.saturating_sub(2));
    if desc_h > 0 {
        let desc_area = Rect {
            x: inner.x,
            y: btn_y.saturating_sub(1 + desc_h),
            width: inner.width,
            height: desc_h,
        };
        frame.render_widget(Paragraph::new(Text::from(desc)), desc_area);
    }

    // Rows fill between the search prompt and the desc/blank/button bottom block.
    let bottom_block = desc_h + 2; // desc + blank + button row
    let rows_area = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height.saturating_sub(1 + bottom_block),
    };

    // Display rows: (optional MenuItem view-index, styled spans). Rows 0/1 carry
    // an icon; a dim rule separates them from the sessions; only real rows carry
    // a `MenuItem` (rule + loading placeholder are inert). Session rows use
    // multi-span so a dim provider tag can sit just left of the right-floated age.
    type SpannedLine = (Option<usize>, Vec<(String, Style)>);
    let sel = |vix: usize| if vix == index { p.selection() } else { Style::default().fg(p.fg) };
    let line = |text: String, style: Style| -> Vec<(String, Style)> { vec![(text, style)] };
    let mut lines: Vec<SpannedLine> = vec![
        (Some(0), line(format!("{GLYPH_NEW_SESSION} {new_session_label}"), sel(0))),
        (Some(1), line(format!("{GLYPH_CREATE_WORKTREE} {CREATE_WT}"), sel(1))),
        (None, line(RULE_CHAR.to_string().repeat(inner_w), Style::default().fg(p.border))),
    ];
    if loading {
        lines.push((None, line("loading sessions…".into(), p.dim_style())));
    } else {
        for (n, &i) in filtered.iter().enumerate() {
            let vix = n + 2;
            let base = sel(vix);
            let age = relative_age(items[i].mtime_ms, now_ms);
            // Right column: optional dim `provider` then age, flush right.
            // Example: `PR Resolve Comments              claude  1h ago`
            let age_w = age.chars().count();
            let prov = items[i].provider.as_deref();
            let prov_w = prov.map(|pr| pr.chars().count() + 1 /* trailing space */).unwrap_or(0);
            let right_w = age_w + prov_w;
            let label = clip(&items[i].label, inner_w.saturating_sub(right_w + 2));
            let gap = inner_w.saturating_sub(label.chars().count() + right_w);
            // Provider: top-bar green. Age: shared timestamp teal (`info`).
            // Selected rows keep concept colors on the selection bg.
            let selected = vix == index;
            let mut prov_style = p.provider_style(prov.unwrap_or(""));
            let mut age_style = p.timestamp_style();
            if selected {
                prov_style = prov_style.bg(p.selection_bg);
                age_style = age_style.bg(p.selection_bg);
            }
            let mut spans = vec![
                (label, base),
                (" ".repeat(gap), base),
            ];
            if let Some(pr) = prov {
                spans.push((format!("{pr} "), prov_style));
            }
            spans.push((age, age_style));
            lines.push((Some(vix), spans));
        }
    }
    for (row, (menu_ix, spans)) in lines.iter().enumerate() {
        if row as u16 >= rows_area.height {
            break;
        }
        let row_rect =
            Rect { x: rows_area.x, y: rows_area.y + row as u16, width: rows_area.width, height: 1 };
        if let Some(vix) = menu_ix {
            hit.push(row_rect, HitTarget::MenuItem(*vix));
        }
        // Pad with the row base style (not the last concept color — age/provider
        // would leak teal/green into the trailing bar fill).
        let row_w = rows_area.width as usize;
        let used: usize = spans.iter().map(|(t, _)| t.chars().count()).sum();
        let mut out: Vec<Span> = spans
            .iter()
            .map(|(t, style)| Span::styled(t.clone(), *style))
            .collect();
        if used < row_w {
            let selected = menu_ix.is_some_and(|vix| vix == index);
            let pad_style = if selected { p.selection() } else { Style::default() };
            out.push(Span::styled(" ".repeat(row_w - used), pad_style));
        } else if used > row_w {
            // Defensive: collapse to a single pad_clip when spans overshoot
            // (shouldn't happen — clip budget is computed against inner_w).
            let flat: String = spans.iter().map(|(t, _)| t.as_str()).collect();
            let style = spans.first().map(|(_, s)| *s).unwrap_or_default();
            out = vec![Span::styled(pad_clip(&flat, row_w), style)];
        }
        frame.render_widget(Paragraph::new(Line::from(out)), row_rect);
    }
}

/// Big lazyvim-style task menu / def picker. Left panel = search prompt +
/// FILTERED def rows (name + arg summary; selected row a
/// full-width inverse bar); right panel = the
/// highlighted def's description (dim "no description" when unset), a blank
/// line, a bold "Prompt" heading, then the full definition's prompt —
/// markdown-styled like the DETAIL pane's prompt tab (dim "loading prompt…"
/// until `full` arrives), scrollable via the mouse wheel.
pub fn render_def_pick(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    title: &str,
    defs: &[DefinitionSummary],
    full: Option<&TaskDefinition>,
    state: PickerState,
) -> PreviewMetrics {
    let p = Palette::default();
    let PickerState { index, query, preview_scroll } = state;
    let filtered = filter_rows(defs, query, |d| d.name.clone());
    let selected = filtered.get(index).and_then(|&i| defs.get(i));
    let layout = render_picker_chrome(
        frame,
        hit,
        title,
        selected.map(|d| d.name.as_str()),
        query,
        filtered.len(),
        defs.len(),
    );
    render_picker_rows(
        frame,
        hit,
        &layout,
        filtered.len(),
        index,
        |pos| {
            let def = &defs[filtered[pos]];
            let mut text = format!(" {}", def.name);
            if !def.args.is_empty() {
                text.push_str(&format!(" ({})", arg_summary(&def.args)));
            }
            // No global-scope `(g)` marker — it carried no meaning for the user
            // (a def is a def; where it is defined doesn't change how it runs).
            text
        },
        |pos| if pos == index { p.selection() } else { Style::default().fg(p.fg) },
    );
    // Bold Title Case "Description"/"Prompt" headings, in `p.heading` (pink) —
    // the same color the markdown pipeline below them uses for `## heading`
    // lines in the prompt body, so both heading kinds read as one system. Their
    // bodies are the plain PREFIX; the def's prompt below it is markdown-styled
    // (matching the DETAIL pane). Until `full` arrives the prompt is a plain
    // "loading" placeholder in the prefix.
    let heading = Style::default().fg(p.heading).add_modifier(Modifier::BOLD);
    let mut prefix: Vec<(String, Style)> = Vec::new();
    let mut prompt: Option<&str> = None;
    if let Some(def) = selected {
        prefix.push(("Description".into(), heading));
        match &def.description {
            Some(desc) if !desc.is_empty() => {
                prefix.push((desc.clone(), Style::default().fg(p.fg)))
            }
            _ => prefix.push(("no description".into(), p.dim_style())),
        }
        prefix.push((String::new(), Style::default()));
        prefix.push(("Prompt".into(), heading));
        match full {
            Some(td) => prompt = Some(td.prompt.as_str()),
            None => prefix.push(("loading prompt…".into(), p.dim_style())),
        }
    }
    match prompt {
        Some(md) => render_preview_markup(frame, hit, &layout, &prefix, md, preview_scroll),
        None => render_preview(frame, hit, &layout, &prefix, preview_scroll),
    }
}

#[cfg(test)]
mod menu_view_tests {
    use super::*;
    use crate::hit::{HitMap, HitTarget};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn def_pick_prompt_renders_markdown_styled() {
        use crate::ipc::types::{DefinitionSummary, TaskDefinition};
        let defs = vec![DefinitionSummary { name: "build".into(), ..Default::default() }];
        // A `## heading` + a fenced code block: the markdown pipeline turns the
        // fence into a labeled rule (── bash ──), never the literal ``` delimiter.
        let full = TaskDefinition {
            name: "build".into(),
            prompt: "## Heading\n\n```bash\necho hi\n```".into(),
            ..Default::default()
        };
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| {
            render_def_pick(
                f,
                &mut hit,
                "tasks",
                &defs,
                Some(&full),
                PickerState { index: 0, query: "", preview_scroll: 0 },
            );
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..24 {
            for x in 0..100 {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        assert!(s.contains("──"), "fenced code block styled as a rule: {s}");
        assert!(s.contains("bash"), "fence carries its language label");
        assert!(!s.contains("```"), "raw fence delimiters replaced by the rule");
        assert!(s.contains("Prompt"), "the bold Prompt-heading prefix still renders");
    }

    #[test]
    fn relative_age_formats_seconds_minutes_hours_days() {
        // Saturating (future/equal → 0s), and the m/h/d thresholds from the brief.
        assert_eq!(relative_age(1_000, 1_000), "0s ago");
        assert_eq!(relative_age(1_000, 500), "0s ago"); // future clamps
        assert_eq!(relative_age(0, 5_000), "5s ago");
        assert_eq!(relative_age(0, 59_000), "59s ago");
        assert_eq!(relative_age(0, 180_000), "3m ago");
        assert_eq!(relative_age(0, 2 * 3_600_000), "2h ago");
        assert_eq!(relative_age(0, 4 * 86_400_000), "4d ago");
    }

    fn draw_session_pick(
        cols: u16,
        rows: u16,
        loading: bool,
        index: usize,
        query: &str,
    ) -> (String, HitMap) {
        let items = vec![
            SessionChoice { session_id: "sess-1".into(), label: "Fix the parser".into(), mtime_ms: 0, model: None, provider: None },
            SessionChoice { session_id: "sess-2".into(), label: "Redesign TUI".into(), mtime_ms: 0, model: None, provider: None },
        ];
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        let mut hit = HitMap::default();
        // now_ms far ahead so ages render as days.
        term.draw(|f| {
            render_session_pick(
                f,
                &mut hit,
                "wt-a",
                &items,
                loading,
                index,
                query,
                5 * 86_400_000,
                ButtonKind::Confirm,
                "",
            );
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..rows {
            for x in 0..cols {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        (s, hit)
    }

    /// Width (in columns) of the bordered popup = span between the top border's
    /// corners. Counts chars, not bytes (box-drawing glyphs are multi-byte).
    fn popup_width(s: &str) -> usize {
        let top = s.lines().find(|l| l.contains('╭')).expect("top border");
        let cols: Vec<char> = top.chars().collect();
        let l = cols.iter().position(|&c| c == '╭').unwrap();
        let r = cols.iter().rposition(|&c| c == '╮').unwrap();
        r - l + 1
    }

    #[test]
    fn session_pick_width_scales_with_terminal_not_content() {
        // The launcher width scales with the TERMINAL (≈47%) so rows show more
        // of their prompt — but it is NOT content-driven: it must not resize as
        // the filter narrows or labels change length.
        let (s, _) = draw_session_pick(200, 24, false, 1, "");
        let w = popup_width(&s);
        assert_eq!(w, 93, "≈47% of the 200-col terminal (200*7/15)");
        // Same width when the filter matches nothing (content-independent).
        let (s2, _) = draw_session_pick(200, 24, false, 0, "zzz");
        assert_eq!(popup_width(&s2), w);
        // A very wide terminal is CAPPED so the dialog never spans the monitor.
        let (s_cap, _) = draw_session_pick(400, 40, false, 1, "");
        assert_eq!(popup_width(&s_cap), 110, "capped at SESSION_PICKER_MAX_W");
        // Narrow terminals keep the old floor (never smaller than before).
        let (s3, _) = draw_session_pick(80, 24, false, 1, "");
        assert_eq!(popup_width(&s3), 60, "floored at DIALOG_WIDTH on an 80-col terminal");
    }

    #[test]
    fn session_pick_new_session_names_the_active_provider() {
        // With an active provider, the `✦ New session` row and its hint name it
        // (the active provider dictates what the fresh session spawns).
        let items = vec![SessionChoice {
            session_id: "sess-1".into(),
            label: "Fix the parser".into(),
            mtime_ms: 0,
            model: None,
            provider: None,
        }];
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| {
            render_session_pick(f, &mut hit, "wt", &items, false, 0, "", 5_000, ButtonKind::Confirm, "grok");
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..24 {
            for x in 0..120 {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        assert!(s.contains("New session (grok)"), "row 0 names the active provider: {s}");
        assert!(
            s.contains("Start a fresh grok session in this worktree."),
            "the hint names the active provider: {s}"
        );
    }

    #[test]
    fn session_pick_floats_age_to_the_right_edge() {
        // The relative age is right-aligned as its own column, not trailing the
        // label with a "· " separator.
        let (s, _) = draw_session_pick(120, 24, false, 2, "");
        let row = s.lines().find(|l| l.contains("Fix the parser")).expect("session row");
        assert!(row.contains("5d ago"));
        assert!(!row.contains('·'), "no '·' separator — the age floats as its own column: {row:?}");
        let label_end = row.find("Fix the parser").unwrap() + "Fix the parser".len();
        let age_start = row.rfind("5d ago").unwrap();
        assert!(age_start - label_end >= 5, "wide gap floats the age right: {row:?}");
    }

    #[test]
    fn launcher_renders_entries_sessions_and_hit_targets() {
        // index 2 = the first session (New=0, Create Worktree=1, sessions 2..).
        let (s, hit) = draw_session_pick(80, 20, false, 2, "");
        assert!(s.contains(" wt-a "), "popup titled with the worktree display name");
        assert!(s.contains("New session"), "row 0 is the synthetic New session");
        assert!(s.contains("Create Worktree"), "row 1 is Create Worktree");
        assert!(s.contains("Fix the parser"), "loaded session label renders");
        assert!(s.contains("Redesign TUI"));
        assert!(s.contains("ago"), "session rows carry a relative age");
        assert!(s.contains("[ Next ]") && s.contains("[ Cancel ]"), "button row present");
        assert!(!s.contains("type to filter"), "the MENU_HINT legend is removed");
        // Highlighted session row (view index 2 = sess-1) shows its id below.
        assert!(s.contains("sess-1"), "highlighted session's id in the description");
        // New(0) + Create(1) + two session rows (2,3) each register a MenuItem.
        let (mut m0, mut m1, mut m2, mut m3, mut modal) = (false, false, false, false, false);
        for y in 0..20 {
            for x in 0..80 {
                match hit.hit(x, y) {
                    Some(HitTarget::MenuItem(0)) => m0 = true,
                    Some(HitTarget::MenuItem(1)) => m1 = true,
                    Some(HitTarget::MenuItem(2)) => m2 = true,
                    Some(HitTarget::MenuItem(3)) => m3 = true,
                    Some(HitTarget::Modal) => modal = true,
                    _ => {}
                }
            }
        }
        assert!(m0 && m1 && m2 && m3, "New + Create + both session rows register MenuItems");
        assert!(modal, "popup body registers a Modal region");
    }

    #[test]
    fn launcher_loading_keeps_entries_selectable_placeholder_inert() {
        let (s, hit) = draw_session_pick(80, 20, true, 0, "");
        assert!(s.contains("New session"));
        assert!(s.contains("Create Worktree"));
        assert!(s.contains("loading sessions"), "loading placeholder row renders");
        // While loading, New(0) + Create(1) stay selectable; the placeholder is not.
        let (mut m0, mut m1, mut m2) = (false, false, false);
        for y in 0..20 {
            for x in 0..80 {
                match hit.hit(x, y) {
                    Some(HitTarget::MenuItem(0)) => m0 = true,
                    Some(HitTarget::MenuItem(1)) => m1 = true,
                    Some(HitTarget::MenuItem(2)) => m2 = true,
                    _ => {}
                }
            }
        }
        assert!(m0 && m1, "New session + Create Worktree selectable while loading");
        assert!(!m2, "the loading placeholder is not a selectable MenuItem");
    }

    #[test]
    fn launcher_filter_counts_sessions_only() {
        // Filtering applies to loaded labels; the two entry rows stay visible. The
        // right-aligned count is filtered/total over sessions (not the view rows).
        let (s, _hit) = draw_session_pick(80, 20, false, 2, "redesign");
        assert!(s.contains("New session"), "New session stays visible under a filter");
        assert!(s.contains("Create Worktree"), "Create Worktree stays visible under a filter");
        assert!(s.contains("Redesign TUI"));
        assert!(!s.contains("Fix the parser"), "non-matching session filtered out");
        assert!(s.contains("1/2"), "count is filtered/total sessions");
    }

    #[test]
    fn session_row_age_survives_long_label() {
        // A very long label must not truncate the "· Ns ago" suffix — the label is
        // clipped with an ellipsis, the age always renders.
        let items = vec![SessionChoice {
            session_id: "s1".into(),
            label: "x".repeat(200),
            mtime_ms: 0,
            model: None,
            provider: None,
        }];
        let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| {
            render_session_pick(f, &mut hit, "wt", &items, false, 2, "", 5_000, ButtonKind::Confirm, "");
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..20 {
            for x in 0..60 {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        assert!(s.contains("ago"), "age suffix must always render");
        assert!(s.contains('…'), "long label is clipped with an ellipsis");
    }

    #[test]
    fn session_row_shows_dim_provider_before_age() {
        // Row layout: label left; dim provider (when Some) then age right-floated.
        // Example: `# PR Resolve Comments              claude  1h ago`
        let items = vec![
            SessionChoice {
                session_id: "s1".into(),
                label: "PR Resolve Comments".into(),
                mtime_ms: 0,
                model: None,
                provider: Some("claude".into()),
            },
            SessionChoice {
                session_id: "s2".into(),
                label: "No provider session".into(),
                mtime_ms: 0,
                model: None,
                provider: None,
            },
        ];
        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| {
            render_session_pick(
                f,
                &mut hit,
                "wt",
                &items,
                false,
                2,
                "",
                3_600_000,
                ButtonKind::Confirm,
                "",
            );
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..20 {
            for x in 0..80 {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        let with = s.lines().find(|l| l.contains("PR Resolve Comments")).expect("provider row");
        let without = s.lines().find(|l| l.contains("No provider session")).expect("no-provider row");
        assert!(with.contains("claude"), "provider tag renders: {with:?}");
        assert!(with.contains("ago"), "age still renders: {with:?}");
        let prov_i = with.find("claude").unwrap();
        let age_i = with.rfind("ago").unwrap();
        assert!(prov_i < age_i, "provider sits before age: {with:?}");
        // No stray provider token when None — only the known label/age tokens.
        assert!(!without.contains("claude"), "None provider leaves no tag: {without:?}");
        assert!(without.contains("ago"), "age still renders without provider: {without:?}");
    }

    #[test]
    fn launcher_snapshot() {
        let (s, _hit) = draw_session_pick(60, 18, false, 0, "");
        insta::assert_snapshot!("launcher_open", s);
    }
}
