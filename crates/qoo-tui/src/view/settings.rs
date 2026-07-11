//! The `s` overlay: a read-only view of the daemon's model-alias table.
//!
//! WHY a pure `settings_rows` splits from `render`: the layout — which sections
//! appear, in what order, and the effective (defaults ⊕ global) merge — is the
//! interesting logic, and it is worth unit-testing without a terminal. `render`
//! is then a thin styler over those rows, and the only thing the snapshot pins.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::hit::{HitMap, HitTarget};
use crate::ipc::types::SettingsPayload;
use crate::view::theme::Palette;

/// Rows for the settings overlay: the effective global table first
/// (defaults ⊕ global, alias → id), then one section per overriding project
/// showing only its deltas. Pure, so the layout is unit-testable.
///
/// A "section header" row is any whose left cell does NOT begin with the
/// two-space indent — its right cell is the layer's `source` path, not an id.
/// `render` keys off that indent to style headers vs. alias rows.
pub(crate) fn settings_rows(p: &SettingsPayload) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut effective = p.models.defaults.clone();
    effective.extend(p.models.global.entries.clone());
    rows.push(("models (global)".into(), p.models.global.source.clone()));
    for (alias, id) in &effective {
        rows.push((format!("  {alias}"), id.clone()));
    }
    for proj in &p.models.projects {
        rows.push((format!("{} (overrides)", proj.repo), proj.source.clone()));
        for (alias, id) in &proj.entries {
            rows.push((format!("  {alias}"), id.clone()));
        }
    }
    rows
}

/// Two-space indent that marks an alias row (vs. a bold section header). Kept as
/// one constant so `settings_rows` and `render` can never drift.
const ALIAS_INDENT: &str = "  ";

/// Body lines for the overlay given the tri-state `App::settings`. Split from the
/// framing so the state→copy mapping is obvious: `None` = still loading,
/// `Some(None)` = unavailable, `Some(Some(_))` = the table.
fn body_lines(settings: &Option<Option<SettingsPayload>>, p: &Palette) -> Vec<Line<'static>> {
    match settings {
        // Fetch in flight (opened for the first time this session).
        None => vec![Line::from(Span::styled(" (loading settings…)", p.dim_style()))],
        // Failed / daemon predates the RPC.
        Some(None) => vec![Line::from(Span::styled(
            " (settings unavailable — daemon predates the settings RPC)",
            p.dim_style(),
        ))],
        Some(Some(payload)) => {
            let rows = settings_rows(payload);
            // Align alias → id: widest alias cell (indent included) sets the gap.
            let alias_w = rows
                .iter()
                .filter(|(left, _)| left.starts_with(ALIAS_INDENT))
                .map(|(left, _)| left.chars().count())
                .max()
                .unwrap_or(0);
            rows.into_iter()
                .map(|(left, right)| {
                    if left.starts_with(ALIAS_INDENT) {
                        // Alias row: alias in `fg`, resolved id in `meta`
                        // (info/teal is reserved for timestamps).
                        Line::from(vec![
                            Span::styled(
                                format!(" {left:<alias_w$}  "),
                                Style::default().fg(p.fg),
                            ),
                            Span::styled(right, Style::default().fg(p.meta)),
                        ])
                    } else {
                        // Section header: bold label, dim provenance path trailing.
                        let mut spans = vec![Span::styled(
                            format!(" {left}"),
                            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
                        )];
                        if !right.is_empty() {
                            spans.push(Span::styled(format!("  {right}"), p.dim_style()));
                        }
                        Line::from(spans)
                    }
                })
                .collect()
        }
    }
}

/// Draw the settings overlay, modeled on `help::render` (centered rounded block,
/// `Clear`ed backdrop, topmost `Modal` hit target so clicks inside never leak).
pub fn render(
    frame: &mut ratatui::Frame,
    area: Rect,
    hits: &mut HitMap,
    p: &Palette,
    settings: &Option<Option<SettingsPayload>>,
) {
    let mut lines = body_lines(settings, p);
    lines.push(Line::from(Span::styled(" any key to close", p.dim_style())));

    // Width: widen enough for `alias  claude-model-id` pairs but stay bounded,
    // matching help's clamp. Height: content + hint + borders, capped to area.
    let width = (area.width.saturating_sub(8)).clamp(20, 72);
    let height = (lines.len() as u16 + 2).min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect { x, y, width, height };
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent))
        .title(Span::styled(
            " settings — model aliases ",
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    // Registered last → topmost: clicks inside never leak to the body.
    hits.push(popup, HitTarget::Modal);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::{SettingsLayer, SettingsModels, SettingsProjectLayer};
    use std::collections::BTreeMap;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn defaults_only_lists_the_global_section() {
        let p = SettingsPayload {
            models: SettingsModels {
                defaults: map(&[("opus", "claude-opus-4-8"), ("sonnet", "claude-sonnet-4-5")]),
                global: SettingsLayer { entries: BTreeMap::new(), source: "/cfg.yaml".into() },
                projects: vec![],
            },
        };
        assert_eq!(
            settings_rows(&p),
            vec![
                ("models (global)".to_string(), "/cfg.yaml".to_string()),
                // BTreeMap → alphabetical, deterministic for the snapshot.
                ("  opus".to_string(), "claude-opus-4-8".to_string()),
                ("  sonnet".to_string(), "claude-sonnet-4-5".to_string()),
            ]
        );
    }

    #[test]
    fn global_overrides_a_default_in_the_effective_table() {
        let p = SettingsPayload {
            models: SettingsModels {
                defaults: map(&[("sonnet", "claude-sonnet-4-5")]),
                // global overlays defaults: sonnet is remapped in-place.
                global: SettingsLayer {
                    entries: map(&[("sonnet", "claude-sonnet-4-6")]),
                    source: "/cfg.yaml".into(),
                },
                projects: vec![],
            },
        };
        let rows = settings_rows(&p);
        assert_eq!(rows[0], ("models (global)".to_string(), "/cfg.yaml".to_string()));
        // The overridden value wins; there is exactly one `sonnet` row.
        assert_eq!(rows[1], ("  sonnet".to_string(), "claude-sonnet-4-6".to_string()));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn project_delta_appends_its_own_section() {
        let p = SettingsPayload {
            models: SettingsModels {
                defaults: map(&[("opus", "claude-opus-4-8")]),
                global: SettingsLayer { entries: BTreeMap::new(), source: "/cfg.yaml".into() },
                projects: vec![SettingsProjectLayer {
                    repo: "acme".into(),
                    entries: map(&[("opus", "claude-opus-4-9")]),
                    source: "/repos/acme/vars.yaml".into(),
                }],
            },
        };
        assert_eq!(
            settings_rows(&p),
            vec![
                ("models (global)".to_string(), "/cfg.yaml".to_string()),
                ("  opus".to_string(), "claude-opus-4-8".to_string()),
                // The project section shows only its deltas — not the merged view.
                ("acme (overrides)".to_string(), "/repos/acme/vars.yaml".to_string()),
                ("  opus".to_string(), "claude-opus-4-9".to_string()),
            ]
        );
    }
}
