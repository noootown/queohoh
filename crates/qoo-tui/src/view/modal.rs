//! Shared modal frame + input modals. `modal_frame` draws a Clear + rounded,
//! accent-bordered, centered popup and returns its interior Rect; the confirm
//! dialog and button-row helpers build on it.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};

use crate::hit::{ButtonKind, HitMap, HitTarget};
use crate::view::theme::Palette;

/// Interior padding shared by every modal dialog: horizontal 2, vertical 1
/// (top/bottom blank row). All sizing math and `Block::inner` account for it.
pub(crate) const MODAL_PADDING: Padding = Padding { left: 2, right: 2, top: 1, bottom: 1 };

/// Fixed outer width (border included) shared by the launcher and the form so
/// the picker-style dialogs never resize with their content. Clamped to the
/// frame at the call site.
pub(crate) const DIALOG_WIDTH: u16 = 60;

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

/// Shared bottom button row: `[ {primary_label} ]  [ Cancel ]`, left-aligned on
/// the 1-high `row` with a two-column gap. The focused button renders
/// `REVERSED | BOLD`; the unfocused primary in `base`, the unfocused Cancel dim.
/// Registers `Button(Confirm)` (the primary, whatever its label) and
/// `Button(Cancel)`. `base` is the only per-dialog variable — `p.warn` for the
/// destructive confirm, `p.accent` everywhere else.
pub(crate) fn render_button_row(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    row: Rect,
    primary_label: &str,
    focus: Option<ButtonKind>,
    base: ratatui::style::Color,
) {
    let p = Palette::default();
    let primary_btn = format!("[ {primary_label} ]");
    let cancel_btn = "[ Cancel ]";
    let primary_w = primary_btn.chars().count() as u16;
    let cancel_w = cancel_btn.chars().count() as u16;

    // `focus == None` (a field owns focus in the form) highlights NEITHER button.
    // Unified selected style: near-black text (`selection_fg`) on the button's
    // `base`-colored bar (accent-blue for normal dialogs, `warn` for a
    // destructive confirm — the bar keeps its semantic color, the text stays
    // readable). Explicit fg+bg, not REVERSED, so the text color is deterministic
    // and matches list/dropdown selections instead of leaking the terminal's
    // default foreground.
    let focused = Style::default().fg(p.selection_fg).bg(base).add_modifier(Modifier::BOLD);
    let primary_style =
        if matches!(focus, Some(ButtonKind::Confirm)) { focused } else { Style::default().fg(base) };
    let cancel_style =
        if matches!(focus, Some(ButtonKind::Cancel)) { focused } else { p.dim_style() };

    let primary_rect = Rect { x: row.x, y: row.y, width: primary_w, height: 1 };
    let cancel_rect = Rect { x: row.x + primary_w + 2, y: row.y, width: cancel_w, height: 1 };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(primary_btn, primary_style))),
        primary_rect,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(cancel_btn, cancel_style))),
        cancel_rect,
    );
    hit.push(primary_rect, HitTarget::Button(ButtonKind::Confirm));
    hit.push(cancel_rect, HitTarget::Button(ButtonKind::Cancel));
}

