//! Shared modal frame + input modals. `modal_frame` draws a Clear + rounded,
//! accent-bordered, centered popup and returns its interior Rect.
//! `render_prompt_modal` fills it with a multiline editor body + hint line (the
//! new-task prompt) and registers a `Modal` body hit target.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::hit::{HitMap, HitTarget};
use crate::view::theme::Palette;

/// Clear + rounded, accent-bordered, centered popup. `height` is the interior
/// line count (borders added here). Returns the interior Rect for content.
/// Width = clamp(20, 72, cols − 8), centered in the frame.
pub fn modal_frame(frame: &mut ratatui::Frame, _area: Rect, title: &str, height: u16) -> Rect {
    let p = Palette::default();
    let area = frame.area();
    let width = area.width.saturating_sub(8).clamp(20, 72);
    let outer_h = (height + 2).min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(outer_h)) / 2;
    let rect = Rect { x, y, width, height: outer_h };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

/// Multiline new-task prompt modal. Renders the wrapped `editor` body (with the
/// caret on its row) inside a `modal_frame`, plus a hint line on the bottom
/// interior row. Enter submits, Shift+Enter inserts a newline, Esc cancels — all
/// handled in the update loop; this only draws. Registers the popup body as a
/// `Modal` hit target so clicks inside are opaque to the panes beneath (mouse
/// routing treats a click outside the popup as cancel).
pub fn render_prompt_modal(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    p: &Palette,
    title: &str,
    editor: &crate::view::multiline_input::MultilineInput,
) {
    use crate::view::args_form::{caret_line, wrap_value_cursor};

    // Replicate modal_frame's interior width so we can wrap the body and size the
    // popup before drawing the frame.
    let area = frame.area();
    let outer_w = area.width.saturating_sub(8).clamp(20, 72);
    let interior_w = outer_w.saturating_sub(2);
    // Reserve one column so the caret can sit past a full row's last char.
    let wrap_w = (interior_w as usize).saturating_sub(1).max(1);
    let (lines, cur_row, cur_col) = wrap_value_cursor(&editor.text, editor.cursor, wrap_w);

    // Body height caps at 12 rows (modal_frame further clamps to the frame);
    // interior = body rows + 1 hint row.
    let body_h = (lines.len() as u16).clamp(1, 12);
    let inner = modal_frame(frame, area, title, body_h + 1);

    // Register the popup body (interior grown by the border ring).
    let body = Rect {
        x: inner.x.saturating_sub(1),
        y: inner.y.saturating_sub(1),
        width: inner.width + 2,
        height: inner.height + 2,
    };
    hit.push(body, HitTarget::Modal);

    // Body rows are everything above the bottom hint row. Window the wrapped
    // lines so the caret row stays visible when the body overflows.
    let body_rows = inner.height.saturating_sub(1) as usize;
    let start = cur_row.saturating_sub(body_rows.saturating_sub(1));
    for (ri, line) in lines.iter().enumerate().skip(start).take(body_rows) {
        let y = inner.y + (ri - start) as u16;
        let row = Rect { x: inner.x, y, width: inner.width, height: 1 };
        if ri == cur_row {
            frame.render_widget(caret_line(line, cur_col, p), row);
        } else {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(line.clone(), Style::default().fg(p.fg)))),
                row,
            );
        }
    }

    // Hint line on the bottom interior row.
    let hint = " enter submit · shift+enter newline · esc cancel";
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(hint, p.dim_style()))),
        Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        },
    );
}

