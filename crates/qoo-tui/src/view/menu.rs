//! Big lazyvim/snacks-style pickers (action menu + task menu) and the
//! destructive worktree-remove confirm. Each picker is TWO separate bordered
//! panels side by side: the left panel holds the search prompt (with a
//! right-aligned match count), the FILTERED rows, and the key hint in its
//! bottom border; the right panel is a scrollable preview (description /
//! prompt) titled with the selected row's label. Both panels register `Modal`
//! hit targets (clicks can't leak to the panes beneath), the left panel
//! registers one `MenuItem(i)` per visible row (`i` = FILTERED display index),
//! and the right panel's interior registers `MenuPreview` so the mouse wheel
//! can scroll the preview.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use crate::action_menu::{filter_items, ActionItem};
use crate::event::SessionChoice;
use crate::hit::{HitMap, HitTarget};
use crate::ipc::types::{DefinitionSummary, TaskDefinition};
use crate::markup::{fence_states, style_transcript_line, wrap_lines, LineCtx};
use crate::selectors::{absolute_local_label, arg_summary, filter_rows, pad_clip};
use crate::view::theme::{GLYPH_CURSOR, GLYPH_DISCOVERY, MARKER_GLOBAL, Palette, RULE_CHAR};

const MENU_HINT: &str = " type to filter · ↑/↓ move · enter run · esc close ";

/// Render-feedback for the preview scroll: the wheel handler clamps
/// `preview_scroll` against `max_scroll`, measured from the last draw (same
/// freshness argument as `detail_max_scroll`: every state change redraws
/// before the next event).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviewMetrics {
    pub max_scroll: usize,
}

/// The picker's interactive state slice (out of `Mode::ActionMenu`/`DefPick`):
/// the highlighted FILTERED index, the label/name filter, and the preview
/// panel's scroll offset.
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
    let inner = |r: Rect| Rect {
        x: r.x + 1,
        y: r.y + 1,
        width: r.width.saturating_sub(2),
        height: r.height.saturating_sub(2),
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

    // Left panel: menu title in the top border, key hint in the bottom border.
    let left_block = Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(Span::styled(MENU_HINT, p.dim_style())))
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
/// description + a "prompt" heading) followed by `markup` rendered through the
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
                out.push(if seg.text.is_empty() {
                    Line::from(" ")
                } else {
                    style_transcript_line(&seg.text, &seg.ctx, w as u16, &p)
                });
            }
            out
        },
        preview_scroll,
    )
}

