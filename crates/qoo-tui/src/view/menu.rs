//! Centered action-menu popup and the destructive worktree-remove confirm.
//! Both register a `Modal` hit target over the whole popup (clicks can't leak
//! through to the panes beneath); the action menu additionally registers one
//! `MenuItem(i)` target per row so a click resolves to the row under it.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph};

use crate::action_menu::ActionItem;
use crate::hit::{HitMap, HitTarget};
use crate::ipc::types::DefinitionSummary;
use crate::selectors::arg_summary;
use crate::view::theme::{GLYPH_DISCOVERY, MARKER_GLOBAL, Palette};

/// Centered popup: width = clamp(20, 72, cols − 8); height fits the rows + hint +
/// borders. Registers a `Modal` target over the whole popup plus one
/// `MenuItem(i)` target per row. Disabled rows render dimmed with `— reason`;
/// the highlighted (enabled) row is inverse-styled.
pub fn render_menu(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    title: &str,
    items: &[ActionItem],
    index: usize,
) {
    let p = Palette::default();
    let area = frame.area();
    let width = area.width.saturating_sub(8).clamp(20, 72);
    // interior line count = items + one hint line; +2 for the top/bottom border.
    let inner_h = items.len() as u16 + 1;
    let height = (inner_h + 2).min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect { x, y, width, height };

    frame.render_widget(Clear, rect);
    hit.push(rect, HitTarget::Modal); // popup body: opaque to clicks

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

    // Each row gets a MenuItem hit rect; disabled rows dim + "— reason".
    let mut rows: Vec<ListItem> = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        let row_rect = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        if row_rect.y < inner.y + inner.height {
            hit.push(row_rect, HitTarget::MenuItem(i));
        }
        let text = match &it.disabled {
            Some(reason) => format!(" {} — {reason}", it.label),
            None => format!(" {}", it.label),
        };
        let style = if it.disabled.is_some() {
            p.dim_style()
        } else if i == index {
            p.selection()
        } else {
            Style::default().fg(p.fg)
        };
        rows.push(ListItem::new(Line::from(Span::styled(text, style))));
    }
    let list = List::new(rows);
    let list_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: (items.len() as u16).min(inner.height),
    };
    frame.render_widget(list, list_area);

    // Hint on the last interior line.
    let hint_y = inner.y + inner.height.saturating_sub(1);
    let hint = Line::from(Span::styled(" ↑/↓ move · enter run · esc close", p.dim_style()));
    frame.render_widget(
        Paragraph::new(hint),
        Rect { x: inner.x, y: hint_y, width: inner.width, height: 1 },
    );
}

/// "Run task definition" picker. Same centered-popup pattern as `render_menu`:
/// a `Modal` body target plus one `MenuItem(i)` per row. Each row shows the def
/// name, its arg summary (when it takes args), a `⏰` discovery glyph, and a
/// trailing `(g)` marker for global-scope defs. The highlighted row is inverse.
pub fn render_def_pick(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    title: &str,
    defs: &[DefinitionSummary],
    index: usize,
) {
    let p = Palette::default();
    let area = frame.area();
    let width = area.width.saturating_sub(8).clamp(20, 72);
    let inner_h = defs.len() as u16 + 1; // rows + one hint line
    let height = (inner_h + 2).min(area.height);
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
        .border_style(Style::default().fg(p.accent));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    for (i, def) in defs.iter().enumerate() {
        let row = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        if row.y >= inner.y + inner.height.saturating_sub(1) {
            break; // leave the last interior line for the hint
        }
        hit.push(row, HitTarget::MenuItem(i));

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
        let style = if i == index { p.selection() } else { Style::default().fg(p.fg) };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, style))),
            row,
        );
    }

    let hint_y = inner.y + inner.height.saturating_sub(1);
    let hint = Line::from(Span::styled(" ↑/↓ move · enter run · q/esc close", p.dim_style()));
    frame.render_widget(
        Paragraph::new(hint),
        Rect { x: inner.x, y: hint_y, width: inner.width, height: 1 },
    );
}

