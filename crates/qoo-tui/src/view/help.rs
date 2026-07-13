use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::hit::{HitMap, HitTarget};
use crate::view::modal::{render_back_button, MODAL_PADDING};
use crate::view::theme::Palette;

const HELP_ROWS: [(&str, &str); 21] = [
    ("Tab / Shift+Tab", "cycle focus: queue → tasks → worktrees"),
    ("1–9 / 0", "switch project tab (0 = 10th)"),
    ("ctrl+s then n/p", "next / previous project tab"),
    ("arrows", "move list cursor (shift: extend selection)"),
    ("j / k", "detail: move lane-task row · else scroll"),
    ("h / l", "detail: previous / next sub-tab"),
    ("ctrl+x / ctrl+z", "detail sub-tab (alias of l / h)"),
    ("enter", "open selected lane task (worktrees)"),
    ("a", "action menu (queue: resume)"),
    ("r", "run: new task on worktree (worktrees) · re-queue (queue) · run def (tasks)"),
    ("g", "goto: open worktree in tmux (worktrees)"),
    ("x", "cancel (queue) · remove worktree (worktrees)"),
    ("c", "create (queue: adhoc task · worktrees: worktree)"),
    ("t", "task menu (worktrees)"),
    ("z", "collapse / expand focused list pane"),
    ("/", "filter focused pane"),
    ("esc", "clear range → clear filter → close overlay"),
    ("Home/End", "detail pane top / bottom"),
    ("s", "settings — model table"),
    ("?", "this help"),
    ("q", "quit"),
];

/// Glyph legend shown under the keymap (the WORKTREES columns especially are
/// dense with markers; this is their one written explanation).
const LEGEND_ROWS: [(&str, &str); 6] = [
    ("● ✗ ⊘ ⊝", "task done / failed / cancelled / skipped"),
    ("○ ‼ ▶", "queued / needs-input / running"),
    ("● / N", "worktree: green dot idle · yellow N = running + queued tasks"),
    ("⏱", "elapsed time of the running task on that lane"),
    ("±", "worktree has uncommitted changes"),
    ("ian · 3d ago", "last commit author · last commit age"),
];

pub fn render(frame: &mut ratatui::Frame, area: Rect, hits: &mut HitMap, p: &Palette) {
    let width = (area.width.saturating_sub(8)).clamp(20, 64);
    // content rows + blank separator + [ Back ] row + border(2) + padding(2).
    let content_rows = HELP_ROWS.len() + 2 + LEGEND_ROWS.len();
    let height = ((content_rows + 2) as u16 + 4).min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect { x, y, width, height };
    frame.render_widget(Clear, popup);
    // Opaque body region FIRST so the [ Back ] button (pushed last) stays topmost.
    hits.push(popup, HitTarget::Modal);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent))
        .padding(MODAL_PADDING)
        .title(Span::styled(
            " keymap ",
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let mut lines: Vec<Line> = HELP_ROWS
        .iter()
        .map(|(keys, what)| {
            Line::from(vec![
                Span::styled(format!(" {keys:<16}"), Style::default().fg(p.accent)),
                Span::styled((*what).to_string(), Style::default().fg(p.fg)),
            ])
        })
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " legend",
        Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
    )));
    for (glyphs, what) in LEGEND_ROWS.iter() {
        lines.push(Line::from(vec![
            Span::styled(format!(" {glyphs:<16}"), Style::default().fg(p.meta)),
            Span::styled((*what).to_string(), Style::default().fg(p.fg)),
        ]));
    }
    // Content fills all but the bottom two interior rows (a blank gap + the
    // [ Back ] button). Esc / any key / a Back click closes (handled in update).
    let content = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.saturating_sub(2),
    };
    frame.render_widget(Paragraph::new(Text::from(lines)), content);
    let btn_y = inner.y + inner.height.saturating_sub(1);
    render_back_button(frame, hits, Rect { x: inner.x, y: btn_y, width: inner.width, height: 1 }, p);
}
