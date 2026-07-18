use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::hit::{HitMap, HitTarget};
use crate::view::Computed;
use crate::view::theme::{GLYPH_DOT, Palette};

/// A per-provider accent for the `↯ <provider>` indicator, reusing the theme's
/// existing color slots so it re-themes with the rest of the UI. A provider name
/// beyond the known set falls back to the generic metadata color.
fn provider_style(name: &str, p: &Palette) -> Style {
    let color = match name {
        "claude" => p.accent,
        "grok" => p.mauve,
        "codex" => p.info,
        _ => p.meta,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

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
    // PER PROJECT, so the fraction is the active project's running count over
    // its own `max_concurrent` cap: `running 1/10`. (The global running total
    // — `snap.running.len()`, across all projects — has no cap, so it only
    // stands in as a bare fallback when there is no active project or an old
    // daemon omits `max_concurrent`.)
    let snap = app.snapshot.as_ref();
    let running = snap.map(|s| s.running.len()).unwrap_or(0);
    let max = snap.and_then(|s| s.max_concurrent);
    let run_label = match (snap, c.active_name.as_deref(), max) {
        (Some(s), Some(repo), Some(m)) => {
            let n = crate::selectors::running_count_for(s, repo);
            format!(" running {}/{}", n, m)
        }
        _ => format!(" running {}", running),
    };
    let conn: Span = if app.connected {
        Span::styled(GLYPH_DOT.to_string(), Style::default().fg(p.ok))
    } else {
        Span::styled("daemon unreachable — retrying…", Style::default().fg(p.warn))
    };
    // The always-visible active-provider indicator sits at the far right edge
    // (`↯ <provider>`), styled per provider. It reads the broadcast-reconciled
    // active provider (snapshot, else the cached settings copy); absent only when
    // neither is known (pre-connect / old daemon). Its hit rect (registered
    // below) makes a click cycle the provider, mirroring the `p` key.
    let mut right_spans = vec![conn, Span::styled(run_label, Style::default().fg(p.fg))];
    let mut prov_w = 0u16;
    if let Some(name) = app.active_provider() {
        // ↯ (U+21AF) rather than ⚡: the emoji bolt is width-ambiguous — buffer
        // and terminal fonts disagree on 1 vs 2 cells (phantom gap or clipped
        // name, depending on the font), and terminals ignore U+FE0E on it.
        // U+21AF is a plain width-1 arrow everywhere.
        let span = Span::styled(format!("  ↯ {name}"), provider_style(&name, p));
        prov_w = span.width() as u16;
        right_spans.push(span);
    }
    let right = Line::from(right_spans);
    let width = area.width; // right-align via Paragraph alignment
    frame.render_widget(
        Paragraph::new(right).alignment(ratatui::layout::Alignment::Right),
        Rect { x: area.x, y: area.y, width, height: 1 },
    );
    // The indicator is the rightmost span, so a right-aligned line ends it at
    // `area.right()`. Register its click rect there (clamped to the header).
    if prov_w > 0 && prov_w <= area.width {
        hits.push(
            Rect { x: area.right() - prov_w, y: area.y, width: prov_w, height: 1 },
            HitTarget::ProviderIndicator,
        );
    }
}
