//! Shared modal frame + input modals. `modal_frame` draws a Clear + rounded,
//! accent-bordered, centered popup and returns its interior Rect.
//! `render_prompt_modal` fills it with a multiline editor body + hint line (the
//! new-task prompt) and registers a `Modal` body hit target.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};

use crate::hit::{ButtonKind, HitMap, HitTarget};
use crate::view::theme::Palette;

/// Interior padding shared by every modal dialog: horizontal 2, vertical 1
/// (top/bottom blank row). All sizing math and `Block::inner` account for it.
const MODAL_PADDING: Padding = Padding { left: 2, right: 2, top: 1, bottom: 1 };

/// Clear + rounded, accent-bordered, centered popup with `MODAL_PADDING`.
/// `height` is the PADDED interior line count (borders + padding added here).
/// Registers the whole popup rect as an opaque `Modal` hit target and returns
/// the padded interior Rect for content. Width = clamp(20, 72, cols − 8),
/// centered in the frame.
pub fn modal_frame(frame: &mut ratatui::Frame, hit: &mut HitMap, title: &str, height: u16) -> Rect {
    let p = Palette::default();
    let area = frame.area();
    let width = area.width.saturating_sub(8).clamp(20, 72);
    // border ring (2) + vertical padding (top 1 + bottom 1).
    let outer_h = (height + 4).min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(outer_h)) / 2;
    let rect = Rect { x, y, width, height: outer_h };
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

    // Replicate modal_frame's padded interior width so we can wrap the body and
    // size the popup before drawing the frame (border ring 2 + horizontal
    // padding 4).
    let area = frame.area();
    let outer_w = area.width.saturating_sub(8).clamp(20, 72);
    let interior_w = outer_w.saturating_sub(6);
    // Reserve one column so the caret can sit past a full row's last char.
    let wrap_w = (interior_w as usize).saturating_sub(1).max(1);
    let (lines, cur_row, cur_col) = wrap_value_cursor(&editor.text, editor.cursor, wrap_w);

    // Body height caps at 12 rows (modal_frame further clamps to the frame);
    // interior = body rows + 1 hint row. `modal_frame` registers the Modal hit
    // target over the whole popup.
    let body_h = (lines.len() as u16).clamp(1, 12);
    let inner = modal_frame(frame, hit, title, body_h + 1);

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
    let hint = "enter submit · shift+enter newline";
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