/// A single focused `[ Back ]` button (the confirm-dialog focused-button style)
/// left-aligned on the 1-high `row`, for read-only overlays (help, settings)
/// that only need dismissing. Registers `Button(Confirm)` — push it AFTER the
/// overlay's `Modal` region so the click lands on the button, not the body.
pub(crate) fn render_back_button(frame: &mut ratatui::Frame, hit: &mut HitMap, row: Rect, p: &Palette) {
    let btn = "[ Back ]";
    let style = Style::default().fg(p.selection_fg).bg(p.accent).add_modifier(Modifier::BOLD);
    let rect = Rect { x: row.x, y: row.y, width: btn.chars().count() as u16, height: 1 };
    frame.render_widget(Paragraph::new(Line::from(Span::styled(btn, style))), rect);
    hit.push(rect, HitTarget::Button(ButtonKind::Confirm));
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

    // Destructive confirm: warn-colored button row on the last interior line.
    let btn_y = inner.y + inner.height.saturating_sub(1);
    render_button_row(
        frame,
        hit,
        Rect { x: inner.x, y: btn_y, width: inner.width, height: 1 },
        confirm_label,
        Some(focus),
        p.warn,
    );
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
mod button_row_view_tests {
    use super::*;
    use crate::hit::{ButtonKind, HitMap, HitTarget};
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};

    fn draw(
        primary: &str,
        focus: ButtonKind,
        base: ratatui::style::Color,
    ) -> (String, HitMap, ratatui::buffer::Buffer) {
        let mut term = Terminal::new(TestBackend::new(40, 3)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| {
            let row = Rect { x: 1, y: 1, width: 38, height: 1 };
            render_button_row(f, &mut hit, row, primary, Some(focus), base);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..3 {
            for x in 0..40 {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        (s, hit, buf)
    }

    #[test]
    fn draws_both_buttons_and_registers_targets() {
        let (s, hit, _buf) = draw("Next", ButtonKind::Confirm, Palette::default().accent);
        assert!(s.contains("[ Next ]"));
        assert!(s.contains("[ Cancel ]"));
        let (mut c, mut x) = (false, false);
        for y in 0..3 {
            for xx in 0..40 {
                match hit.hit(xx, y) {
                    Some(HitTarget::Button(ButtonKind::Confirm)) => c = true,
                    Some(HitTarget::Button(ButtonKind::Cancel)) => x = true,
                    _ => {}
                }
            }
        }
        assert!(c && x, "both buttons register hit targets");
    }

    #[test]
    fn focus_highlights_the_focused_button_only() {
        // The focused button now carries the selection background (base color,
        // here accent) instead of a REVERSED modifier — plain buttons have no bg.
        let selected = |focus| {
            let (_s, _h, buf) = draw("Next", focus, Palette::default().accent);
            let base = Palette::default().accent;
            let mut out = String::new();
            for y in 0..3 {
                for x in 0..40 {
                    if buf[(x, y)].bg == base {
                        out.push_str(buf[(x, y)].symbol());
                    }
                }
            }
            out
        };
        assert!(selected(ButtonKind::Confirm).contains("Next"));
        assert!(!selected(ButtonKind::Confirm).contains("Cancel"));
        assert!(selected(ButtonKind::Cancel).contains("Cancel"));
        assert!(!selected(ButtonKind::Cancel).contains("Next"));
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
    fn focused_button_uses_the_selection_style() {
        use ratatui::style::Color;
        // Render the confirm dialog and locate the run painted with a selection
        // background (the focused button is `fg(selection_fg).bg(base)+BOLD`;
        // nothing else in the dialog sets a bg). Confirm-focus highlights
        // `[ Remove ]`; Cancel-focus highlights `[ Cancel ]`.
        let body = vec![" body".to_string()];
        let selected_run = |title: &str, focus: ButtonKind| -> String {
            let mut term = Terminal::new(TestBackend::new(80, 12)).unwrap();
            let mut hit = HitMap::default();
            term.draw(|f| render_confirm(f, &mut hit, title, &body, "Remove", focus)).unwrap();
            let buf = term.backend().buffer().clone();
            let mut out = String::new();
            for y in 0..12 {
                for x in 0..80 {
                    let cell = &buf[(x, y)];
                    if cell.bg != Color::Reset {
                        out.push_str(cell.symbol());
                    }
                }
            }
            out
        };
        assert!(selected_run("t", ButtonKind::Confirm).contains("Remove"));
        assert!(!selected_run("t", ButtonKind::Confirm).contains("Cancel"));
        assert!(selected_run("t", ButtonKind::Cancel).contains("Cancel"));
        assert!(!selected_run("t", ButtonKind::Cancel).contains("Remove"));
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