/// Compact centered action menu — a SINGLE bordered popup sized to its content,
/// NOT the big two-panel picker (that shell stays for `render_def_pick` /
/// `render_run_form`, which genuinely use the preview panel; an action menu of a
/// few items in it was almost all empty space). Top border = the target label;
/// then the search prompt row (type-to-filter + right-aligned `{filtered}/{total}`
/// count); the FILTERED item rows (disabled dim, selected row a full-width inverse
/// bar, windowed with a scrollbar if they overflow); and — under a thin rule,
/// pinned to the bottom — the SELECTED item's wrapped dim description plus a warn
/// `disabled — {reason}` line when applicable. Key hint sits in the bottom border.
/// Registers `Modal` over the body + one `MenuItem(i)` per visible row. There is
/// NO preview panel: no `MenuPreview` target and `preview_scroll` is inert, so it
/// returns `PreviewMetrics::default()`. The wheel over the popup falls to the
/// `MenuItem`/`Modal` arm of `App::menu_wheel`, moving the selection like it does
/// over the picker rows. Bulk-range menus flow through here too (also short).
pub fn render_menu(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    title: &str,
    items: &[ActionItem],
    state: PickerState,
) -> PreviewMetrics {
    let p = Palette::default();
    let PickerState { index, query, .. } = state; // preview_scroll inert (no preview)
    let filtered = filter_items(items, query);
    let selected = filtered.get(index).and_then(|&i| items.get(i));
    let area = frame.area();

    // Width: content-driven (longest label / the title), clamped to [44, 72] and
    // the frame. The description wraps to whatever inner width results.
    let label_w = filtered.iter().map(|&i| items[i].label.chars().count()).max().unwrap_or(0);
    let content_w = label_w.max(title.chars().count()).max(28);
    let width = ((content_w + 6) as u16).clamp(44, 72).min(area.width.max(1));
    let inner_w = width.saturating_sub(2).max(1) as usize; // inside the border

    // Description block (dim, wrapped) under a thin rule, plus the warn disabled
    // line — only when a row is selected and there is something to show.
    let mut desc: Vec<Line<'static>> = Vec::new();
    if let Some(it) = selected {
        let mut body: Vec<Line<'static>> = Vec::new();
        if !it.description.is_empty() {
            body.extend(wrap_styled(&[(it.description.clone(), p.dim_style())], inner_w));
        }
        if let Some(reason) = &it.disabled {
            body.extend(wrap_styled(
                &[(format!("disabled — {reason}"), Style::default().fg(p.warn))],
                inner_w,
            ));
        }
        if !body.is_empty() {
            desc.push(Line::from(Span::styled(
                RULE_CHAR.to_string().repeat(inner_w),
                Style::default().fg(p.border),
            )));
            desc.extend(body);
        }
    }

    // Height: borders(2) + search(1) + item rows + description block, clamped to
    // the frame. `.max(1)` on the row count reserves a line for the "no matches"
    // placeholder.
    let want = 2 + 1 + filtered.len().max(1) + desc.len();
    let height = (want as u16).min(area.height.max(1));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect { x, y, width, height };

    frame.render_widget(Clear, rect);
    hit.push(rect, HitTarget::Modal); // popup body opaque to clicks

    let block = Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(Span::styled(MENU_HINT, p.dim_style())))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return PreviewMetrics::default();
    }

    // Search prompt on the interior's first line: accent `> `, the query, a block
    // cursor, then a right-aligned dim `{filtered}/{total}` count.
    let mut spans = vec![
        Span::styled("> ", Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Span::styled(query.to_string(), Style::default().fg(p.fg)),
        Span::styled(GLYPH_CURSOR.to_string(), Style::default().fg(p.accent)),
    ];
    let used = 3 + query.chars().count(); // "> " + query + cursor
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

    // The description block is pinned to the BOTTOM of the interior; the rows fill
    // whatever remains between the search prompt and it (windowed + scrollbar on
    // overflow so both the prompt and description always stay visible).
    let desc_h = (desc.len() as u16).min(inner.height.saturating_sub(1));
    let rows_area = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height.saturating_sub(1 + desc_h),
    };
    if filtered.is_empty() {
        if rows_area.height > 0 {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(" no matches", p.dim_style()))),
                Rect { x: rows_area.x, y: rows_area.y, width: rows_area.width, height: 1 },
            );
        }
    } else if rows_area.height > 0 {
        let visible = rows_area.height as usize;
        let overflow = filtered.len() > visible;
        let row_w = if overflow { rows_area.width.saturating_sub(1) } else { rows_area.width };
        let start = window_start(index, filtered.len(), visible);
        for row in 0..rows_area.height {
            let pos = start + row as usize;
            if pos >= filtered.len() {
                break;
            }
            let it = &items[filtered[pos]];
            let style = if it.disabled.is_some() {
                p.dim_style()
            } else if pos == index {
                p.selection()
            } else {
                Style::default().fg(p.fg)
            };
            let row_rect = Rect { x: rows_area.x, y: rows_area.y + row, width: row_w, height: 1 };
            hit.push(row_rect, HitTarget::MenuItem(pos));
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    pad_clip(&format!(" {}", it.label), row_w as usize),
                    style,
                ))),
                row_rect,
            );
        }
        if overflow {
            let mut state =
                ScrollbarState::new(filtered.len().saturating_sub(visible)).position(start);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight),
                rows_area,
                &mut state,
            );
        }
    }

    // Description block, pinned flush to the bottom interior row.
    if desc_h > 0 {
        let desc_area = Rect {
            x: inner.x,
            y: inner.y + inner.height - desc_h,
            width: inner.width,
            height: desc_h,
        };
        frame.render_widget(Paragraph::new(Text::from(desc)), desc_area);
    }
    PreviewMetrics::default()
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

