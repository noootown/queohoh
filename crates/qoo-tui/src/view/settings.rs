//! The `,` overlay: a read-only view of the daemon's provider + model settings.
//!
//! Post-Task-5 the daemon dropped the per-provider alias table for a flat
//! `catalog` plus an `active_provider` and `default_models` block, so this
//! overlay reads THOSE (the legacy `models` block a modern daemon no longer
//! sends would render empty). It surfaces: the configured providers with their
//! enabled state (the active one marked — `p` switches it), the merged model
//! catalog (`label (provider)` → concrete id, hidden entries marked), and the
//! effective default-model chains (global + per-project overrides).
//!
//! WHY a pure `settings_rows` splits from `render`: the layout — which sections
//! appear and in what order — is the interesting logic, worth unit-testing
//! without a terminal. `render` is then a thin styler over those rows, and the
//! only thing the snapshot pins.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::hit::{HitMap, HitTarget};
use crate::ipc::types::SettingsPayload;
use crate::view::modal::{render_back_button, MODAL_PADDING};
use crate::view::theme::Palette;

/// Rows for the settings overlay, in three sections: `providers` (precedence
/// order, enabled state, the active one marked), `catalog` (`label (provider)` →
/// concrete id, hidden marked), and `default models` (the global chain, then one
/// section per project override). Pure, so the layout is unit-testable.
///
/// A "section header" row is any whose left cell does NOT begin with the
/// two-space indent — its right cell is a provenance path (or empty). `render`
/// keys off that indent to style headers vs. value rows.
pub(crate) fn settings_rows(p: &SettingsPayload) -> Vec<(String, String)> {
    let mut rows = Vec::new();

    // Providers, in the payload's precedence order, with enabled state; the
    // active provider (what `p` switches) is flagged so the overlay names it.
    rows.push(("providers".into(), String::new()));
    for pr in &p.providers {
        let mut state = if pr.enabled { "enabled".to_string() } else { "disabled".to_string() };
        if pr.name == p.active_provider {
            state.push_str(" · active");
        }
        rows.push((format!("  {}", pr.name), state));
    }

    // The merged catalog: reference display `label (provider)` → concrete CLI
    // model id. Hidden entries (picker-suppressed but still resolvable when named
    // explicitly) are listed and marked.
    rows.push(("catalog".into(), String::new()));
    for e in &p.catalog {
        let id = if e.hidden { format!("{} · hidden", e.id) } else { e.id.clone() };
        rows.push((format!("  {}", e.model_display()), id));
    }

    // Effective default-model chains: the workspace-wide global chain, then one
    // labeled section per project that overrides it (source path trailing).
    rows.push(("default models (global)".into(), String::new()));
    rows.push((format!("  {}", chain_or_none(&p.default_models.global)), String::new()));
    for proj in &p.default_models.projects {
        rows.push((format!("{} (default models override)", proj.name), proj.source.clone()));
        rows.push((format!("  {}", chain_or_none(&proj.default_models)), String::new()));
    }

    rows
}

/// A ` → `-joined model-ref chain, or a placeholder when the chain is empty
/// (the daemon then resolves its own built-in default).
fn chain_or_none(refs: &[String]) -> String {
    if refs.is_empty() {
        "(daemon default)".into()
    } else {
        refs.join(" → ")
    }
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
    let lines = body_lines(settings, p);

    // Width: widen enough for `alias  claude-model-id` pairs but stay bounded,
    // matching help's clamp. Height: content + blank + [ Back ] + border(2) +
    // padding(2), capped to area.
    let width = (area.width.saturating_sub(8)).clamp(20, 72);
    let height = ((lines.len() + 2) as u16 + 4).min(area.height);
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
            " settings — providers & models ",
            Style::default().fg(p.fg).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::{
        CatalogEntry, DefaultModels, DefaultModelsProject, SettingsProvider,
    };

    fn provider(name: &str, enabled: bool) -> SettingsProvider {
        SettingsProvider { name: name.into(), enabled, bin: None }
    }

    fn entry(provider: &str, id: &str, label: &str, hidden: bool) -> CatalogEntry {
        CatalogEntry { provider: provider.into(), id: id.into(), label: label.into(), hidden }
    }

    #[test]
    fn providers_section_lists_enabled_state_and_marks_the_active_one() {
        let p = SettingsPayload {
            active_provider: "grok".into(),
            providers: vec![
                provider("claude", true),
                provider("grok", true),
                provider("codex", false),
            ],
            ..Default::default()
        };
        let rows = settings_rows(&p);
        // Header + one row per provider, in precedence order; active marked.
        assert_eq!(rows[0], ("providers".to_string(), String::new()));
        assert_eq!(rows[1], ("  claude".to_string(), "enabled".to_string()));
        assert_eq!(rows[2], ("  grok".to_string(), "enabled · active".to_string()));
        assert_eq!(rows[3], ("  codex".to_string(), "disabled".to_string()));
    }

    #[test]
    fn catalog_section_shows_ref_display_and_marks_hidden_entries() {
        let p = SettingsPayload {
            catalog: vec![
                entry("claude", "claude-opus-4-8", "claude-opus-4.8", false),
                entry("grok", "grok-legacy", "legacy", true),
            ],
            ..Default::default()
        };
        let rows = settings_rows(&p);
        // The catalog header follows the (empty) providers section header.
        let cat = rows.iter().position(|(l, _)| l == "catalog").unwrap();
        assert_eq!(rows[cat + 1], ("  claude-opus-4.8 (claude)".to_string(), "claude-opus-4-8".to_string()));
        // Hidden entries are listed and flagged.
        assert_eq!(rows[cat + 2], ("  legacy (grok)".to_string(), "grok-legacy · hidden".to_string()));
    }

    #[test]
    fn default_models_shows_global_chain_then_project_overrides() {
        let p = SettingsPayload {
            default_models: DefaultModels {
                global: vec!["claude/claude-opus-4.8".into(), "grok/grok-4.5".into()],
                projects: vec![DefaultModelsProject {
                    name: "acme".into(),
                    default_models: vec!["grok/grok-4.5".into()],
                    source: "/repos/acme/vars.yaml".into(),
                }],
            },
            ..Default::default()
        };
        let rows = settings_rows(&p);
        let g = rows.iter().position(|(l, _)| l == "default models (global)").unwrap();
        // Global chain joined with the fallback arrow.
        assert_eq!(rows[g + 1], ("  claude/claude-opus-4.8 → grok/grok-4.5".to_string(), String::new()));
        // Project override: labeled header with source path, then its chain.
        assert_eq!(
            rows[g + 2],
            ("acme (default models override)".to_string(), "/repos/acme/vars.yaml".to_string())
        );
        assert_eq!(rows[g + 3], ("  grok/grok-4.5".to_string(), String::new()));
    }

    #[test]
    fn empty_default_models_chain_renders_the_daemon_default_placeholder() {
        let p = SettingsPayload::default();
        let rows = settings_rows(&p);
        let g = rows.iter().position(|(l, _)| l == "default models (global)").unwrap();
        assert_eq!(rows[g + 1], ("  (daemon default)".to_string(), String::new()));
    }
}