/// Unified destructive-confirmation dialog (remove worktree, bulk remove, queue
/// cancel). Warn-colored rounded border + bold `title` and `MODAL_PADDING`; the
/// `body` message lines as-is; a blank separator; then a
/// `[ confirm_label ] [ Cancel ]` button row. `focus` picks the highlighted
/// button (reversed+bold); the other renders plain. Self-sizes to the widest of
/// title/body/button-row (clamped, centered, capped at the frame). Registers the
/// popup body as a `Modal` hit target (inert) and the two buttons as
/// `Button(Confirm)`/`Button(Cancel)`; an outside click dismisses (handled in
/// `on_mouse`).
pub fn render_confirm(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    title: &str,
    body: &[String],
    confirm_label: &str,
    focus: ButtonKind,
) {
    let p = Palette::default();
    let confirm_btn = format!("[ {confirm_label} ]");
    let cancel_btn = "[ Cancel ]";
    // Button row: "confirm  cancel" (two-column gap). Feeds sizing so the row is
    // never truncated.
    let confirm_w = confirm_btn.chars().count();
    let cancel_w = cancel_btn.chars().count();
    let button_row_w = confirm_w + 2 + cancel_w;

    // Interior width fits the widest of the framed title, each body line, and the
    // button row.
    let inner_w = body
        .iter()
        .map(|l| l.chars().count())
        .chain([title.chars().count() + 2, button_row_w])
        .max()
        .unwrap_or(button_row_w);

    let area = frame.area();
    // border ring (2) + horizontal padding (4).
    let width = (inner_w as u16 + 6).clamp(20, 72).min(area.width);
    // body lines + blank separator + button row, plus the border ring (2) and
    // vertical padding (2).
    let inner_h = body.len() as u16 + 2;
    let height = (inner_h + 4).min(area.height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let rect = Rect { x, y, width, height };

    frame.render_widget(Clear, rect);
    hit.push(rect, HitTarget::Modal);

    let block = Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(p.warn).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.warn))
        .padding(MODAL_PADDING);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    // Message lines (uniform fg) followed by a blank separator; the button row
    // draws on the last interior line.
    let mut lines: Vec<Line> =
        body.iter().map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(p.fg)))).collect();
    lines.push(Line::from(""));
    frame.render_widget(Paragraph::new(lines), inner);

    // Focused button: reversed + bold; unfocused: plain (warn Confirm, dim
    // Cancel).
    let focused = Style::default().fg(p.warn).add_modifier(Modifier::REVERSED | Modifier::BOLD);
    let confirm_style =
        if matches!(focus, ButtonKind::Confirm) { focused } else { Style::default().fg(p.warn) };
    let cancel_style =
        if matches!(focus, ButtonKind::Cancel) { focused } else { p.dim_style() };

    let btn_y = inner.y + inner.height.saturating_sub(1);
    let confirm_rect = Rect { x: inner.x, y: btn_y, width: confirm_w as u16, height: 1 };
    let cancel_rect =
        Rect { x: inner.x + confirm_w as u16 + 2, y: btn_y, width: cancel_w as u16, height: 1 };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(confirm_btn, confirm_style))),
        confirm_rect,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(cancel_btn, cancel_style))),
        cancel_rect,
    );
    hit.push(confirm_rect, HitTarget::Button(ButtonKind::Confirm));
    hit.push(cancel_rect, HitTarget::Button(ButtonKind::Cancel));
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
    // `modal_frame` registers the Modal hit target over the whole popup so
    // clicks inside are opaque to the panes beneath.
    let inner = modal_frame(frame, hit, &format!("Create worktree — {repo}"), 3);

    // Field line: "branch> value█".
    let field = Line::from(vec![
        Span::styled("branch> ", p.dim_style()),
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
                    msg.to_string(),
                    Style::default().fg(p.error),
                ))),
                Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
            );
        }

    // Hint line on the bottom interior row.
    if inner.height >= 3 {
        let hint = Line::from(Span::styled("enter submit", p.dim_style()));
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
        use crate::hit::HitMap;
        let mut term = Terminal::new(TestBackend::new(200, 40)).unwrap();
        let mut r = Rect::default();
        term.draw(|f| {
            let mut hit = HitMap::default();
            r = modal_frame(f, &mut hit, "t", 3);
        })
        .unwrap();
        // padded interior width = 72 − 2 border − 4 padding = 66; left border at
        // (200−72)/2 = 64, then +1 border +2 padding.
        assert_eq!(r.width, 66);
        assert_eq!(r.x, 67);
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
        assert!(s.contains("enter submit · shift+enter newline"));
        // Esc-to-dismiss is universal and deliberately unadvertised.
        assert!(!s.contains("esc cancel"));
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
mod confirm_view_tests {
    use super::*;
    use crate::hit::{ButtonKind, HitMap, HitTarget};
    use ratatui::{backend::TestBackend, Terminal};

    fn draw(
        cols: u16,
        rows: u16,
        title: &str,
        body: &[String],
        confirm_label: &str,
    ) -> (String, HitMap) {
        draw_focus(cols, rows, title, body, confirm_label, ButtonKind::Confirm)
    }

    fn draw_focus(
        cols: u16,
        rows: u16,
        title: &str,
        body: &[String],
        confirm_label: &str,
        focus: ButtonKind,
    ) -> (String, HitMap) {
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| render_confirm(f, &mut hit, title, body, confirm_label, focus)).unwrap();
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
    fn renders_title_body_and_button_row() {
        let body = vec![
            " Remove acme.feature on branch feature/x?".to_string(),
            " This discards uncommitted changes and deletes the local branch.".to_string(),
        ];
        let (s, _hit) = draw(80, 12, "Remove worktree", &body, "Remove");
        assert!(s.contains("Remove worktree"));
        assert!(s.contains("acme.feature on branch feature/x?"));
        assert!(s.contains("[ Remove ]"));
        // The Cancel button drops the old "Esc" wording.
        assert!(s.contains("[ Cancel ]"));
        assert!(!s.contains("Esc cancel"));
        assert!(!s.contains("y confirm"));
    }

    #[test]
    fn focused_button_is_reversed_and_bold() {
        use ratatui::style::Modifier;
        // Render the confirm dialog and locate the reversed+bold run — it must
        // sit on the FOCUSED button. Confirm-focus reverses `[ Remove ]`;
        // Cancel-focus reverses `[ Cancel ]`.
        let body = vec![" body".to_string()];
        let reversed_run = |title: &str, focus: ButtonKind| -> String {
            let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
            let mut hit = HitMap::default();
            term.draw(|f| render_confirm(f, &mut hit, title, &body, "Remove", focus)).unwrap();
            let buf = term.backend().buffer().clone();
            let mut out = String::new();
            for y in 0..12 {
                for x in 0..80 {
                    let cell = &buf[(x, y)];
                    if cell.modifier.contains(Modifier::REVERSED) {
                        out.push_str(cell.symbol());
                    }
                }
            }
            out
        };
        assert!(reversed_run("t", ButtonKind::Confirm).contains("Remove"));
        assert!(!reversed_run("t", ButtonKind::Confirm).contains("Cancel"));
        assert!(reversed_run("t", ButtonKind::Cancel).contains("Cancel"));
        assert!(!reversed_run("t", ButtonKind::Cancel).contains("Remove"));
    }

    #[test]
    fn registers_modal_and_button_hit_targets() {
        let body = vec!["cancel 3 tasks (1 running will be stopped)".to_string()];
        let (_s, hit) = draw(80, 12, "Cancel 3 tasks", &body, "Cancel tasks");
        let (mut modal, mut confirm, mut cancel) = (false, false, false);
        for y in 0..12 {
            for x in 0..80 {
                match hit.hit(x, y) {
                    Some(HitTarget::Modal) => modal = true,
                    Some(HitTarget::Button(ButtonKind::Confirm)) => confirm = true,
                    Some(HitTarget::Button(ButtonKind::Cancel)) => cancel = true,
                    _ => {}
                }
            }
        }
        assert!(modal, "popup body registers a Modal region");
        assert!(confirm && cancel, "both buttons register hit targets");
    }

    #[test]
    fn bulk_remove_variant_lists_up_to_eight_then_and_n_more() {
        // Bulk-remove body: warning line, up to 8 names, then "…and N more".
        let mut body = vec!["discards uncommitted changes and deletes each local branch".to_string()];
        body.extend((0..10).map(|i| format!("  wt-{i}")));
        // The constructor caps at 8 + a "…and N more" line; mirror that here.
        body.truncate(9);
        body.push("  …and 2 more".to_string());
        let (s, _hit) = draw(80, 20, "Remove 10 worktrees", &body, "Remove");
        assert!(s.contains("Remove 10 worktrees"));
        assert!(s.contains("wt-7"));
        assert!(!s.contains("wt-8")); // truncated after 8
        assert!(s.contains("…and 2 more"));
        insta::assert_snapshot!("confirm_bulk_remove", s);
    }
}
