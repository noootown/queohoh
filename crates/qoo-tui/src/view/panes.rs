use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use throbber_widgets_tui::{Throbber, ThrobberState};

use crate::app::{App, ListPane, PaneId, Selection};
use crate::hit::{HitMap, HitTarget};
use crate::ipc::types::DefinitionSummary;
use crate::selectors::{QueueRow, WorktreeRow, WtState, arg_summary, pane_layout, pane_title};
use crate::view::theme::{GLYPH_DISCOVERY, GLYPH_DOT, GLYPH_MAIN_SESSION, GLYPH_MAIN_WT, Palette};
use crate::view::{Computed, selection_range, window_start};

/// Render one pane's chrome (rounded border, focused accent, bold title). Returns
/// the inner content `Rect` (below the title line).
fn pane_chrome(
    frame: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    focused: bool,
    p: &Palette,
) -> Rect {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(p.border_style(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // Title line (bold) at the top of the inner area.
    if inner.height > 0 {
        let title_line = Line::from(Span::styled(
            title.to_string(),
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(
            Paragraph::new(title_line),
            Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
        );
    }
    // Content region starts one row below the title.
    Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: inner.height.saturating_sub(1),
    }
}

fn queue_line(row: &QueueRow, p: &Palette) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    // Glyph column: running rows get a placeholder space (throbber painted over).
    if row.running {
        spans.push(Span::raw(" "));
    } else {
        spans.push(Span::raw(row.glyph.to_string()));
    }
    spans.push(Span::raw(" "));
    if row.main_session {
        spans.push(Span::styled(format!("{GLYPH_MAIN_SESSION} "), Style::default().fg(p.info)));
    }
    spans.push(Span::raw(format!("{} {} {}", row.lane, row.summary, row.detail)));
    Line::from(spans)
}

fn worktree_line(row: &WorktreeRow, p: &Palette) -> Line<'static> {
    let dot = match row.state {
        WtState::Free => p.ok,
        WtState::Busy | WtState::You => p.warn,
        WtState::Failed => p.error,
    };
    let mut spans = vec![
        Span::styled(GLYPH_DOT.to_string(), Style::default().fg(dot)),
        Span::raw(format!(" {}", row.name)),
    ];
    if row.has_main_session {
        spans.push(Span::styled(format!(" {GLYPH_MAIN_WT}"), Style::default().fg(p.info)));
    }
    if row.queued > 0 {
        spans.push(Span::styled(format!(" [{}]", row.queued), p.dim_style()));
    }
    Line::from(spans)
}

fn def_line(def: &DefinitionSummary) -> Line<'static> {
    let mut s = def.name.clone();
    if !def.args.is_empty() {
        s.push_str(&format!(" ({})", arg_summary(&def.args)));
    }
    if def.has_discovery {
        s.push(' ');
        s.push(GLYPH_DISCOVERY);
    }
    Line::from(s)
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

/// Shared renderer for all three list panes: chrome + `PaneBody` hit, empty
/// state, cursor-centered windowing, per-row selection/dim styling + `Row` hit,
/// throbbers over running rows, and the scrollbar. Only the line-builder, the
/// empty message, and the per-row `dim`/`running` predicates differ between
/// panes — keeping the loop here means queue/tasks/worktrees can never drift.
#[allow(clippy::too_many_arguments)]
fn render_list_pane<T>(
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
    line_of: impl Fn(&T, &Palette) -> Line<'static>,
    dim_of: impl Fn(&T) -> bool,
    running_of: impl Fn(&T) -> bool,
) {
    let title = pane_title(title_base, sel, search, searching);
    let inner = pane_chrome(frame, area, &title, focused, p);
    hits.push(inner, HitTarget::PaneBody(pane));

    if rows.is_empty() {
        frame.render_widget(Paragraph::new(empty_msg.to_string()).style(p.dim_style()), inner);
        return;
    }

    let (start_i, end_i) = selection_range(sel);
    let cap = inner.height as usize;
    let offset = window_start(rows.len(), sel.cursor, cap);
    let visible = cap.min(rows.len() - offset);
    let mut lines: Vec<Line> = Vec::with_capacity(visible);
    for vi in 0..visible {
        let idx = offset + vi;
        let mut line = line_of(&rows[idx], p);
        let selected = focused && idx >= start_i && idx <= end_i;
        if selected {
            line = line.style(p.selection());
        } else if dim_of(&rows[idx]) {
            line = line.style(p.dim_style());
        }
        lines.push(line);
        hits.push(
            Rect { x: inner.x, y: inner.y + vi as u16, width: inner.width, height: 1 },
            HitTarget::Row(list_pane, idx),
        );
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);

    // Throbbers over running rows (seeded from the wall clock so they animate on
    // the 1 s tick without App holding throbber state). Panes with no running
    // rows paint nothing here.
    let mut tstate = ThrobberState::default();
    for _ in 0..(now_epoch_s % 8) {
        tstate.calc_next();
    }
    for vi in 0..visible {
        let idx = offset + vi;
        if running_of(&rows[idx]) {
            let mut st = tstate.clone();
            frame.render_stateful_widget(
                Throbber::default(),
                Rect { x: inner.x, y: inner.y + vi as u16, width: 1, height: 1 },
                &mut st,
            );
        }
    }
    render_scrollbar(frame, inner, rows.len(), offset, visible, pane, hits);
}

pub fn render(app: &App, c: &Computed, frame: &mut ratatui::Frame, area: Rect, hits: &mut HitMap) {
    let p = &c.palette;
    let layout = pane_layout(area.height);
    let regions = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(layout.queue_h),
            ratatui::layout::Constraint::Length(layout.tasks_h),
            ratatui::layout::Constraint::Length(layout.worktrees_h),
        ])
        .split(area);

    render_list_pane(
        frame,
        regions[0],
        hits,
        p,
        "QUEUE",
        &c.ui.search[0],
        c.searching[0],
        matches!(c.ui.focus, PaneId::Queue),
        PaneId::Queue,
        ListPane::Queue,
        &c.queue_sel,
        &c.queue,
        "queue empty — [a] on a worktree to add a task",
        app.now_epoch_s,
        queue_line,
        |row| row.archived,
        |row| row.running,
    );
    render_list_pane(
        frame,
        regions[1],
        hits,
        p,
        "TASKS",
        &c.ui.search[1],
        c.searching[1],
        matches!(c.ui.focus, PaneId::Tasks),
        PaneId::Tasks,
        ListPane::Tasks,
        &c.tasks_sel,
        &c.defs,
        "no task definitions",
        app.now_epoch_s,
        |def, _p| def_line(def),
        |_| false,
        |_| false,
    );
    render_list_pane(
        frame,
        regions[2],
        hits,
        p,
        "WORKTREES",
        &c.ui.search[2],
        c.searching[2],
        matches!(c.ui.focus, PaneId::Worktrees),
        PaneId::Worktrees,
        ListPane::Worktrees,
        &c.wt_sel,
        &c.worktrees,
        "no worktrees",
        app.now_epoch_s,
        worktree_line,
        |_| false,
        |_| false,
    );
}