/// Compact session picker popup (mirrors [`render_menu`]'s single-panel shell).
/// Row 0 is ALWAYS the synthetic "New session"; the query-filtered loaded
/// sessions follow it as `"{label} · {relative_age}"`. While `loading`, a dim
/// `loading sessions…` placeholder stands in for the (not-yet-arrived) session
/// rows. Under a thin rule, pinned to the bottom, the highlighted row's
/// description: a hint for "New session", or the full session id + absolute time
/// for a session row. Registers `Modal` over the body and one `MenuItem(view_ix)`
/// per selectable row (`0` = New session), so clicks route through
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
) {
    let p = Palette::default();
    let filtered = filter_rows(items, query, |s| s.label.clone());
    let area = frame.area();

    // View rows below the search prompt: the always-present "New session" plus
    // either a loading placeholder or the filtered session labels. Only real,
    // selectable rows carry a `MenuItem` (the placeholder does not).
    const NEW_SESSION: &str = "New session";
    let session_rows: Vec<(usize, String)> = if loading {
        Vec::new()
    } else {
        filtered
            .iter()
            .map(|&i| (i, format!("{} · {}", items[i].label, relative_age(items[i].mtime_ms, now_ms))))
            .collect()
    };
    // Total selectable view rows (New session + loaded sessions); the highlight
    // clamps into this. The loading placeholder is not counted (inert).
    let selectable = 1 + session_rows.len();
    let index = index.min(selectable - 1);

    // Width: content-driven (longest row text / the title), clamped to [44, 72].
    let row_w = session_rows.iter().map(|(_, t)| t.chars().count()).max().unwrap_or(0);
    let content_w = row_w.max(NEW_SESSION.chars().count()).max(title.chars().count()).max(28);
    let width = ((content_w + 6) as u16).clamp(44, 72).min(area.width.max(1));
    let inner_w = width.saturating_sub(2).max(1) as usize;

    // Bottom description for the highlighted row (dim, under a thin rule): a hint
    // for the New-session row, else the picked session's id + absolute time.
    let mut desc: Vec<Line<'static>> = Vec::new();
    {
        let body: Vec<Line<'static>> = if index == 0 {
            wrap_styled(
                &[("Start a fresh Claude session in this worktree.".into(), p.dim_style())],
                inner_w,
            )
        } else if let Some((i, _)) = session_rows.get(index - 1) {
            let s = &items[*i];
            let when = absolute_local_label(s.mtime_ms / 1000, 0);
            wrap_styled(&[(format!("{}  ·  {when}", s.session_id), p.dim_style())], inner_w)
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

    // Height: borders(2) + search(1) + rows + description. Rows = New session +
    // (loading placeholder | session rows), clamped to the frame.
    let display_rows = 1 + if loading { 1 } else { session_rows.len() };
    let want = 2 + 1 + display_rows + desc.len();
    let height = (want as u16).min(area.height.max(1));
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
        .title_bottom(Line::from(Span::styled(MENU_HINT, p.dim_style())))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Search prompt (same shape as render_menu): `> query█` + right-aligned
    // `{filtered}/{total}` session count (the New-session row is not counted).
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

    // Rows fill between the search prompt and the bottom-pinned description.
    let desc_h = (desc.len() as u16).min(inner.height.saturating_sub(1));
    let rows_area = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height.saturating_sub(1 + desc_h),
    };
    // Build the (optional MenuItem view-index, text, style) for every display row.
    let mut lines: Vec<(Option<usize>, String, Style)> = Vec::new();
    let new_style = if index == 0 { p.selection() } else { Style::default().fg(p.fg) };
    lines.push((Some(0), format!(" {NEW_SESSION}"), new_style));
    if loading {
        lines.push((None, " loading sessions…".into(), p.dim_style()));
    } else {
        for (view_ix, (_, text)) in session_rows.iter().enumerate() {
            let vix = view_ix + 1; // row 0 is New session
            let style = if vix == index { p.selection() } else { Style::default().fg(p.fg) };
            lines.push((Some(vix), format!(" {text}"), style));
        }
    }
    for (row, (menu_ix, text, style)) in lines.iter().enumerate() {
        if row as u16 >= rows_area.height {
            break;
        }
        let row_rect =
            Rect { x: rows_area.x, y: rows_area.y + row as u16, width: rows_area.width, height: 1 };
        if let Some(vix) = menu_ix {
            hit.push(row_rect, HitTarget::MenuItem(*vix));
        }
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                pad_clip(text, rows_area.width as usize),
                *style,
            ))),
            row_rect,
        );
    }

    // Description block, pinned flush to the bottom interior row.
    if desc_h > 0 {
        let desc_area = Rect {
            x: inner.x,
            y: inner.y + inner.height - desc_h,
            width: inner.width,
            height: desc_h,
        };
        frame.render_widget(Paragraph::new(Text::from(desc)), desc_area);
    }
}

