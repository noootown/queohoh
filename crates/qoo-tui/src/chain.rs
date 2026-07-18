//! Model fallback-chain resolution — pure Rust port of
//! `packages/core/src/models.ts` `resolveModelChain` (+ the
//! `findModel` / `groupHead` / `unknownModelError` helpers from
//! `packages/core/src/catalog.ts` it depends on).
//!
//! TUI model display under the operator's `active_provider` (stable re-head +
//! default-model prepend), not the authored yaml list: the TASKS Model column
//! shows the **effective head** only; the definition config row shows the
//! **full resolved chain** — so switching the active provider reorders /
//! re-heads without rewriting every definition.

use crate::ipc::types::{CatalogEntry, ModelRef};

/// One step in a model fallback chain: which provider to spawn, which
/// provider-specific model id to pass it, and the canonical `provider/label`
/// ref that produced it (for display / attempted-provider bookkeeping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainEntry {
    pub provider: String,
    pub model_id: String,
    pub model_ref: String,
}

/// Look up a `provider/label` (or `provider/id` exact-match fallback) ref
/// within its named provider group only. Hidden entries still match — hidden
/// is picker-only. Mirrors `findModel` in packages/core/src/catalog.ts.
///
/// After exact label/id miss, two short-form fallbacks keep pre-versioned
/// refs resolvable so a catalog label rename does not blank the TASKS Model
/// column for configs still using the older form:
/// 1. `label.ends_with("-" + rest)` — e.g. `claude/sonnet-5` → `claude-sonnet-5`
/// 2. pure-alphabetic `rest` matching a hyphen segment of a label — e.g.
///    `claude/opus` → `claude-opus-4.8`, `claude/sonnet` → `claude-sonnet-5`
/// Group order is most→least powerful, so the first match is the current top
/// of that family. Never crosses provider groups.
pub fn find_model<'a>(catalog: &'a [CatalogEntry], r#ref: &str) -> Option<&'a CatalogEntry> {
    let slash = r#ref.find('/')?;
    let provider = &r#ref[..slash];
    let rest = &r#ref[slash + 1..];
    let group: Vec<&CatalogEntry> = catalog.iter().filter(|e| e.provider == provider).collect();
    if let Some(e) = group.iter().copied().find(|e| e.label == rest) {
        return Some(e);
    }
    if let Some(e) = group.iter().copied().find(|e| e.id == rest) {
        return Some(e);
    }
    // Suffix form: rest is a trailing portion of a versioned label (must
    // contain a letter so pure-digit tails like "5" do not accidental-match).
    if rest.chars().any(|c| c.is_ascii_alphabetic()) {
        let suffix = format!("-{rest}");
        if let Some(e) = group.iter().copied().find(|e| e.label.ends_with(&suffix)) {
            return Some(e);
        }
    }
    // Family-token form: pure alphabetic short name (opus/sonnet/haiku/…).
    if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphabetic()) {
        return group
            .into_iter()
            .find(|e| e.label.split('-').any(|seg| seg == rest));
    }
    None
}

/// First entry of a provider's group ("a provider's most powerful model"),
/// or `None` if the provider has no entries. Mirrors `groupHead`.
pub fn group_head<'a>(catalog: &'a [CatalogEntry], provider: &str) -> Option<&'a CatalogEntry> {
    catalog.iter().find(|e| e.provider == provider)
}

/// Display form for a `provider/label` model ref: just the `label`, since the
/// provider prefix is redundant (`grok/grok-4.5` → `grok-4.5`). The single
/// source of truth for rendering a ref anywhere in the TUI. Falls back to the
/// full `provider/label` ONLY when the same label exists under two or more
/// providers in `catalog` (so the render stays unambiguous — catalog labels are
/// guaranteed unique only within a provider). A ref with no `/` is returned
/// unchanged.
pub fn model_ref_display(catalog: &[CatalogEntry], r#ref: &str) -> String {
    let Some(slash) = r#ref.find('/') else {
        return r#ref.to_string();
    };
    let label = &r#ref[slash + 1..];
    let ambiguous = catalog.iter().filter(|e| e.label == label).count() >= 2;
    if ambiguous {
        r#ref.to_string()
    } else {
        label.to_string()
    }
}

