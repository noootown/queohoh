//! Shared modal frame + single-field input modal (add-task, assign-worktree).
//! `modal_frame` draws a Clear + rounded, accent-bordered, centered popup and
//! returns its interior Rect. `render_input_modal` fills that interior with a
//! label/value field, a hint line, and clickable `[ OK ] [ Cancel ]` buttons,
//! registering a `Modal` body target plus `Button` targets on top.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::hit::{ButtonKind, HitMap, HitTarget};
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

/// Single-field input modal with clickable `[ OK ]` / `[ Cancel ]` buttons.
/// Layout: field line (`label> value█`), hint line, buttons line. The whole
/// popup registers a `Modal` target; the two buttons register `Button` targets
/// on top of it so a click resolves to Confirm/Cancel where they overlap.
pub fn render_input_modal(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    title: &str,
    label: &str,
    input: &tui_input::Input,
) {
    let p = Palette::default();
    let normal = Style::default().fg(p.fg);
    let inner = modal_frame(frame, frame.area(), title, 3);

    // Register the popup body (outer rect = inner grown by the border ring) so
    // clicks inside are opaque to the panes beneath.
    let body = Rect {
        x: inner.x.saturating_sub(1),
        y: inner.y.saturating_sub(1),
        width: inner.width + 2,
        height: inner.height + 2,
    };
    hit.push(body, HitTarget::Modal);

    // Field line: "label> value█".
    let field = Line::from(vec![
        Span::styled(format!(" {label}> "), p.dim_style()),
        Span::styled(input.value().to_string(), normal),
        Span::styled("█", Style::default().fg(p.accent)),
    ]);
    frame.render_widget(
        Paragraph::new(field),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );

    // Hint line just under the field (middle interior row).
    if inner.height >= 3 {
        let hint = Line::from(Span::styled(" enter submit · esc cancel", p.dim_style()));
        frame.render_widget(
            Paragraph::new(hint),
            Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
        );
    }

    // Buttons line (bottom interior row).
    let btn_y = inner.y + inner.height.saturating_sub(1);
    let ok = " [ OK ] ";
    let cancel = "[ Cancel ] ";
    let ok_rect = Rect { x: inner.x, y: btn_y, width: ok.len() as u16, height: 1 };
    let cancel_rect =
        Rect { x: inner.x + ok.len() as u16, y: btn_y, width: cancel.len() as u16, height: 1 };
    frame.render_widget(Paragraph::new(Line::from(Span::styled(ok, p.selection()))), ok_rect);
    frame.render_widget(Paragraph::new(Line::from(Span::styled(cancel, normal))), cancel_rect);
    hit.push(ok_rect, HitTarget::Button(ButtonKind::Confirm));
    hit.push(cancel_rect, HitTarget::Button(ButtonKind::Cancel));
}

/// Bulk-remove confirmation: a warning line, up to 8 worktree names, then
/// "…and N more" when the range exceeds 8, and a key hint. Self-sizes to the
/// (longest) warning line so it never truncates on a narrow terminal, capped at
/// the frame; registers the popup body as a `Modal` hit target (y confirms via
/// the app's key branch). Built directly rather than via `modal_frame` because
/// the warning is wider than that helper's clamped interior on small screens.
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
/// row and a key hint. Same body/hit-target shape as `render_input_modal` (the
/// popup registers a `Modal` target) but without OK/Cancel buttons — Enter/esc
/// drive it. `error` renders red on the row under the field when set.
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
    if let Some(msg) = error {
        if inner.height >= 2 {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(" {msg}"),
                    Style::default().fg(p.error),
                ))),
                Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
            );
        }
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
    use crate::hit::{ButtonKind, HitMap, HitTarget};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use tui_input::Input;

    fn render_input(cols: u16, rows: u16, value: &str) -> (String, HitMap) {
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        let mut hit = HitMap::default();
        let input = Input::new(value.to_string());
        term.draw(|f| {
            render_input_modal(
                f,
                &mut hit,
                "New task — fresh session — platform (adhoc)",
                "prompt",
                &input,
            )
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

    #[test]
    fn shows_label_value_and_buttons() {
        let (s, _hit) = render_input(80, 15, "hello");
        assert!(s.contains("prompt"));
        assert!(s.contains("hello"));
        assert!(s.contains("[ OK ]"));
        assert!(s.contains("[ Cancel ]"));
    }

    #[test]
    fn buttons_register_hit_targets() {
        let (_s, hit) = render_input(80, 15, "");
        let mut ok = false;
        let mut cancel = false;
        let mut modal = false;
        for y in 0..15 {
            for x in 0..80 {
                match hit.hit(x, y) {
                    Some(HitTarget::Button(ButtonKind::Confirm)) => ok = true,
                    Some(HitTarget::Button(ButtonKind::Cancel)) => cancel = true,
                    Some(HitTarget::Modal) => modal = true,
                    _ => {}
                }
            }
        }
        assert!(ok && cancel && modal);
    }

    #[test]
    fn add_task_modal_snapshot() {
        let (s, _hit) = render_input(60, 15, "run this now");
        insta::assert_snapshot!("add_task_modal", s);
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