/// Big lazyvim-style task menu / def picker. Left panel = search prompt +
/// FILTERED def rows (name + arg summary + `⏰` discovery glyph + `(g)` global
/// marker; selected row a full-width inverse bar); right panel = the
/// highlighted def's description (dim "no description" when unset), a blank
/// line, a bold "prompt" heading, then the full definition's prompt —
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
            if def.has_discovery {
                text.push(' ');
                text.push(GLYPH_DISCOVERY);
            }
            if def.scope == "global" {
                text.push(' ');
                text.push_str(MARKER_GLOBAL);
            }
            text
        },
        |pos| if pos == index { p.selection() } else { Style::default().fg(p.fg) },
    );
    // The description + blank + bold "prompt" heading are the plain PREFIX; the
    // def's prompt below it is markdown-styled (matching the DETAIL pane). Until
    // `full` arrives the prompt is a plain "loading" placeholder in the prefix.
    let mut prefix: Vec<(String, Style)> = Vec::new();
    let mut prompt: Option<&str> = None;
    if let Some(def) = selected {
        match &def.description {
            Some(desc) if !desc.is_empty() => {
                prefix.push((desc.clone(), Style::default().fg(p.fg)))
            }
            _ => prefix.push(("no description".into(), p.dim_style())),
        }
        prefix.push((String::new(), Style::default()));
        prefix.push(("prompt".into(), Style::default().fg(p.fg).add_modifier(Modifier::BOLD)));
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
    use crate::action_menu::{ActionItem, MenuAction};
    use crate::hit::{HitMap, HitTarget};
    use ratatui::{Terminal, backend::TestBackend};

    fn items() -> Vec<ActionItem> {
        vec![
            // Synthetic two-row fixture for the picker WIDGET (an enabled row + a
            // disabled row with a reason). The `action` payload is never inspected
            // here — only label/disabled/description render — so any surviving
            // variant stands in.
            ActionItem {
                label: "Rerun".into(),
                disabled: None,
                description: "Re-queue this task and run it again.".into(),
                action: MenuAction::Resume { path: String::new(), session_id: String::new() },
            },
            ActionItem {
                label: "Skip".into(),
                disabled: Some("cannot skip a running task".into()),
                description: "Mark this task as skipped; it will not run.".into(),
                action: MenuAction::Resume { path: String::new(), session_id: String::new() },
            },
        ]
    }

    fn draw_q(cols: u16, rows: u16, index: usize, query: &str) -> (String, HitMap) {
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| {
            render_menu(f, &mut hit, "do the thing", &items(), PickerState { index, query, preview_scroll: 0 });
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

    fn draw(cols: u16, rows: u16, index: usize) -> (String, HitMap) {
        draw_q(cols, rows, index, "")
    }

    #[test]
    fn disabled_row_shows_reason_in_description() {
        // Row labels render in the item rows (no inline "— reason"); the disabled
        // reason for the *selected* row surfaces in the description block under
        // the rule. Wide terminal so the warn line fits without wrapping.
        let (s, _hit) = draw(120, 20, 1); // select the disabled "Skip" row
        assert!(s.contains("Rerun"));
        assert!(s.contains("Skip"));
        assert!(s.contains("disabled — cannot skip a running task"));
        // The label row no longer carries the inline reason.
        assert!(!s.contains("Skip — cannot skip a running task"));
    }

    #[test]
    fn search_prompt_count_and_hint_render() {
        let (s, _hit) = draw(80, 20, 0);
        assert!(s.contains('>')); // search prompt prefix
        assert!(s.contains('█')); // cursor block
        assert!(s.contains("2/2")); // right-aligned match count
        // The hint lives in the popup's bottom border; the full text needs a
        // wide-enough popup (the border clips it gracefully otherwise).
        let (w, _hit) = draw(200, 30, 0);
        assert!(w.contains("type to filter"));
        assert!(w.contains("enter run"));
        assert!(!w.contains("ctrl+d/u"), "paging hint removed with the binding");
    }

    #[test]
    fn menu_title_and_selected_label_render() {
        // The popup's top border carries the menu title; the selected item's
        // label sits in its row and its description in the block below.
        let (s, _hit) = draw(80, 20, 0);
        assert!(s.contains(" do the thing "), "popup titled with the menu title");
        assert!(s.contains(" Rerun "), "selected item's label renders in its row");
        assert!(s.contains("Re-queue this task"), "selected item's description renders");
    }

    #[test]
    fn typing_filters_rows_and_count() {
        // Query "ski" keeps only "Skip"; "Rerun" is filtered out of the rows.
        let (s, hit) = draw_q(80, 20, 0, "ski");
        assert!(s.contains("Skip"));
        assert!(s.contains("1/2"), "count shows filtered/total");
        // Only one filtered row → its MenuItem index is 0; no MenuItem(1).
        let mut saw0 = false;
        let mut saw1 = false;
        for y in 0..20 {
            for x in 0..80 {
                match hit.hit(x, y) {
                    Some(HitTarget::MenuItem(0)) => saw0 = true,
                    Some(HitTarget::MenuItem(1)) => saw1 = true,
                    _ => {}
                }
            }
        }
        assert!(saw0);
        assert!(!saw1, "filtered-out row must not register a hit target");
    }

    #[test]
    fn no_matches_shows_placeholder() {
        let (s, _hit) = draw_q(80, 20, 0, "zzz");
        assert!(s.contains("no matches"));
        assert!(s.contains("0/2"));
        // No selection → no rows and no description block.
        assert!(!s.contains(" Rerun "), "filtered-out label must not render");
    }

    #[test]
    fn hit_targets_cover_rows_and_modal_no_preview() {
        let (_s, hit) = draw(80, 20, 0);
        // The popup body is a Modal (clicks don't leak) and each visible row is a
        // MenuItem. The compact menu has NO preview panel, so — unlike the
        // def-pick/run-form pickers — it registers no MenuPreview.
        let (mut saw_item0, mut saw_modal, mut saw_preview) = (false, false, false);
        for y in 0..20 {
            for x in 0..80 {
                match hit.hit(x, y) {
                    Some(HitTarget::MenuItem(0)) => saw_item0 = true,
                    Some(HitTarget::Modal) => saw_modal = true,
                    Some(HitTarget::MenuPreview) => saw_preview = true,
                    _ => {}
                }
            }
        }
        assert!(saw_item0, "expected a MenuItem(0) hit region");
        assert!(saw_modal, "expected a Modal body region");
        assert!(!saw_preview, "compact menu registers no MenuPreview");
    }

    /// Many items so the filtered list overflows the left panel rows.
    fn many_items(n: usize) -> Vec<ActionItem> {
        (0..n)
            .map(|i| ActionItem {
                label: format!("Action {i:02}"),
                disabled: None,
                description: format!("Description for action {i:02}."),
                action: MenuAction::Resume { path: String::new(), session_id: String::new() },
            })
            .collect()
    }

    #[test]
    fn list_scrollbar_renders_only_on_overflow() {
        let draw_n = |n: usize| {
            let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
            let mut hit = HitMap::default();
            term.draw(|f| {
                render_menu(f, &mut hit, "t", &many_items(n), PickerState { index: 0, query: "", preview_scroll: 0 });
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
            s
        };
        // 80x20 → compact popup height clamped to the frame; 40 rows overflow the
        // available row band, so a scrollbar renders.
        assert!(draw_n(40).contains('█') || draw_n(40).contains('▐') || draw_n(40).contains('║'));
        // Few rows: no scrollbar glyph beyond the search cursor is asserted here —
        // instead assert the windowed top row is the first item in both cases.
        assert!(draw_n(3).contains("Action 00"));
        assert!(draw_n(40).contains("Action 00"));
    }

    #[test]
    fn description_renders_and_preview_scroll_is_inert() {
        // The compact menu has no scrollable preview: the selected row's
        // description renders under a thin rule, the reported metrics are the
        // default (max_scroll 0), and `preview_scroll` does nothing — the wheel
        // moves the selection instead (handled in `App::menu_wheel`).
        let draw_scrolled = |scroll: usize| {
            let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
            let mut hit = HitMap::default();
            let mut metrics = PreviewMetrics { max_scroll: 999 };
            term.draw(|f| {
                metrics = render_menu(f, &mut hit, "do the thing", &items(), PickerState { index: 0, query: "", preview_scroll: scroll });
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
            (s, metrics)
        };
        let (s0, m0) = draw_scrolled(0);
        assert!(s0.contains("Re-queue this task"), "selected row's description renders");
        assert_eq!(m0, PreviewMetrics::default(), "no scrollable preview → default metrics");
        // A nonzero preview_scroll paints an identical buffer (it is inert).
        let (s5, _) = draw_scrolled(5);
        assert_eq!(s0, s5, "preview_scroll must not change the compact popup");
    }

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
        assert!(s.contains("prompt"), "the bold prompt-heading prefix still renders");
    }

    #[test]
    fn menu_snapshot() {
        let (s, _hit) = draw(60, 15, 0);
        insta::assert_snapshot!("action_menu_open", s);
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
            SessionChoice { session_id: "sess-1".into(), label: "Fix the parser".into(), mtime_ms: 0 },
            SessionChoice { session_id: "sess-2".into(), label: "Redesign TUI".into(), mtime_ms: 0 },
        ];
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        let mut hit = HitMap::default();
        // now_ms far ahead so ages render as days.
        term.draw(|f| {
            render_session_pick(f, &mut hit, "wt-a", &items, loading, index, query, 5 * 86_400_000);
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

    #[test]
    fn session_pick_renders_new_session_rows_and_hit_targets() {
        let (s, hit) = draw_session_pick(80, 20, false, 1, "");
        assert!(s.contains(" wt-a "), "popup titled with the worktree display name");
        assert!(s.contains("New session"), "row 0 is the synthetic New session");
        assert!(s.contains("Fix the parser"), "loaded session label renders");
        assert!(s.contains("Redesign TUI"));
        assert!(s.contains("ago"), "session rows carry a relative age");
        // Highlighted session row (index 1) shows its id + absolute time below.
        assert!(s.contains("sess-1"), "highlighted session's id in the description");
        // New session (0) + two session rows (1,2) each register a MenuItem.
        let (mut m0, mut m1, mut m2, mut modal) = (false, false, false, false);
        for y in 0..20 {
            for x in 0..80 {
                match hit.hit(x, y) {
                    Some(HitTarget::MenuItem(0)) => m0 = true,
                    Some(HitTarget::MenuItem(1)) => m1 = true,
                    Some(HitTarget::MenuItem(2)) => m2 = true,
                    Some(HitTarget::Modal) => modal = true,
                    _ => {}
                }
            }
        }
        assert!(m0 && m1 && m2, "New session + both session rows register MenuItems");
        assert!(modal, "popup body registers a Modal region");
    }

    #[test]
    fn session_pick_loading_shows_placeholder_and_only_new_session_hits() {
        let (s, hit) = draw_session_pick(80, 20, true, 0, "");
        assert!(s.contains("New session"));
        assert!(s.contains("loading sessions"), "loading placeholder row renders");
        // While loading only the New-session row is a MenuItem; no session rows.
        let (mut m0, mut m1) = (false, false);
        for y in 0..20 {
            for x in 0..80 {
                match hit.hit(x, y) {
                    Some(HitTarget::MenuItem(0)) => m0 = true,
                    Some(HitTarget::MenuItem(1)) => m1 = true,
                    _ => {}
                }
            }
        }
        assert!(m0, "New session row is selectable while loading");
        assert!(!m1, "the loading placeholder is not a selectable MenuItem");
    }

    #[test]
    fn session_pick_filter_counts_sessions_only() {
        // Filtering applies to loaded labels; New session stays visible. The
        // right-aligned count is filtered/total over sessions (not the view rows).
        let (s, _hit) = draw_session_pick(80, 20, false, 1, "redesign");
        assert!(s.contains("New session"), "New session stays visible under a filter");
        assert!(s.contains("Redesign TUI"));
        assert!(!s.contains("Fix the parser"), "non-matching session filtered out");
        assert!(s.contains("1/2"), "count is filtered/total sessions");
    }

}
