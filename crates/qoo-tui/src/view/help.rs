use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::hit::{HitMap, HitTarget};
use crate::view::theme::Palette;

const HELP_ROWS: [(&str, &str); 15] = [
    ("Tab / Shift+Tab", "cycle focus: queue → tasks → worktrees"),
    ("1–9 / 0", "switch project tab (0 = 10th)"),
    ("ctrl+s then n/p", "next / previous project tab"),
    ("ctrl+x / ctrl+z", "next / previous detail sub-tab"),
    ("j/k · arrows", "move cursor"),
    ("J/K · shift+↑↓", "extend selection"),
    ("Enter / a", "tasks: run def · queue/worktrees: action menu"),
    ("c", "create (queue: adhoc task · worktrees: worktree)"),
    ("t", "task menu — run a task definition"),
    ("z", "collapse / expand focused list pane"),
    ("/", "filter focused pane"),
    ("esc", "clear range → clear filter → close overlay"),
    ("g / G", "jump to top / bottom"),
    ("?", "this help"),
    ("q", "quit"),
];

/// Glyph legend shown under the keymap (the WORKTREES columns especially are
/// dense with markers; this is their one written explanation).
const LEGEND_ROWS: [(&str, &str); 6] = [
    ("✓ ✗ ○ ?", "task done / failed / queued / needs input"),
    ("● (colored)", "worktree: green free · yellow busy · red failed"),
    ("⏱", "elapsed time of the running task on that lane"),
    ("⌂", "lane has a main session (tasks can resume it)"),
    ("±", "worktree has uncommitted changes"),
    ("koshea · 3d ago", "last commit author · last commit age"),
];

pub fn render(frame: &mut ratatui::Frame, area: Rect, hits: &mut HitMap, p: &Palette) {
    let width = (area.width.saturating_sub(8)).clamp(20, 64);
    // keymap rows + blank + "legend" heading + legend rows + hint + borders.
    let height =
        ((HELP_ROWS.len() + 2 + LEGEND_ROWS.len()) as u16 + 3).min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect { x, y, width, height };
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent))
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
            Span::styled(format!(" {glyphs:<16}"), Style::default().fg(p.info)),
            Span::styled((*what).to_string(), Style::default().fg(p.fg)),
        ]));
    }
    lines.push(Line::from(Span::styled(" any key to close", p.dim_style())));
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    // Registered last → topmost: clicks inside never leak to the body.
    hits.push(popup, HitTarget::Modal);
}
