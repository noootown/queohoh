use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::hit::{HitMap, HitTarget};
use crate::view::Computed;
use crate::view::theme::GLYPH_DOT;

pub fn render(app: &App, c: &Computed, frame: &mut ratatui::Frame, area: Rect, hits: &mut HitMap) {
    let p = &c.palette;
    // Left: tab chips. Track x as we lay them out so each chip gets a hit rect.
    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;
    for (i, name) in c.tab_names.iter().enumerate() {
        let label = format!(" {}:{} ", i + 1, name);
        let w = label.chars().count() as u16;
        let style = if i == c.active_index {
            Style::default()
                .fg(p.selection_fg)
                .bg(p.selection_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.fg)
        };
        // Clamp the hit rect to the header width.
        if x < area.right() {
            let clamped_w = w.min(area.right() - x);
            hits.push(
                Rect { x, y: area.y, width: clamped_w, height: 1 },
                HitTarget::Tab(i),
            );
        }
        spans.push(Span::styled(label, style));
        x = x.saturating_add(w);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    // Right: connection indicator + running counter, right-aligned.
    let running = app.snapshot.as_ref().map(|s| s.running.len()).unwrap_or(0);
    let max = app.snapshot.as_ref().and_then(|s| s.max_concurrent);
    let run_label = match max {
        Some(m) => format!(" running {}/{}", running, m),
        None => format!(" running {}", running),
    };
    let conn: Span = if app.connected {
        Span::styled(GLYPH_DOT.to_string(), Style::default().fg(p.ok))
    } else {
        Span::styled("daemon unreachable — retrying…", Style::default().fg(p.warn))
    };
    let right = Line::from(vec![conn, Span::styled(run_label, Style::default().fg(p.fg))]);
    let width = area.width; // right-align via Paragraph alignment
    frame.render_widget(
        Paragraph::new(right).alignment(ratatui::layout::Alignment::Right),
        Rect { x: area.x, y: area.y, width, height: 1 },
    );
}
