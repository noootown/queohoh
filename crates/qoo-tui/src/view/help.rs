use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::hit::{HitMap, HitTarget};
use crate::view::theme::Palette;

const HELP_ROWS: [(&str, &str); 13] = [
    ("Tab / Shift+Tab", "cycle focus between panes (incl. detail)"),
    ("1–9", "switch project tab"),
    ("[ / ]", "previous / next project tab"),
    ("{ / }", "previous / next detail sub-tab"),
    ("j/k · arrows", "move cursor (detail: scroll)"),
    ("J/K · shift+↑↓", "extend selection"),
    ("Enter / a", "action menu for selection"),
    ("c", "create (queue: adhoc task · worktrees: worktree)"),
    ("/", "filter focused pane"),
    ("esc", "clear range → clear filter → close overlay"),
    ("g / G", "top / bottom (detail scroll or list jump)"),
    ("?", "this help"),
    ("q", "quit"),
];

pub fn render(frame: &mut ratatui::Frame, area: Rect, hits: &mut HitMap, p: &Palette) {
    let width = (area.width.saturating_sub(8)).clamp(20, 64);
    let height = (HELP_ROWS.len() as u16 + 3).min(area.height);
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
    lines.push(Line::from(Span::styled(" any key to close", p.dim_style())));
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    // Registered last → topmost: clicks inside never leak to the body.
    hits.push(popup, HitTarget::Modal);
}