/// Destructive-confirm popup for `Remove worktree…`. Warns that removal discards
/// uncommitted changes and deletes the local branch, then waits for y / n. Like
/// the menu it registers a `Modal` target so body clicks are inert (a click
/// outside closes it — handled in `on_mouse`).
pub fn render_confirm_remove(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    worktree: &str,
    branch: &str,
) {
    let p = Palette::default();
    let area = frame.area();
    let width = area.width.saturating_sub(8).clamp(28, 72);
    let height = 7u16.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect { x, y, width, height };

    frame.render_widget(Clear, rect);
    hit.push(rect, HitTarget::Modal);

    let block = Block::default()
        .title(Span::styled(
            " remove worktree ",
            Style::default().fg(p.warn).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.warn));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let branch_line = if branch.is_empty() {
        String::new()
    } else {
        format!(" on branch {branch}")
    };
    let lines = vec![
        Line::from(Span::styled(format!(" Remove {worktree}{branch_line}?"), Style::default().fg(p.fg))),
        Line::from(Span::styled(
            " This discards uncommitted changes and deletes the local branch.",
            p.dim_style(),
        )),
        Line::from(""),
        Line::from(Span::styled(" y confirm · n cancel", p.dim_style())),
    ];
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

#[cfg(test)]
mod menu_view_tests {
    use super::*;
    use crate::action_menu::{ActionItem, MenuAction};
    use crate::hit::{HitMap, HitTarget};
    use ratatui::{Terminal, backend::TestBackend};

    fn items() -> Vec<ActionItem> {
        vec![
            ActionItem {
                label: "Rerun".into(),
                disabled: None,
                action: MenuAction::Rerun { id: "t1".into() },
            },
            ActionItem {
                label: "Skip".into(),
                disabled: Some("cannot skip a running task".into()),
                action: MenuAction::Skip { id: "t1".into() },
            },
        ]
    }

    fn draw(cols: u16, rows: u16, index: usize) -> (String, HitMap) {
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| render_menu(f, &mut hit, "do the thing", &items(), index)).unwrap();
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
    fn disabled_row_shows_reason() {
        let (s, _hit) = draw(80, 20, 0);
        assert!(s.contains("Rerun"));
        assert!(s.contains("Skip — cannot skip a running task"));
    }

    #[test]
    fn hit_targets_cover_rows_and_modal_body() {
        let (_s, hit) = draw(80, 20, 0);
        // Somewhere inside the popup a click resolves to a MenuItem; the popup
        // body is also covered by a Modal target so clicks never leak through.
        let mut saw_item0 = false;
        let mut saw_modal = false;
        for y in 0..20 {
            for x in 0..80 {
                match hit.hit(x, y) {
                    Some(HitTarget::MenuItem(0)) => saw_item0 = true,
                    Some(HitTarget::Modal) => saw_modal = true,
                    _ => {}
                }
            }
        }
        assert!(saw_item0, "expected a MenuItem(0) hit region");
        assert!(saw_modal, "expected a Modal body region");
    }

    #[test]
    fn menu_snapshot() {
        let (s, _hit) = draw(60, 15, 0);
        insta::assert_snapshot!("action_menu_open", s);
    }

    fn draw_confirm(cols: u16, rows: u16) -> (String, HitMap) {
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| render_confirm_remove(f, &mut hit, "platform.wt-a", "wt-a")).unwrap();
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
    fn confirm_remove_warns_and_registers_modal() {
        let (s, hit) = draw_confirm(80, 20);
        assert!(s.contains("discards uncommitted changes and deletes the local branch"));
        assert!(s.contains("platform.wt-a"));
        let mut saw_modal = false;
        for y in 0..20 {
            for x in 0..80 {
                if let Some(HitTarget::Modal) = hit.hit(x, y) {
                    saw_modal = true;
                }
            }
        }
        assert!(saw_modal, "confirm popup registers a Modal body region");
    }
}
