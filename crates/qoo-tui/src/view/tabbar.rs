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
        // Scheduled (queued) + running count for the project, e.g.
        // `1:platform (2)`; a quiet project keeps the bare chip.
        let active = app
            .snapshot
            .as_ref()
            .map(|s| crate::selectors::active_count_for(s, name))
            .unwrap_or(0);
        let label = if active > 0 {
            format!(" {}:{} ({}) ", i + 1, name, active)
        } else {
            format!(" {}:{} ", i + 1, name)
        };
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

    // Right: connection indicator + running counter, right-aligned. The cap is
    // PER PROJECT, so the denominator belongs to the active project, not the
    // global running total. Show the global total (no denominator — there is no
    // global cap) plus the active project's usage against its cap:
    // `running 3 · acme 1/10`. Fall back to a bare total when there is no active
    // project or an old daemon omits `max_concurrent`.
    let snap = app.snapshot.as_ref();
    let running = snap.map(|s| s.running.len()).unwrap_or(0);
    let max = snap.and_then(|s| s.max_concurrent);
    let run_label = match (snap, c.active_name.as_deref(), max) {
        (Some(s), Some(repo), Some(m)) => {
            let n = crate::selectors::running_count_for(s, repo);
            format!(" running {} · {} {}/{}", running, repo, n, m)
        }
        _ => format!(" running {}", running),
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