/// Build the `unknown model: <ref>` error, with a `did you mean
/// provider/label?` suggestion when the part after `/` (or the whole ref
/// when there is no `/`) matches some entry's label or id. Mirrors
/// `unknownModelError`.
pub fn unknown_model_error(catalog: &[CatalogEntry], r#ref: &str) -> String {
    let part = match r#ref.find('/') {
        Some(i) => &r#ref[i + 1..],
        None => r#ref,
    };
    let match_entry = catalog
        .iter()
        .find(|e| e.label == part || e.id == part);
    match match_entry {
        Some(e) => format!("unknown model: {ref} (did you mean {}?)", e.model_ref()),
        None => format!("unknown model: {ref}"),
    }
}

fn is_enabled(enabled_providers: &[&str], provider: &str) -> bool {
    enabled_providers.contains(&provider)
}

fn to_chain_entry(entry: &CatalogEntry) -> ChainEntry {
    ChainEntry {
        provider: entry.provider.clone(),
        model_id: entry.id.clone(),
        model_ref: entry.model_ref(),
    }
}

/// Resolve a model spec into an ordered fallback chain over `catalog`.
///
/// Algorithm (design spec Section 4, mirrored from packages/core resolveModelChain):
/// 1. `refs = spec is None ? default_models : model_ref.refs()`.
/// 2. Map each ref via `find_model`; any miss ⇒ `unknown_model_error`.
/// 3. Drop entries whose provider is not in `enabled_providers`.
/// 4. Stable-partition: entries with `provider == active_provider` first
///    (keeping order), rest after.
/// 5. If no entry has `provider == active_provider` AND that provider is
///    enabled: prepend the active provider's `default_models` entry (its
///    chosen default from the pool), falling back to
///    `group_head(catalog, active_provider)` when `default_models` names no
///    model for that provider (skip if neither exists).
/// 6. Dedup by `provider/id` keeping first occurrence. Empty final chain ⇒
///    an error.
pub fn resolve_model_chain(
    spec: Option<&ModelRef>,
    catalog: &[CatalogEntry],
    enabled_providers: &[&str],
    default_models: &[String],
    active_provider: &str,
) -> Result<Vec<ChainEntry>, String> {
    let owned_refs: Vec<String> = match spec {
        None => default_models.to_vec(),
        Some(m) => m.refs(),
    };

    let mut entries: Vec<&CatalogEntry> = Vec::with_capacity(owned_refs.len());
    for r in &owned_refs {
        match find_model(catalog, r) {
            Some(e) => entries.push(e),
            None => return Err(unknown_model_error(catalog, r)),
        }
    }

    let enabled: Vec<&CatalogEntry> = entries
        .into_iter()
        .filter(|e| is_enabled(enabled_providers, &e.provider))
        .collect();
    let active: Vec<&CatalogEntry> = enabled
        .iter()
        .copied()
        .filter(|e| e.provider == active_provider)
        .collect();
    let rest: Vec<&CatalogEntry> = enabled
        .iter()
        .copied()
        .filter(|e| e.provider != active_provider)
        .collect();
    let mut ordered: Vec<&CatalogEntry> = active.iter().copied().chain(rest).collect();

    if active.is_empty() && is_enabled(enabled_providers, active_provider) {
        // Inject the active provider's DEFAULT from the pool (its `default_models`
        // entry), NOT its most-powerful group head; fall back to the group head
        // only when `default_models` names no model for the active provider.
        let injected = default_models
            .iter()
            .filter_map(|r| find_model(catalog, r))
            .find(|e| e.provider == active_provider)
            .or_else(|| group_head(catalog, active_provider));
        if let Some(head) = injected {
            ordered.insert(0, head);
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut chain = Vec::new();
    for entry in ordered {
        let key = format!("{}/{}", entry.provider, entry.id);
        if !seen.insert(key) {
            continue;
        }
        chain.push(to_chain_entry(entry));
    }

    if chain.is_empty() {
        return Err(
            "no runnable model: all configured models are on disabled providers".into(),
        );
    }
    Ok(chain)
}

/// The first chain entry's canonical `provider/label` ref, or `None` when
/// resolution fails (unknown model / nothing runnable). Used by pickers and
/// the TASKS Model column (head-only); the detail config row uses
/// [`resolved_model_chain_display`] for the full ordered chain.
pub fn effective_model_head(
    spec: Option<&ModelRef>,
    catalog: &[CatalogEntry],
    enabled_providers: &[&str],
    default_models: &[String],
    active_provider: &str,
) -> Option<String> {
    resolve_model_chain(spec, catalog, enabled_providers, default_models, active_provider)
        .ok()
        .and_then(|c| c.into_iter().next())
        .map(|e| e.model_ref)
}

/// Format a resolved chain for TUI display: each entry label-only via
/// [`model_ref_display`], joined with ` → ` (the fallback-order arrow used by
/// settings default-models and [`ModelRef::display`]). Single-entry chains
/// render as one label with no arrow.
pub fn format_chain_display(catalog: &[CatalogEntry], chain: &[ChainEntry]) -> String {
    chain
        .iter()
        .map(|e| model_ref_display(catalog, &e.model_ref))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Full resolved-chain display under `active_provider` (active-provider
/// entries first, then the rest, labels versioned via the catalog), or
/// `None` when resolution fails. Unlike [`effective_model_head`], returns
/// every walk-order entry — not just the head.
pub fn resolved_model_chain_display(
    spec: Option<&ModelRef>,
    catalog: &[CatalogEntry],
    enabled_providers: &[&str],
    default_models: &[String],
    active_provider: &str,
) -> Option<String> {
    resolve_model_chain(spec, catalog, enabled_providers, default_models, active_provider)
        .ok()
        .filter(|c| !c.is_empty())
        .map(|c| format_chain_display(catalog, &c))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirror of `BUILTIN_CATALOG` in packages/core/src/catalog.ts (incl. codex
    /// so the disabled-provider cases exercise a third group).
    fn builtin_catalog() -> Vec<CatalogEntry> {
        let e = |provider: &str, id: &str, label: &str| CatalogEntry {
            provider: provider.into(),
            id: id.into(),
            label: label.into(),
            hidden: false,
        };
        vec![
            e("claude", "claude-fable-5", "claude-fable-5"),
            e("claude", "claude-opus-4-8", "claude-opus-4.8"),
            e("claude", "claude-sonnet-5", "claude-sonnet-5"),
            e("claude", "claude-haiku-4-5", "claude-haiku-4.5"),
            e("grok", "grok-4.5", "grok-4.5"),
            e("grok", "grok-composer-2.5-fast", "grok-composer-2.5-fast"),
            e("codex", "gpt-5.6-sol", "gpt-5.6-sol"),
            e("codex", "gpt-5.6-terra", "gpt-5.6-terra"),
            e("codex", "gpt-5.6-luna", "gpt-5.6-luna"),
        ]
    }

    /// claude+grok enabled, codex disabled — mirrors the TS PROVIDERS fixture.
    const ENABLED: &[&str] = &["claude", "grok"];

    fn entry(provider: &str, model_id: &str, model_ref: &str) -> ChainEntry {
        ChainEntry {
            provider: provider.into(),
            model_id: model_id.into(),
            model_ref: model_ref.into(),
        }
    }

    #[test]
    fn model_ref_display_drops_provider_prefix_unless_ambiguous() {
        let cat = builtin_catalog();
        // Unique label → label-only.
        assert_eq!(model_ref_display(&cat, "grok/grok-4.5"), "grok-4.5");
        assert_eq!(model_ref_display(&cat, "claude/claude-opus-4.8"), "claude-opus-4.8");
        // No `/` → returned unchanged.
        assert_eq!(model_ref_display(&cat, "claude-opus-4.8"), "claude-opus-4.8");
        // Same label under two providers → keep the qualified form for it only.
        let mut ambiguous = builtin_catalog();
        ambiguous.push(CatalogEntry {
            provider: "grok".into(),
            id: "grok-opus".into(),
            label: "claude-opus-4.8".into(),
            hidden: false,
        });
        assert_eq!(model_ref_display(&ambiguous, "claude/claude-opus-4.8"), "claude/claude-opus-4.8");
        assert_eq!(model_ref_display(&ambiguous, "grok/claude-opus-4.8"), "grok/claude-opus-4.8");
        // A non-colliding label in the same catalog still strips.
        assert_eq!(model_ref_display(&ambiguous, "grok/grok-4.5"), "grok-4.5");
    }

    #[test]
    fn find_model_resolves_short_family_tokens_and_suffix_forms() {
        let cat = builtin_catalog();
        // Pure-alphabetic family tokens from the pre-versioned label era.
        assert_eq!(
            find_model(&cat, "claude/opus").map(|e| e.label.as_str()),
            Some("claude-opus-4.8")
        );
        assert_eq!(
            find_model(&cat, "claude/sonnet").map(|e| e.label.as_str()),
            Some("claude-sonnet-5")
        );
        assert_eq!(
            find_model(&cat, "claude/haiku").map(|e| e.label.as_str()),
            Some("claude-haiku-4.5")
        );
        // Intermediate suffix form (label gained a provider prefix).
        assert_eq!(
            find_model(&cat, "claude/sonnet-5").map(|e| e.label.as_str()),
            Some("claude-sonnet-5")
        );
        assert_eq!(
            find_model(&cat, "claude/opus-4.8").map(|e| e.label.as_str()),
            Some("claude-opus-4.8")
        );
        // Still provider-scoped; pure digit must not accidental-match.
        assert!(find_model(&cat, "grok/opus").is_none());
        assert!(find_model(&cat, "claude/5").is_none());
        assert!(find_model(&cat, "claude/nonexistent").is_none());
    }

    #[test]
    fn short_family_token_chain_displays_versioned_label() {
        // Def authored `claude/sonnet` (stale short form) + active claude →
        // full chain uses the CANONICAL versioned labels (not bare `sonnet`).
        let cat = builtin_catalog();
        let spec = ModelRef::Many(vec!["claude/sonnet".into(), "grok/grok-4.5".into()]);
        assert_eq!(
            effective_model_head(Some(&spec), &cat, ENABLED, &[], "claude").as_deref(),
            Some("claude/claude-sonnet-5")
        );
        assert_eq!(
            resolved_model_chain_display(Some(&spec), &cat, ENABLED, &[], "claude").as_deref(),
            Some("claude-sonnet-5 → grok-4.5")
        );
        // Stale default_models: [claude/opus, grok/grok-4.5] still resolve.
        let defaults = vec!["claude/opus".to_string(), "grok/grok-4.5".to_string()];
        assert_eq!(
            effective_model_head(None, &cat, ENABLED, &defaults, "claude").as_deref(),
            Some("claude/claude-opus-4.8")
        );
        assert_eq!(
            resolved_model_chain_display(None, &cat, ENABLED, &defaults, "claude").as_deref(),
            Some("claude-opus-4.8 → grok-4.5")
        );
    }

    #[test]
    fn null_spec_uses_default_models() {
        let cat = builtin_catalog();
        let defaults = vec!["claude/claude-sonnet-5".to_string()];
        assert_eq!(
            resolve_model_chain(None, &cat, ENABLED, &defaults, "claude").unwrap(),
            vec![entry("claude", "claude-sonnet-5", "claude/claude-sonnet-5")]
        );
    }

    #[test]
    fn string_spec_resolves_to_one_entry_chain() {
        let cat = builtin_catalog();
        let spec = ModelRef::One("claude/claude-opus-4.8".into());
        assert_eq!(
            resolve_model_chain(Some(&spec), &cat, ENABLED, &[], "claude").unwrap(),
            vec![entry("claude", "claude-opus-4-8", "claude/claude-opus-4.8")]
        );
    }

    #[test]
    fn list_spec_keeps_given_order_when_already_active() {
        let cat = builtin_catalog();
        let spec = ModelRef::Many(vec!["claude/claude-sonnet-5".into(), "claude/claude-haiku-4.5".into()]);
        assert_eq!(
            resolve_model_chain(Some(&spec), &cat, ENABLED, &[], "claude").unwrap(),
            vec![
                entry("claude", "claude-sonnet-5", "claude/claude-sonnet-5"),
                entry("claude", "claude-haiku-4-5", "claude/claude-haiku-4.5"),
            ]
        );
    }

    #[test]
    fn canonicalizes_provider_id_form_ref_to_provider_label() {
        let cat = builtin_catalog();
        let spec = ModelRef::One("claude/claude-opus-4-8".into());
        assert_eq!(
            resolve_model_chain(Some(&spec), &cat, ENABLED, &[], "claude").unwrap(),
            vec![entry("claude", "claude-opus-4-8", "claude/claude-opus-4.8")]
        );
    }

    #[test]
    fn unknown_ref_produces_catalog_unknown_model_error() {
        let cat = builtin_catalog();
        let spec = ModelRef::One("claude/nonexistent".into());
        assert_eq!(
            resolve_model_chain(Some(&spec), &cat, ENABLED, &[], "claude").unwrap_err(),
            unknown_model_error(&cat, "claude/nonexistent")
        );
    }

    #[test]
    fn drops_entries_whose_provider_is_disabled() {
        let cat = builtin_catalog();
        let spec = ModelRef::Many(vec!["codex/gpt-5.6-sol".into(), "claude/claude-opus-4.8".into()]);
        assert_eq!(
            resolve_model_chain(Some(&spec), &cat, ENABLED, &[], "claude").unwrap(),
            vec![entry("claude", "claude-opus-4-8", "claude/claude-opus-4.8")]
        );
    }

    #[test]
    fn stable_partitions_active_provider_entries_first() {
        // [claude/claude-opus-4.8, grok/grok-4.5] + active grok → head grok/grok-4.5
        let cat = builtin_catalog();
        let spec = ModelRef::Many(vec!["claude/claude-opus-4.8".into(), "grok/grok-4.5".into()]);
        assert_eq!(
            resolve_model_chain(Some(&spec), &cat, ENABLED, &[], "grok").unwrap(),
            vec![
                entry("grok", "grok-4.5", "grok/grok-4.5"),
                entry("claude", "claude-opus-4-8", "claude/claude-opus-4.8"),
            ]
        );
        assert_eq!(
            effective_model_head(Some(&spec), &cat, ENABLED, &[], "grok").as_deref(),
            Some("grok/grok-4.5")
        );
    }

    #[test]
    fn switch_miss_injects_active_provider_default_not_group_head() {
        // [grok/grok-4.5] + active claude, pool = [claude/claude-opus-4.8, grok/grok-4.5] →
        // inject claude's DEFAULT (opus), not claude's group head (fable).
        let cat = builtin_catalog();
        let spec = ModelRef::One("grok/grok-4.5".into());
        let defaults = vec!["claude/claude-opus-4.8".to_string(), "grok/grok-4.5".to_string()];
        assert_eq!(
            resolve_model_chain(Some(&spec), &cat, ENABLED, &defaults, "claude").unwrap(),
            vec![
                entry("claude", "claude-opus-4-8", "claude/claude-opus-4.8"),
                entry("grok", "grok-4.5", "grok/grok-4.5"),
            ]
        );
    }

    #[test]
    fn switch_miss_falls_back_to_group_head_when_no_default_for_active() {
        // [grok/grok-4.5] + active claude, pool has only a grok entry → fall back
        // to claude's group head (fable).
        let cat = builtin_catalog();
        let spec = ModelRef::One("grok/grok-4.5".into());
        let defaults = vec!["grok/grok-4.5".to_string()];
        assert_eq!(
            resolve_model_chain(Some(&spec), &cat, ENABLED, &defaults, "claude").unwrap(),
            vec![
                entry("claude", "claude-fable-5", "claude/claude-fable-5"),
                entry("grok", "grok-4.5", "grok/grok-4.5"),
            ]
        );
    }

    #[test]
    fn switch_miss_empty_defaults_falls_back_to_group_head() {
        // [claude/claude-opus-4.8] + active grok, empty pool → group-head fallback grok-4.5.
        let cat = builtin_catalog();
        let spec = ModelRef::One("claude/claude-opus-4.8".into());
        assert_eq!(
            resolve_model_chain(Some(&spec), &cat, ENABLED, &[], "grok").unwrap(),
            vec![
                entry("grok", "grok-4.5", "grok/grok-4.5"),
                entry("claude", "claude-opus-4-8", "claude/claude-opus-4.8"),
            ]
        );
        assert_eq!(
            effective_model_head(Some(&spec), &cat, ENABLED, &[], "grok").as_deref(),
            Some("grok/grok-4.5")
        );
    }

    #[test]
    fn switch_miss_does_not_prepend_when_active_provider_disabled() {
        let cat = builtin_catalog();
        let spec = ModelRef::One("claude/claude-opus-4.8".into());
        assert_eq!(
            resolve_model_chain(Some(&spec), &cat, ENABLED, &[], "codex").unwrap(),
            vec![entry("claude", "claude-opus-4-8", "claude/claude-opus-4.8")]
        );
    }

    #[test]
    fn dedups_by_provider_id_keeping_first() {
        let cat = builtin_catalog();
        let spec = ModelRef::Many(vec!["claude/claude-opus-4.8".into(), "claude/claude-opus-4.8".into()]);
        assert_eq!(
            resolve_model_chain(Some(&spec), &cat, ENABLED, &[], "claude").unwrap(),
            vec![entry("claude", "claude-opus-4-8", "claude/claude-opus-4.8")]
        );
    }

    #[test]
    fn all_disabled_yields_no_runnable_model_error() {
        let cat = builtin_catalog();
        let spec = ModelRef::One("codex/gpt-5.6-sol".into());
        assert_eq!(
            resolve_model_chain(Some(&spec), &cat, ENABLED, &[], "codex").unwrap_err(),
            "no runnable model: all configured models are on disabled providers"
        );
        assert_eq!(
            effective_model_head(Some(&spec), &cat, ENABLED, &[], "codex"),
            None
        );
    }

    #[test]
    fn null_spec_reheads_defaults_under_active_provider() {
        // defaults = [claude/claude-opus-4.8] (no grok entry), active = grok → group-head
        // fallback grok/grok-4.5.
        let cat = builtin_catalog();
        let defaults = vec!["claude/claude-opus-4.8".to_string()];
        assert_eq!(
            effective_model_head(None, &cat, ENABLED, &defaults, "grok").as_deref(),
            Some("grok/grok-4.5")
        );
        // Full-chain display lists every entry (re-headed), not head alone.
        assert_eq!(
            resolved_model_chain_display(None, &cat, ENABLED, &defaults, "grok").as_deref(),
            Some("grok-4.5 → claude-opus-4.8")
        );
    }

    #[test]
    fn resolved_model_chain_display_lists_full_reheaded_chain() {
        let cat = builtin_catalog();
        let spec = ModelRef::Many(vec!["claude/claude-opus-4.8".into(), "grok/grok-4.5".into()]);
        // Active grok → grok first, then claude; versioned labels, no bare `opus`.
        assert_eq!(
            resolved_model_chain_display(Some(&spec), &cat, ENABLED, &[], "grok").as_deref(),
            Some("grok-4.5 → claude-opus-4.8")
        );
        // Active claude → authored order preserved (already active-first).
        assert_eq!(
            resolved_model_chain_display(Some(&spec), &cat, ENABLED, &[], "claude").as_deref(),
            Some("claude-opus-4.8 → grok-4.5")
        );
        // Single authored model under matching active → one label, no arrow.
        let one = ModelRef::One("claude/claude-opus-4.8".into());
        assert_eq!(
            resolved_model_chain_display(Some(&one), &cat, ENABLED, &[], "claude").as_deref(),
            Some("claude-opus-4.8")
        );
    }
}
