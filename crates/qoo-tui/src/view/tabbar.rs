use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::hit::{HitMap, HitTarget};
use crate::ipc::types::{ProviderUsage, UsageSeverity};
use crate::view::Computed;
use crate::view::theme::{GLYPH_DOT, Palette};

/// A per-provider accent for the active `↯ <provider>` indicator, reusing the
/// theme's existing color slots so it re-themes with the rest of the UI. A
/// provider name beyond the known set falls back to the generic metadata color.
fn provider_style(name: &str, p: &Palette) -> Style {
    let color = match name {
        "claude" => p.accent,
        "grok" => p.mauve,
        "codex" => p.info,
        _ => p.meta,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// Severity color for an active provider's usage text. Stale samples add DIM
/// so a last-good chip reads quieter without disappearing.
fn usage_style(u: &ProviderUsage, p: &Palette) -> Style {
    let mut style = Style::default().fg(match u.severity {
        UsageSeverity::Ok => p.ok,
        UsageSeverity::Warn => p.warn,
        UsageSeverity::Crit => p.error,
        UsageSeverity::Unknown => p.meta,
    });
    if u.stale {
        style = style.add_modifier(Modifier::DIM);
    }
    style
}

/// Enabled provider names for the top-bar chips, in config-precedence order.
///
/// Source order (first hit wins):
/// 1. Snapshot `enabled_providers` — the daemon's live `config.providers`
///    with `enabled: true` only (authoritative; disabled names never appear).
/// 2. Settings payload's enabled providers — old daemon that omits the
///    snapshot field but still serves the settings RPC.
/// 3. Empty — caller falls back to the single active provider so the header
///    still shows something pre-connect / pre-settings.
fn enabled_provider_names(app: &App) -> Vec<String> {
    if let Some(names) = app
        .snapshot
        .as_ref()
        .and_then(|s| s.enabled_providers.as_ref())
    {
        // Empty vec is intentional (every provider disabled) — do not fall
        // through to settings and re-introduce a disabled name.
        return names.clone();
    }
    app.settings
        .as_ref()
        .and_then(|s| s.as_ref())
        .map(|payload| {
            payload
                .providers
                .iter()
                .filter(|pr| pr.enabled)
                .map(|pr| pr.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Usage samples keyed by provider name. Prefers the multi-chip
/// `provider_usages` array; falls back to the single-chip `provider_usage`
/// field so an older daemon still feeds the active chip.
fn usage_by_provider(app: &App) -> std::collections::HashMap<String, &ProviderUsage> {
    let mut map = std::collections::HashMap::new();
    let Some(snap) = app.snapshot.as_ref() else {
        return map;
    };
    if let Some(us) = snap.provider_usages.as_ref() {
        for u in us {
            map.insert(u.provider.clone(), u);
        }
    } else if let Some(u) = snap.provider_usage.as_ref() {
        map.insert(u.provider.clone(), u);
    }
    map
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

    // Provider chips: every ENABLED provider from the workspace config
    // (snapshot.enabled_providers), each with its usage sample when the
    // poller has one. Disabled providers never appear. Active = provider
    // accent + severity-colored usage; inactive = grey (`p.dim`) for both
    // name and usage so the active one pops. Click the cluster to open the
    // Switch-provider form (mirrors the `p` key).
    //
    // ↯ (U+21AF) rather than ⚡: the emoji bolt is width-ambiguous — buffer
    // and terminal fonts disagree on 1 vs 2 cells (phantom gap or clipped
    // name, depending on the font), and terminals ignore U+FE0E on it.
    // U+21AF is a plain width-1 arrow everywhere.
    let active_name = app.active_provider();
    let mut names = enabled_provider_names(app);
    if names.is_empty() {
        // Settings not loaded yet: still show the active provider alone so the
        // header isn't blank pre-FetchSettings.
        if let Some(n) = active_name.clone() {
            names.push(n);
        }
    }
    let usages = usage_by_provider(app);

    let mut right_spans = vec![conn, Span::styled(run_label, Style::default().fg(p.fg))];
    let mut cluster_w = 0u16;
    for name in &names {
        let is_active = active_name.as_deref() == Some(name.as_str());
        // Leading double-space separates chips from each other / the run label.
        let prov_span = if is_active {
            Span::styled(format!("  ↯ {name}"), provider_style(name, p))
        } else {
            // Inactive: grey (not Modifier::DIM — theme notes grey-on-grey
            // with DIM is unreadable; `p.dim` is the dedicated de-emphasis slot).
            Span::styled(
                format!("  ↯ {name}"),
                Style::default().fg(p.dim),
            )
        };
        cluster_w = cluster_w.saturating_add(prov_span.width() as u16);
        right_spans.push(prov_span);

        if let Some(u) = usages.get(name.as_str()) {
            let style = if is_active {
                usage_style(u, p)
            } else {
                Style::default().fg(p.dim)
            };
            let usage_span = Span::styled(format!(" {}", u.text), style);
            cluster_w = cluster_w.saturating_add(usage_span.width() as u16);
            right_spans.push(usage_span);
        }
    }

    let right = Line::from(right_spans);
    let width = area.width; // right-align via Paragraph alignment
    frame.render_widget(
        Paragraph::new(right).alignment(ratatui::layout::Alignment::Right),
        Rect { x: area.x, y: area.y, width, height: 1 },
    );
    // Right-aligned cluster ends at `area.right()`. One hit rect over the whole
    // provider+usage strip so a click opens the Switch-provider form (same as `p`).
    if cluster_w > 0 && cluster_w <= area.width {
        hits.push(
            Rect {
                x: area.right() - cluster_w,
                y: area.y,
                width: cluster_w,
                height: 1,
            },
            HitTarget::ProviderIndicator,
        );
    }
}