/// Bulk-remove confirmation: a warning line, up to 8 worktree names, then
/// "…and N more" when the range exceeds 8, and a key hint. Self-sizes to the
/// (longest) warning line so it never truncates on a narrow terminal, capped at
/// the frame; registers the popup body as a `Modal` hit target (y confirms via
/// the app's key branch). Built directly rather than via `modal_frame` because
/// the warning is wider than that helper's clamped interior on small screens.
/// Confirm dialog for the queue `x` cancel action. `count` is the number of
/// tasks that will be cancelled; `summary` is the one-line description
/// (`cancel 1 queued task` / `cancel 3 tasks (1 running will be stopped)`).
/// Default focus is confirm — Enter (or y) fires; n/esc cancel.
pub fn render_confirm_cancel(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    count: usize,
    summary: &str,
) {
    let p = Palette::default();
    let hint = " enter/y confirm · n/esc dismiss";
    // Width to the widest line (summary or hint), plus the border ring.
    let inner_w = summary.chars().count().max(hint.chars().count());
    let area = frame.area();
    let width = (inner_w as u16 + 2).min(area.width);
    let height = 4u16.min(area.height); // summary + hint + border ring
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let rect = Rect { x, y, width, height };

    frame.render_widget(Clear, rect);
    hit.push(rect, HitTarget::Modal);

    let block = Block::default()
        .title(Span::styled(
            format!(" Cancel {count} task{} ", if count == 1 { "" } else { "s" }),
            Style::default().fg(p.warn).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.warn));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let lines = vec![
        Line::from(Span::styled(summary.to_string(), Style::default().fg(p.fg))),
        Line::from(Span::styled(hint, p.dim_style())),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

pub fn render_confirm_bulk_remove(frame: &mut ratatui::Frame, hit: &mut HitMap, names: &[String]) {
    let p = Palette::default();
    let normal = Style::default().fg(p.fg);
    let shown = names.len().min(8);
    let extra = names.len().saturating_sub(8);

    // No leading space: the warning is the widest line, so keeping it flush-left
    // lets the whole message fit inside the border on a 60-col terminal.
    let warn = "discards uncommitted changes and deletes each local branch";
    let inner_h = (1 + shown + if extra > 0 { 1 } else { 0 } + 1) as u16; // warn + names + more? + hint

    let area = frame.area();
    let width = (warn.len() as u16 + 2).min(area.width); // +2 for the border ring
    let height = (inner_h + 2).min(area.height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let rect = Rect { x, y, width, height };

    frame.render_widget(Clear, rect);
    hit.push(rect, HitTarget::Modal);

    let block = Block::default()
        .title(Span::styled(
            format!(" Remove {} worktrees ", names.len()),
            Style::default().fg(p.warn).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.warn));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(warn, normal)));
    for name in names.iter().take(8) {
        lines.push(Line::from(Span::styled(format!("  {name}"), normal)));
    }
    if extra > 0 {
        lines.push(Line::from(Span::styled(format!("  …and {extra} more"), p.dim_style())));
    }
    lines.push(Line::from(Span::styled(" y confirm · n/esc cancel", p.dim_style())));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Create-worktree modal: a bordered branch-name input with an inline red error
/// row and a key hint. The popup registers a `Modal` hit target; Enter/esc drive
/// it (no OK/Cancel buttons). `error` renders red on the row under the field when set.
pub fn render_create_worktree(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    repo: &str,
    input: &tui_input::Input,
    error: Option<&str>,
) {
    let p = Palette::default();
    let normal = Style::default().fg(p.fg);
    let inner = modal_frame(frame, frame.area(), &format!("Create worktree — {repo}"), 3);

    // Register the popup body (outer rect = inner grown by the border ring) so
    // clicks inside are opaque to the panes beneath.
    let body = Rect {
        x: inner.x.saturating_sub(1),
        y: inner.y.saturating_sub(1),
        width: inner.width + 2,
        height: inner.height + 2,
    };
    hit.push(body, HitTarget::Modal);

    // Field line: "branch> value█".
    let field = Line::from(vec![
        Span::styled(" branch> ", p.dim_style()),
        Span::styled(input.value().to_string(), normal),
        Span::styled("█", Style::default().fg(p.accent)),
    ]);
    frame.render_widget(
        Paragraph::new(field),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );

    // Second interior row: the inline error (red) when validation failed.
    if let Some(msg) = error
        && inner.height >= 2 {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(" {msg}"),
                    Style::default().fg(p.error),
                ))),
                Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
            );
        }

    // Hint line on the bottom interior row.
    if inner.height >= 3 {
        let hint = Line::from(Span::styled(" enter submit · esc cancel", p.dim_style()));
        frame.render_widget(
            Paragraph::new(hint),
            Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width,
                height: 1,
            },
        );
    }
}

#[cfg(test)]
mod modal_view_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    #[test]
    fn width_clamps_to_72_and_centers() {
        let mut term = Terminal::new(TestBackend::new(200, 40)).unwrap();
        let mut r = Rect::default();
        term.draw(|f| {
            r = modal_frame(f, f.area(), "t", 3);
        })
        .unwrap();
        // interior width = 72 - 2 border cols = 70; left border at (200-72)/2 = 64.
        assert_eq!(r.width, 70);
        assert_eq!(r.x, 65); // 64 border + 1
    }
}

#[cfg(test)]
mod prompt_modal_view_tests {
    use super::*;
    use crate::hit::{HitMap, HitTarget};
    use crate::view::multiline_input::MultilineInput;
    use ratatui::{Terminal, backend::TestBackend};

    fn render_prompt(cols: u16, rows: u16, text: &str) -> (String, HitMap) {
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        let mut hit = HitMap::default();
        let p = Palette::default();
        let editor = MultilineInput { text: text.to_string(), cursor: text.chars().count() };
        term.draw(|f| render_prompt_modal(f, &mut hit, &p, "New task — platform (adhoc)", &editor))
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
    fn shows_title_body_and_multiline_hint() {
        let (s, _hit) = render_prompt(60, 15, "first line\nsecond line");
        assert!(s.contains("New task — platform (adhoc)"));
        assert!(s.contains("first line"));
        assert!(s.contains("second line"));
        assert!(s.contains("enter submit · shift+enter newline · esc cancel"));
        // No OK/Cancel buttons on the multiline prompt modal.
        assert!(!s.contains("[ OK ]"));
    }

    #[test]
    fn registers_modal_body_hit_target() {
        let (_s, hit) = render_prompt(60, 15, "hi");
        let mut modal = false;
        for y in 0..15 {
            for x in 0..60 {
                if hit.hit(x, y) == Some(&HitTarget::Modal) {
                    modal = true;
                }
            }
        }
        assert!(modal);
    }

    #[test]
    fn prompt_modal_snapshot() {
        let (s, _hit) = render_prompt(60, 15, "run this now\nand a second line");
        insta::assert_snapshot!("prompt_modal", s);
    }
}

#[cfg(test)]
mod bulk_confirm_view_tests {
    use super::*;
    use crate::hit::HitMap;
    use ratatui::{backend::TestBackend, Terminal};

    fn draw(names: &[String]) -> String {
        let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| render_confirm_bulk_remove(f, &mut hit, names)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..20 { for x in 0..60 { s.push_str(buf[(x, y)].symbol()); } s.push('\n'); }
        s
    }

    #[test]
    fn lists_up_to_eight_names_then_and_n_more() {
        let names: Vec<String> = (0..10).map(|i| format!("wt-{i}")).collect();
        let s = draw(&names);
        assert!(s.contains("Remove 10 worktrees"));
        assert!(s.contains("discards uncommitted changes and deletes each local branch"));
        assert!(s.contains("wt-0"));
        assert!(s.contains("wt-7"));
        assert!(!s.contains("wt-8")); // truncated after 8
        assert!(s.contains("…and 2 more"));
    }
}
