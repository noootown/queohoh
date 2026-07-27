//! Key + mouse handling for the reusable bordered form ([`Mode::Form`]).
//!
//! Mirrors `def_args` in shape: `form_key` drives the keyboard (Tab focus,
//! text edits, the inline dropdown, and the explicit-commit Primary/Cancel
//! buttons) and `form_click` routes a left-click onto the form's hit targets
//! (`FormField`/`DropdownItem`/`Button`). The Primary button validates the
//! [`FormState`] and, on success, fires the frozen [`FormAction`] via
//! `fire_form_action` (the New-session enqueue / Create-worktree create+enqueue
//! flows land in Phase 5).

use super::*;
use crate::chain::{effective_model_head, resolve_model_chain};
use crate::ipc::types::{CatalogEntry, DefaultModels, ModelRef};
use crate::selectors::ModelResolveOwned;
use crate::view::form::{DropdownOption, Field, FieldKind, FocusKind, FormState};

/// Hardcoded mirror of `BUILTIN_CATALOG` in packages/core/src/catalog.ts — the
/// model picker's fallback when the cached `settings` payload has no catalog (an
/// old daemon, or settings not fetched yet). **Keep in sync with that file.**
/// codex is omitted deliberately: it ships disabled-by-default, so it never
/// appears in a picker anyway (a disabled provider is filtered out regardless).
pub(super) fn builtin_catalog() -> Vec<CatalogEntry> {
    let mk = |provider: &str, id: &str, label: &str, hidden: bool| CatalogEntry {
        provider: provider.into(),
        id: id.into(),
        label: label.into(),
        hidden,
    };
    let e = |provider: &str, id: &str, label: &str| mk(provider, id, label, false);
    vec![
        e("claude", "claude-fable-5", "claude-fable-5"),
        e("claude", "claude-opus-4-8", "claude-opus-4.8"),
        e("claude", "claude-sonnet-5", "claude-sonnet-5"),
        e("claude", "claude-haiku-4-5", "claude-haiku-4.5"),
        e("grok", "grok-4.5", "grok-4.5"),
        // Hidden from pickers (grok group offers only grok-4.5); still resolves
        // when referenced explicitly. Mirrors catalog.ts's `hidden: true`.
        mk("grok", "grok-composer-2.5-fast", "grok-composer-2.5-fast", true),
    ]
}

/// Title-case a provider id for the re-run dropdown (`grok` → `Grok`).
pub(super) fn title_case_provider(provider: &str) -> String {
    let mut chars = provider.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => provider.to_string(),
    }
}

/// Provider segment of a `provider/label` ref (or the whole string if bare).
fn provider_of_ref(model_ref: &str) -> &str {
    model_ref.split_once('/').map(|(p, _)| p).unwrap_or(model_ref)
}

/// The dropdown's head-option display label: `default (<refs joined with " → ">)`,
/// or the bare `default` when there are no refs to show. Used by the ad-hoc /
/// new-session catalog picker ([`App::model_field`]); refs come from the repo's
/// `default_models` and carry no marker (`default (claude/claude-opus-4.8)`). The head
/// option's stored VALUE is always the empty string (= leave `model` unset →
/// the daemon resolves the chain).
///
/// Def-run launch uses [`App::def_model_field`] instead (effective chain, no
/// empty head). `from_def = true` remains available for a `def: ` marker in the
/// label if a future surface wants the old "default (def: …)" wording.
pub(super) fn default_head_label(refs: &[String], from_def: bool) -> String {
    if refs.is_empty() {
        return "default".into();
    }
    let marker = if from_def { "def: " } else { "" };
    format!("default ({marker}{})", refs.join(" → "))
}

impl App {
    /// The effective model catalog: the cached `settings` payload's `catalog`
    /// when present and non-empty, else the built-in mirror ([`builtin_catalog`]).
    /// Hidden entries and disabled providers are still included here — the picker
    /// ([`Self::visible_model_options`]) filters them. `pub(crate)` so the run-info
    /// detail pane (`view::detail`) can resolve a run's raw model id to its
    /// `label (provider)` display without duplicating the settings/builtin
    /// fallback logic.
    pub(crate) fn model_catalog(&self) -> Vec<CatalogEntry> {
        self.settings
            .as_ref()
            .and_then(|s| s.as_ref())
            .map(|p| p.catalog.clone())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(builtin_catalog)
    }

    /// Provider names the payload marks `enabled: false`. Empty when settings are
    /// absent (nothing to filter → the built-in fallback shows all its groups).
    fn disabled_providers(&self) -> std::collections::HashSet<String> {
        self.settings
            .as_ref()
            .and_then(|s| s.as_ref())
            .map(|p| {
                p.providers
                    .iter()
                    .filter(|pr| !pr.enabled)
                    .map(|pr| pr.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The catalog entries the model picker offers, in catalog order: hidden
    /// entries and entries whose provider is disabled are filtered out.
    fn visible_model_options(&self) -> Vec<CatalogEntry> {
        let disabled = self.disabled_providers();
        self.model_catalog()
            .into_iter()
            .filter(|e| !e.hidden && !disabled.contains(&e.provider))
            .collect()
    }

    /// Owned model-chain resolution inputs for the TASKS Model column (catalog,
    /// enabled providers, default_models, active_provider). Settings absent or
    /// an empty `providers` list → every catalog provider treated as enabled
    /// (picker parity: empty providers means nothing is disabled, so the model
    /// column must not blank while the dropdown still lists the catalog).
    /// `pub(crate)` so the TASKS pane layout/render share one source.
    pub(crate) fn model_resolve_owned(&self) -> ModelResolveOwned {
        let catalog = self.model_catalog();
        let catalog_providers = || {
            let mut seen = std::collections::HashSet::new();
            let mut enabled = Vec::new();
            for e in &catalog {
                if seen.insert(e.provider.clone()) {
                    enabled.push(e.provider.clone());
                }
            }
            enabled
        };
        let (enabled_providers, default_models) =
            match self.settings.as_ref().and_then(|s| s.as_ref()) {
                Some(p) if !p.providers.is_empty() => {
                    let enabled = p
                        .providers
                        .iter()
                        .filter(|pr| pr.enabled)
                        .map(|pr| pr.name.clone())
                        .collect();
                    (enabled, p.default_models.clone())
                }
                Some(p) => {
                    // Payload present but providers empty (old daemon / wire
                    // default) — same as settings-absent for enabled set.
                    (catalog_providers(), p.default_models.clone())
                }
                None => (catalog_providers(), DefaultModels::default()),
            };
        ModelResolveOwned {
            catalog,
            enabled_providers,
            default_models,
            active_provider: self.active_provider().unwrap_or_default(),
        }
    }

    /// Every visible catalog entry as a dropdown option (`provider/label` value,
    /// `label (provider)` display), reordered so the ACTIVE provider's group
    /// leads and the other providers follow — each group in catalog order
    /// (stable). This is the shared "provider-first" body both the new-session
    /// and def-run pickers list below their respective head option.
    fn provider_first_model_options(&self) -> Vec<DropdownOption> {
        let active = self.active_provider().unwrap_or_default();
        let (mut active_group, mut rest): (Vec<DropdownOption>, Vec<DropdownOption>) =
            (Vec::new(), Vec::new());
        for e in self.visible_model_options() {
            let opt = DropdownOption { value: e.model_ref(), label: e.model_display() };
            if e.provider == active {
                active_group.push(opt);
            } else {
                rest.push(opt);
            }
        }
        active_group.into_iter().chain(rest).collect()
    }

    /// The resolved head of `repo`'s DEFAULT chain — `resolveModelChain(null,
    /// …, active_provider)`'s `chain[0]` ref — as a one-element slice (empty
    /// when defaults resolve to nothing). Drives the new-session picker's
    /// `default (<resolved-head>)` head label: only the model the default
    /// actually resolves to under the active provider, not the whole authored
    /// chain.
    fn default_resolved_head_refs(&self, repo: &str) -> Vec<String> {
        let owned = self.model_resolve_owned();
        let defaults = owned.default_models.refs_for(repo);
        let enabled: Vec<&str> = owned.enabled_providers.iter().map(String::as_str).collect();
        effective_model_head(None, &owned.catalog, &enabled, &defaults, &owned.active_provider)
            // Render label-only (provider prefix dropped) via the shared helper,
            // so the head reads `default (grok-4.5)` not `default (grok/grok-4.5)`.
            .map(|r| vec![crate::chain::model_ref_display(&owned.catalog, &r)])
            .unwrap_or_default()
    }

    /// The full labeled option list for the new-session/adhoc model dropdown:
    /// the `default (<resolved-head>)` head (value `""`, label = the single
    /// model the repo's `default_models` resolve to under the active provider)
    /// followed by the provider-first full catalog ([`Self::provider_first_model_options`]).
    fn model_dropdown_options(&self, repo: &str) -> Vec<DropdownOption> {
        let head = DropdownOption {
            value: String::new(),
            label: default_head_label(&self.default_resolved_head_refs(repo), false),
        };
        std::iter::once(head)
            .chain(self.provider_first_model_options())
            .collect()
    }

    /// The model dropdown field, preselected to its head option (leave `model`
    /// unset → the daemon resolves the chain).
    pub(super) fn model_field(&self, repo: &str) -> Field {
        self.model_field_defaulting(repo, None)
    }

    /// The model dropdown field, preselected to `preferred` when it names a real
    /// catalog option (e.g. the `provider/label` ref a resumed session already
    /// ran on), else the head option (`""` = leave unset). `preferred` is
    /// validated against the visible option VALUES so a stale/foreign ref can't
    /// select a phantom option.
    pub(super) fn model_field_defaulting(&self, repo: &str, preferred: Option<&str>) -> Field {
        let options = self.model_dropdown_options(repo);
        let default = preferred
            .filter(|m| options.iter().any(|o| o.value == *m))
            .unwrap_or("");
        Field::dropdown_labeled("model", options, default)
    }

    /// Adhoc schedule form only: model options scoped to a session's provider.
    /// `provider = None` (New session / unknown) → full catalog with the default
    /// head. `Some("claude")` / `Some("grok")` → that provider's models only
    /// (no cross-provider default head — resuming a claude session with a grok
    /// model would be nonsense). Prefers `preferred` when it lands in the
    /// scoped list, else the first option (or `""` when the full-catalog head
    /// is present).
    pub(super) fn model_field_for_session(
        &self,
        repo: &str,
        provider: Option<&str>,
        preferred: Option<&str>,
    ) -> Field {
        let options: Vec<DropdownOption> = match provider {
            None => self.model_dropdown_options(repo),
            Some(p) => self
                .visible_model_options()
                .into_iter()
                .filter(|e| e.provider == p)
                .map(|e| DropdownOption {
                    value: e.model_ref(),
                    label: e.model_display(),
                })
                .collect(),
        };
        // Own the default string so we can move `options` into the field after.
        let default = preferred
            .filter(|m| options.iter().any(|o| o.value == *m))
            .map(|s| s.to_string())
            .or_else(|| {
                // Full catalog keeps the empty head ("default"); scoped lists
                // preselect the first real model so the field never holds a
                // value missing from its options.
                if options.iter().any(|o| o.value.is_empty()) {
                    Some(String::new())
                } else {
                    options.first().map(|o| o.value.clone())
                }
            })
            .unwrap_or_default();
        Field::dropdown_labeled("model", options, &default)
    }

    /// QUEUE re-run **provider** picker. One option per enabled provider that
    /// has a resolvable model for the first selected task, labeled
    /// `Grok (grok-4.5)` (provider title + the model that would be pinned).
    /// Value is the bare provider id; the concrete ref is re-derived per task
    /// on submit. Preselects `preferred_provider` (last-run / first task).
    /// No "Keep current" — the current provider is the default selection.
    pub(super) fn requeue_model_field(
        &self,
        task_ids: &[String],
        preferred_provider: Option<&str>,
    ) -> Field {
        let options = self.requeue_provider_options(task_ids);
        let default = preferred_provider
            .filter(|p| options.iter().any(|o| o.value == *p))
            .map(|s| s.to_string())
            .or_else(|| options.first().map(|o| o.value.clone()))
            .unwrap_or_default();
        Field::dropdown_labeled("provider", options, &default)
    }

    /// Enabled providers that the first selected task can resolve a model for.
    /// Label shows which model would be used (manifest entry or ad-hoc default).
    fn requeue_provider_options(&self, task_ids: &[String]) -> Vec<DropdownOption> {
        let owned = self.model_resolve_owned();
        let providers: Vec<String> = if owned.enabled_providers.is_empty() {
            let mut seen = std::collections::HashSet::new();
            owned
                .catalog
                .iter()
                .filter(|e| !e.hidden && seen.insert(e.provider.clone()))
                .map(|e| e.provider.clone())
                .collect()
        } else {
            owned.enabled_providers.clone()
        };
        let sample = self.requeue_sample_task(task_ids);
        providers
            .into_iter()
            .filter_map(|p| {
                let model_ref = sample
                    .as_ref()
                    .and_then(|t| self.resolve_requeue_model_for_provider(t, &p))
                    .or_else(|| {
                        // No sample task (shouldn't happen) — group head only.
                        crate::chain::group_head(&owned.catalog, &p).map(|e| e.model_ref())
                    })?;
                let model_label = crate::chain::model_ref_display(&owned.catalog, &model_ref);
                Some(DropdownOption {
                    value: p.clone(),
                    label: format!("{} ({model_label})", title_case_provider(&p)),
                })
            })
            .collect()
    }

    /// First selected task (live or archived) — drives dropdown labels and the
    /// default provider for bulk re-run.
    fn requeue_sample_task(
        &self,
        task_ids: &[String],
    ) -> Option<crate::ipc::types::TaskInstance> {
        let snap = self.snapshot.as_ref()?;
        for id in task_ids {
            if let Some(t) = snap
                .tasks
                .iter()
                .chain(snap.archived_recent.iter())
                .find(|t| t.id == *id)
            {
                return Some(t.clone());
            }
        }
        None
    }

    /// Preferred **provider** for the re-run form: last-run provider (what
    /// actually executed after fallback), else the stamp's first ref's provider.
    pub(super) fn requeue_preferred_model(&self, task_ids: &[String]) -> Option<String> {
        let catalog = self.model_catalog();
        let snap = self.snapshot.as_ref()?;
        for id in task_ids {
            if let Some((rid, files)) = self.run_files.as_ref() {
                if rid == id {
                    if let Some(p) = Self::provider_from_run_meta(files.meta.as_ref(), &catalog) {
                        return Some(p);
                    }
                }
            }
            if let Some(p) = self.read_run_provider(id, &catalog) {
                return Some(p);
            }
            let task = snap
                .tasks
                .iter()
                .chain(snap.archived_recent.iter())
                .find(|t| t.id == *id);
            if let Some(r) = task
                .and_then(|t| t.model.as_ref())
                .and_then(|m| m.refs().into_iter().next())
            {
                return Some(provider_of_ref(&r).to_string());
            }
        }
        None
    }

    /// Provider that last ran this task (`meta.provider`, or from catalog id).
    fn provider_from_run_meta(
        meta: Option<&crate::runfiles::RunMeta>,
        catalog: &[CatalogEntry],
    ) -> Option<String> {
        let meta = meta?;
        if let Some(p) = meta.provider.as_deref().filter(|s| !s.is_empty()) {
            return Some(p.to_string());
        }
        let model = meta.model.as_deref()?;
        if let Some(e) = catalog.iter().find(|e| e.id == model || e.label == model) {
            return Some(e.provider.clone());
        }
        if model.contains('/') {
            return Some(provider_of_ref(model).to_string());
        }
        None
    }

    fn read_run_provider(&self, task_id: &str, catalog: &[CatalogEntry]) -> Option<String> {
        let path = self.runs_dir.join(task_id).join("data.json");
        let text = std::fs::read_to_string(path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&text).ok()?;
        if !json.is_object() {
            return None;
        }
        let model = json
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let provider = json
            .get("provider")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let meta = crate::runfiles::RunMeta {
            model,
            provider,
            ..Default::default()
        };
        Self::provider_from_run_meta(Some(&meta), catalog)
    }

    /// Concrete pin ref for re-running `task` under `provider`.
    /// - Stamp lists a matching `provider/…` → that first match (task manifest).
    /// - Ad-hoc / null stamp → first repo `default_models` ref for the provider,
    ///   else catalog group head for that provider.
    /// - Stamp exists but has no entry for `provider` → `None` (keep stamp).
    pub(super) fn resolve_requeue_model_for_provider(
        &self,
        task: &crate::ipc::types::TaskInstance,
        provider: &str,
    ) -> Option<String> {
        if provider.is_empty() {
            return None;
        }
        let prefix = format!("{provider}/");
        if let Some(m) = task.model.as_ref() {
            return m
                .refs()
                .into_iter()
                .find(|r| r.starts_with(&prefix));
        }
        // Ad-hoc (or unstamped): default model for this provider.
        let owned = self.model_resolve_owned();
        let defaults = owned.default_models.refs_for(&task.target.repo);
        if let Some(r) = defaults.into_iter().find(|r| r.starts_with(&prefix)) {
            return Some(r);
        }
        crate::chain::group_head(&owned.catalog, provider).map(|e| e.model_ref())
    }

    /// Whether switching this task onto `provider` is allowed (manifest lists
    /// that provider, or the task is unstamped/ad-hoc).
    pub(super) fn task_allows_requeue_provider(
        task: &crate::ipc::types::TaskInstance,
        provider: &str,
    ) -> bool {
        if provider.is_empty() {
            return true;
        }
        match &task.model {
            None => true,
            Some(m) => {
                let prefix = format!("{provider}/");
                m.refs().iter().any(|r| r.starts_with(&prefix))
            }
        }
    }

    /// Def-run model picker: concrete `provider/label` options only (no empty
    /// `default (…)` row). Preselects the option that matches today's
    /// active-provider resolution head so the dropdown lands on "whatever
    /// would run now," not a separate default line.
    ///
    /// - Def authors `model:` → only those catalog entries, authored order.
    /// - Def omits `model:` → full visible catalog, provider-first.
    ///
    /// Submit always sends the selected ref as a hard pin (`model_pinned`);
    /// there is no unpinned empty head. Picking another entry overrides.
    pub(super) fn def_model_field(&self, repo: &str, def_model: Option<&ModelRef>) -> Field {
        let options = self.def_model_pin_options(def_model);
        let owned = self.model_resolve_owned();
        let defaults = owned.default_models.refs_for(repo);
        let enabled: Vec<&str> = owned.enabled_providers.iter().map(String::as_str).collect();
        let resolved_head = resolve_model_chain(
            def_model,
            &owned.catalog,
            &enabled,
            &defaults,
            &owned.active_provider,
        )
        .ok()
        .and_then(|c| c.into_iter().next())
        .map(|e| e.model_ref);
        let default = Self::def_model_preselect(&options, resolved_head.as_deref(), &owned.active_provider);
        Field::dropdown_labeled("model", options, &default)
    }

    /// Preselect among pin options: exact resolved-head ref if present, else
    /// first option for the active provider, else the first option.
    fn def_model_preselect(
        options: &[DropdownOption],
        resolved_head: Option<&str>,
        active_provider: &str,
    ) -> String {
        if let Some(r) = resolved_head {
            if options.iter().any(|o| o.value == r) {
                return r.to_string();
            }
        }
        if !active_provider.is_empty() {
            let prefix = format!("{active_provider}/");
            if let Some(o) = options.iter().find(|o| o.value.starts_with(&prefix)) {
                return o.value.clone();
            }
        }
        options
            .first()
            .map(|o| o.value.clone())
            .unwrap_or_default()
    }

    /// Pin options for the def-run model dropdown. When the def authors a
    /// `model:` list, only those refs appear (authored order, disabled providers
    /// dropped). When it does not — or every authored ref is unusable — fall
    /// back to the full visible catalog so the operator still has a pin list.
    fn def_model_pin_options(&self, def_model: Option<&ModelRef>) -> Vec<DropdownOption> {
        let Some(spec) = def_model else {
            return self.provider_first_model_options();
        };
        let catalog = self.model_catalog();
        let disabled = self.disabled_providers();
        let mut seen = std::collections::HashSet::new();
        let mut opts = Vec::new();
        for r in spec.refs() {
            let Some(e) = crate::chain::find_model(&catalog, &r) else {
                continue;
            };
            // Disabled providers are not runnable pins. Hidden entries still
            // show when the def names them (hidden is picker-only for the full
            // catalog, not for an explicit author choice).
            if disabled.contains(&e.provider) {
                continue;
            }
            let value = e.model_ref();
            if !seen.insert(value.clone()) {
                continue;
            }
            opts.push(DropdownOption {
                value,
                label: e.model_display(),
            });
        }
        if opts.is_empty() {
            return self.provider_first_model_options();
        }
        opts
    }

    /// The adhoc-create session field's display label: `New session` when no
    /// session is pinned, else `↻ <label>` (the session being continued).
    pub(super) fn adhoc_session_label(resume_label: Option<&str>) -> String {
        match resume_label {
            Some(l) => format!("↻ {l}"),
            None => "New session".into(),
        }
    }

    /// Clear a stale adhoc session pin when the target combobox is edited: the
    /// pinned session belongs to a specific worktree, so any change to the target
    /// invalidates it (and resets the session field back to "New session").
    /// No-op unless `action` is an `AdhocTask` currently carrying a pin.

    /// Open the unified adhoc-create form (`s` / Schedule on QUEUE, or `r` on
    /// WORKTREES with `lock_target`). Fields, in `adhoc_field` order: `[target,
    /// session, model, prompt]` — session sits above model so a chosen session
    /// can filter the model list. When `lock_target` is true the target is a
    /// readonly field fixed to `prefill_target` (WORKTREES run: worktree locked
    /// in); otherwise it's an editable combobox (QUEUE schedule). Prefills that
    /// name an existing worktree kick a `listSessions` fetch (returned as cmds).
    pub(super) fn open_adhoc_create(
        &mut self,
        repo: String,
        prefill_target: Option<String>,
        lock_target: bool,
    ) -> Vec<Cmd> {
        let rows = self.active_worktree_rows();
        let worktrees = Self::worktree_names(&rows);
        let aliases = crate::worktree_context::worktree_ref_aliases(&rows);
        let prefill = prefill_target.as_deref().unwrap_or("").trim().to_string();
        let prefetch = !prefill.is_empty() && worktrees.iter().any(|w| w == &prefill);
        // Locked target (WORKTREES [r]un): display-only so the operator can't
        // retarget; focus lands on session (first non-readonly). Unlocked
        // (QUEUE schedule): editable combobox, focus on target.
        let target_field = if lock_target && !prefill.is_empty() {
            Field::readonly("worktree", &prefill)
        } else {
            Field::combobox("worktree / PR / ticket", worktrees, &prefill)
        };
        let mut state = FormState::new(
            &format!("New task · {repo}"),
            "Enqueue",
            vec![
                target_field,
                Field::picker("session", &Self::adhoc_session_label(None)),
                self.model_field(&repo),
                Field::textarea("prompt", "", true),
            ],
        );
        // Wide layout matches the worktree session-picker width so target /
        // session rows have room for the "(new worktree)" suffix and provider tags.
        state.wide = true;
        state.ref_aliases = aliases;
        let mut cmds = Vec::new();
        if prefetch {
            state.sessions_for = Some(prefill.clone());
            state.sessions_loading = true;
            cmds.push(Cmd::FetchSessions {
                repo: repo.clone(),
                worktree: prefill,
            });
        }
        self.mode = Mode::Form {
            state,
            action: FormAction::AdhocTask {
                repo,
                resume_session_id: None,
                resume_label: None,
                resume_worktree: None,
            },
        };
        cmds
    }

    /// Kick off (or reuse) a `listSessions` fetch for `worktree` into the open
    /// adhoc form's session cache. No-op when the form is not open or the
    /// cache already covers this worktree (settled or in flight).
    fn adhoc_ensure_sessions(&mut self, worktree: &str) -> Vec<Cmd> {
        let Mode::Form {
            state,
            action: FormAction::AdhocTask { repo, .. },
        } = &mut self.mode
        else {
            return Vec::new();
        };
        if state.sessions_for.as_deref() == Some(worktree) {
            return Vec::new(); // settled or in-flight for this wt
        }
        let repo = repo.clone();
        state.sessions.clear();
        state.sessions_for = Some(worktree.to_string());
        state.sessions_loading = true;
        vec![Cmd::FetchSessions {
            repo,
            worktree: worktree.to_string(),
        }]
    }

    /// Activate the adhoc form's session field as an INLINE dropdown (the
    /// worktree-launcher session list, without Create Worktree): New session +
    /// loaded sessions with provider/age columns and a fixed id·datetime footer.
    /// When the target is not an existing worktree, leave a status hint — there
    /// are no sessions to continue on a brand-new worktree.
    pub(super) fn open_adhoc_session_pick(&mut self) -> Update {
        let worktrees = self.active_worktree_names();
        let target = match &self.mode {
            Mode::Form {
                state,
                action: FormAction::AdhocTask { .. },
            } => state
                .fields
                .get(crate::app::mode::adhoc_field::TARGET)
                .map(|f| f.value.trim().to_string())
                .unwrap_or_default(),
            _ => return Update { dirty: false, cmds: vec![] },
        };
        if target.is_empty() || !worktrees.contains(&target) {
            self.status_line = Some(
                "choose an existing worktree to continue a session (new worktrees start fresh)"
                    .into(),
            );
            return Update { dirty: true, cmds: vec![] };
        }
        let cmds = self.adhoc_ensure_sessions(&target);
        if let Mode::Form { state, .. } = &mut self.mode {
            state.focus_field(crate::app::mode::adhoc_field::SESSION);
            state.dropdown_open = true;
            state.dropdown_index = 0;
        }
        Update { dirty: true, cmds }
    }

    /// After the target combobox changes, clear a stale session pin, restore the
    /// full model catalog (pin gone → New session scope), and refresh the
    /// sessions cache when the new target is an existing worktree.
    pub(super) fn adhoc_on_target_changed(&mut self) -> Vec<Cmd> {
        let worktrees = self.active_worktree_names();
        let (repo, target, is_existing) = {
            let Mode::Form {
                state,
                action: FormAction::AdhocTask {
                    repo,
                    resume_session_id,
                    resume_label,
                    resume_worktree,
                    ..
                },
            } = &mut self.mode
            else {
                return Vec::new();
            };
            *resume_session_id = None;
            *resume_label = None;
            *resume_worktree = None;
            state.set_field_value(
                crate::app::mode::adhoc_field::SESSION,
                &Self::adhoc_session_label(None),
            );
            let target = state
                .fields
                .get(crate::app::mode::adhoc_field::TARGET)
                .map(|f| f.value.trim().to_string())
                .unwrap_or_default();
            let is_existing = !target.is_empty() && worktrees.iter().any(|w| w == &target);
            if !is_existing {
                state.sessions.clear();
                state.sessions_for = None;
                state.sessions_loading = false;
            }
            (repo.clone(), target, is_existing)
        };
        // Pin cleared → any model is choosable again.
        self.adhoc_apply_model_scope(&repo, None, None);
        if is_existing {
            self.adhoc_ensure_sessions(&target)
        } else {
            Vec::new()
        }
    }

    /// Pick the highlighted row of the open session dropdown (0 = New session,
    /// 1+ = `state.sessions[i-1]`). Pins the action, scopes the model dropdown
    /// to the session's provider (or restores the full catalog for New session),
    /// and closes the list.
    pub(super) fn adhoc_session_dropdown_pick(&mut self) -> Update {
        let (repo, provider, preferred) = {
            let Mode::Form {
                state,
                action: FormAction::AdhocTask {
                    repo,
                    resume_session_id,
                    resume_label,
                    resume_worktree,
                    ..
                },
            } = &mut self.mode
            else {
                return Update { dirty: false, cmds: vec![] };
            };
            let idx = state.dropdown_index;
            let target = state
                .fields
                .get(crate::app::mode::adhoc_field::TARGET)
                .map(|f| f.value.trim().to_string())
                .unwrap_or_default();
            let (provider, preferred) = if idx == 0 || state.sessions.is_empty() {
                *resume_session_id = None;
                *resume_label = None;
                *resume_worktree = None;
                state.set_field_value(
                    crate::app::mode::adhoc_field::SESSION,
                    &Self::adhoc_session_label(None),
                );
                (None, None)
            } else if let Some(s) = state.sessions.get(idx - 1).cloned() {
                let label = format!("↻ {}", s.label);
                let provider = s.provider.clone();
                let preferred = s.model.clone();
                *resume_session_id = Some(s.session_id.clone());
                *resume_label = Some(label.clone());
                *resume_worktree = Some(target);
                state.set_field_value(crate::app::mode::adhoc_field::SESSION, &label);
                (provider, preferred)
            } else {
                (None, None)
            };
            state.close_dropdown();
            (repo.clone(), provider, preferred)
        };
        self.adhoc_apply_model_scope(repo.as_str(), provider.as_deref(), preferred.as_deref());
        Update { dirty: true, cmds: vec![] }
    }

    /// Rebuild the adhoc form's model field for the chosen session scope.
    /// `provider = None` → full catalog (New session). Called after a session
    /// pick or when the target change clears a pin.
    fn adhoc_apply_model_scope(
        &mut self,
        repo: &str,
        provider: Option<&str>,
        preferred: Option<&str>,
    ) {
        let field = self.model_field_for_session(repo, provider, preferred);
        if let Mode::Form {
            state,
            action: FormAction::AdhocTask { .. },
        } = &mut self.mode
            && let Some(slot) = state.fields.get_mut(crate::app::mode::adhoc_field::MODEL)
        {
            *slot = field;
        }
    }

    /// Row count of the open session dropdown: 1 (New session) + sessions, or
    /// 1 + 1 loading placeholder when loading with empty cache.
    pub(super) fn adhoc_session_dropdown_len(state: &FormState) -> usize {
        if state.sessions_loading && state.sessions.is_empty() {
            1 // only New session selectable while loading
        } else {
            1 + state.sessions.len()
        }
    }

    /// `Mode::Form` key handling. Dropdown-open: ↑/↓ move the highlight, Enter
    /// picks, Esc closes the dropdown only. Dropdown-closed: Tab/Shift-Tab are
    /// the ONLY focus movers between fields and the bottom buttons (app-wide
    /// form standard); ↑/↓ open a focused dropdown or move the caret between
    /// lines in a focused textarea (never stepping focus, so multiline stays
    /// navigable); ←/→/Home/End/Backspace/printable edit the focused text
    /// field; Shift+Enter inserts a newline (textarea only). Plain Enter NEVER
    /// submits from a field (explicit-commit rule): it adds a newline in a
    /// textarea, advances focus from a single-line input, or opens a focused
    /// dropdown; only the Primary button submits. Cancel/Esc close.
    pub(super) fn form_key(&mut self, ev: &crossterm::event::KeyEvent) -> Update {
        use crossterm::event::{KeyCode::*, KeyModifiers};
        let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        // Session Picker (adhoc form): Enter opens an INLINE session list (or
        // picks when already open); Up/Down move the highlight; Esc closes the
        // list (or the form when closed); Tab moves focus. No separate modal.
        if matches!(&self.mode, Mode::Form { state, .. } if state.is_picker_focused()) {
            let sess_open = matches!(
                &self.mode,
                Mode::Form { state, .. } if state.dropdown_open
            );
            if sess_open {
                return match ev.code {
                    Esc => {
                        if let Mode::Form { state, .. } = &mut self.mode {
                            state.close_dropdown();
                        }
                        Update { dirty: true, cmds: vec![] }
                    }
                    Enter => self.adhoc_session_dropdown_pick(),
                    Up => {
                        if let Mode::Form { state, .. } = &mut self.mode {
                            let n = Self::adhoc_session_dropdown_len(state);
                            if n > 0 {
                                state.dropdown_index =
                                    (state.dropdown_index + n - 1) % n;
                            }
                        }
                        Update { dirty: true, cmds: vec![] }
                    }
                    Down => {
                        if let Mode::Form { state, .. } = &mut self.mode {
                            let n = Self::adhoc_session_dropdown_len(state);
                            if n > 0 {
                                state.dropdown_index =
                                    (state.dropdown_index + 1) % n;
                            }
                        }
                        Update { dirty: true, cmds: vec![] }
                    }
                    Tab if !shift => {
                        if let Mode::Form { state, .. } = &mut self.mode {
                            state.close_dropdown();
                            state.focus_next();
                        }
                        Update { dirty: true, cmds: vec![] }
                    }
                    BackTab | Tab => {
                        if let Mode::Form { state, .. } = &mut self.mode {
                            state.close_dropdown();
                            state.focus_prev();
                        }
                        Update { dirty: true, cmds: vec![] }
                    }
                    _ => Update { dirty: false, cmds: vec![] },
                };
            }
            return match ev.code {
                Enter | Down => self.open_adhoc_session_pick(),
                Esc => {
                    self.note_esc_dismiss();
                    self.mode = Mode::List;
                    Update { dirty: true, cmds: vec![] }
                }
                Tab if !shift => {
                    if let Mode::Form { state, .. } = &mut self.mode {
                        state.focus_next();
                    }
                    Update { dirty: true, cmds: vec![] }
                }
                BackTab | Tab => {
                    if let Mode::Form { state, .. } = &mut self.mode {
                        state.focus_prev();
                    }
                    Update { dirty: true, cmds: vec![] }
                }
                _ => Update { dirty: false, cmds: vec![] },
            };
        }
        // Combobox (adhoc target) — handle in a scoped block so the form
        // borrow ends before adhoc_on_target_changed / note_esc_dismiss.
        if matches!(&self.mode, Mode::Form { state, .. } if state.is_combobox_focused()) {
            let dropdown_open =
                matches!(&self.mode, Mode::Form { state, .. } if state.dropdown_open);
            let mut target_changed = false;
            let dirty = {
                let Mode::Form { state, .. } = &mut self.mode else {
                    return Update { dirty: false, cmds: vec![] };
                };
                match ev.code {
                    Esc if dropdown_open => {
                        state.close_dropdown();
                        true
                    }
                    Esc => false, // cancel form — handled after block
                    Enter if dropdown_open => {
                        state.dropdown_pick();
                        target_changed = true;
                        true
                    }
                    Enter => {
                        state.open_dropdown();
                        true
                    }
                    Up => {
                        if dropdown_open {
                            state.dropdown_move(-1);
                        } else {
                            state.open_dropdown();
                        }
                        true
                    }
                    Down => {
                        if dropdown_open {
                            state.dropdown_move(1);
                        } else {
                            state.open_dropdown();
                        }
                        true
                    }
                    Left => {
                        state.move_left();
                        true
                    }
                    Right => {
                        state.move_right();
                        true
                    }
                    Home => {
                        state.move_home();
                        true
                    }
                    End => {
                        state.move_end();
                        true
                    }
                    Tab if !shift => {
                        state.focus_next();
                        true
                    }
                    BackTab | Tab => {
                        state.focus_prev();
                        true
                    }
                    Backspace => {
                        state.backspace();
                        state.open_dropdown();
                        target_changed = true;
                        true
                    }
                    Char(c) if !ctrl && !alt => {
                        state.insert_char(c);
                        state.open_dropdown();
                        target_changed = true;
                        true
                    }
                    _ => false,
                }
            };
            if matches!(ev.code, Esc) && !dropdown_open {
                self.note_esc_dismiss();
                self.mode = Mode::List;
                return Update { dirty: true, cmds: vec![] };
            }
            if !dirty {
                return Update { dirty: false, cmds: vec![] };
            }
            let mut cmds = Vec::new();
            if target_changed {
                cmds.extend(self.adhoc_on_target_changed());
            }
            return Update { dirty: true, cmds };
        }
        let Mode::Form { state, action: _ } = &mut self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        let dropdown_open = state.dropdown_open;
        if dropdown_open {
            match ev.code {
                Up => { state.dropdown_move(-1); return Update { dirty: true, cmds: vec![] }; }
                Down => { state.dropdown_move(1); return Update { dirty: true, cmds: vec![] }; }
                Enter => { state.dropdown_pick(); return Update { dirty: true, cmds: vec![] }; }
                Esc => { state.close_dropdown(); return Update { dirty: true, cmds: vec![] }; }
                _ => return Update { dirty: false, cmds: vec![] },
            }
        }
        let is_dropdown = state.is_dropdown_focused();
        let fk = state.focus_kind();
        match ev.code {
            Esc => {
                self.note_esc_dismiss();
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
            // Newline chord first — must win over the plain-Enter submit arm; inert
            // on anything but a focused textarea.
            Enter if shift => { state.insert_newline(); Update { dirty: true, cmds: vec![] } }
            // Enter NEVER submits from a text field — only the Primary button
            // does (explicit-commit rule): a focused dropdown opens, a textarea
            // takes a newline, a single-line input advances focus, the buttons
            // fire. This is what stops "type something, hit Enter, everything
            // submits".
            Enter => match fk {
                FocusKind::Field(_) if is_dropdown => {
                    state.open_dropdown();
                    Update { dirty: true, cmds: vec![] }
                }
                FocusKind::Field(i) if matches!(state.fields[i].kind, FieldKind::Textarea) => {
                    state.insert_newline();
                    Update { dirty: true, cmds: vec![] }
                }
                FocusKind::Field(_) => {
                    state.focus_next();
                    Update { dirty: true, cmds: vec![] }
                }
                FocusKind::Primary => self.submit_form(),
                FocusKind::Cancel => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
            },
            // Tab/Shift-Tab are the ONLY focus movers between fields and the
            // bottom buttons — app-wide form standard. Arrow keys never change
            // focus (they'd hijack a textarea's line navigation).
            Tab if !shift => { state.focus_next(); Update { dirty: true, cmds: vec![] } }
            BackTab => { state.focus_prev(); Update { dirty: true, cmds: vec![] } }
            Tab if shift => { state.focus_prev(); Update { dirty: true, cmds: vec![] } }
            // ↑/↓ open a focused dropdown, move the caret between lines in a
            // focused textarea, and are otherwise inert — they NEVER step focus.
            Up => {
                if is_dropdown { state.open_dropdown(); } else { state.move_up(); }
                Update { dirty: true, cmds: vec![] }
            }
            Down => {
                if is_dropdown { state.open_dropdown(); } else { state.move_down(); }
                Update { dirty: true, cmds: vec![] }
            }
            Left => { state.move_left(); Update { dirty: true, cmds: vec![] } }
            Right => { state.move_right(); Update { dirty: true, cmds: vec![] } }
            Home => { state.move_home(); Update { dirty: true, cmds: vec![] } }
            End => { state.move_end(); Update { dirty: true, cmds: vec![] } }
            Backspace => { state.backspace(); Update { dirty: true, cmds: vec![] } }
            Char(c) if !ctrl && !alt => { state.insert_char(c); Update { dirty: true, cmds: vec![] } }
            _ => Update { dirty: false, cmds: vec![] },
        }
    }

    /// Validate the open form and, on success, fire its action; on the first
    /// empty required field keep the form open (the field is flagged via
    /// `error`, focus moved to it by `validate`).
    fn submit_form(&mut self) -> Update {
        let Mode::Form { state, action } = &mut self.mode else {
            return Update { dirty: false, cmds: vec![] };
        };
        let values = match state.validate() {
            Ok(v) => v,
            Err(_) => return Update { dirty: true, cmds: vec![] }, // required field flagged
        };
        // Action-specific secondary validation (e.g. branch-name syntax): keep the
        // form open and flag the offending field on failure.
        if let Some(bad) = Self::action_field_error(action, &values) {
            state.error = Some(bad);
            state.focus_field(bad);
            return Update { dirty: true, cmds: vec![] };
        }
        let action = action.clone();
        self.mode = Mode::List;
        self.fire_form_action(action, values)
    }

    /// Field-level validation beyond required-empty, keyed on the action. For a
    /// Create Worktree the branch/name field (index 1, after the leading model
    /// dropdown) must be a valid git branch name. Returns the failing field
    /// index, or `None` when the values pass.
    fn action_field_error(action: &FormAction, values: &[String]) -> Option<usize> {
        match action {
            FormAction::CreateWorktree { .. } => {
                let name = values.get(1).map(String::as_str).unwrap_or("");
                crate::worktree_context::validate_branch(name).map(|_| 1)
            }
            // The adhoc target combobox accepts a worktree name, a PR/ticket, or
            // empty (temp) — `resolve_target_ref` normalizes all three, so no
            // secondary field validation is needed. The provider dropdown is
            // always one of its own options, so it can't fail either.
            FormAction::NewSession { .. }
            | FormAction::AdhocTask { .. }
            | FormAction::GotoProvider { .. }
            | FormAction::SwitchProvider
            | FormAction::Requeue { .. } => None,
        }
    }

    /// Route a left-click while the form is open: a `DropdownItem` picks it, a
    /// `FormField` focuses (a dropdown field also opens), `Button` Confirm
    /// submits and Cancel closes; the `Modal`/preview body is inert; anything
    /// else (outside the popup) dismisses.
    pub(super) fn form_click(&mut self, target: &HitTarget) -> Update {
        match target {
            HitTarget::DropdownItem(i) => {
                // Session dropdown rows use the same hit target; pick via the
                // session path when the focused field is the session picker.
                let session_list = matches!(
                    &self.mode,
                    Mode::Form { state, .. }
                        if state.is_picker_focused() && state.dropdown_open
                );
                if session_list {
                    if let Mode::Form { state, .. } = &mut self.mode {
                        state.dropdown_index = *i;
                    }
                    return self.adhoc_session_dropdown_pick();
                }
                let mut target_changed = false;
                if let Mode::Form { state, .. } = &mut self.mode {
                    state.dropdown_index = *i;
                    state.dropdown_pick();
                    // Combobox target pick → refresh session cache.
                    if state.focus == crate::app::mode::adhoc_field::TARGET {
                        target_changed = true;
                    }
                }
                let mut cmds = Vec::new();
                if target_changed {
                    cmds.extend(self.adhoc_on_target_changed());
                }
                Update { dirty: true, cmds }
            }
            HitTarget::FormField(i) => {
                if let Mode::Form { state, .. } = &mut self.mode {
                    state.focus_field(*i);
                    if state.is_dropdown_focused() {
                        state.open_dropdown();
                    }
                }
                // A click on a Picker field (now focused) activates it, the same
                // as Enter — opens the inline session list.
                if matches!(&self.mode, Mode::Form { state, .. } if state.is_picker_focused()) {
                    return self.open_adhoc_session_pick();
                }
                Update { dirty: true, cmds: vec![] }
            }
            HitTarget::Button(crate::hit::ButtonKind::Confirm) => self.submit_form(),
            HitTarget::Button(crate::hit::ButtonKind::Cancel) => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
            HitTarget::Modal | HitTarget::MenuPreview => Update { dirty: false, cmds: vec![] },
            _ => {
                self.mode = Mode::List;
                Update { dirty: true, cmds: vec![] }
            }
        }
    }

    /// Fire a validated form's frozen action. `values` are the field values in
    /// declaration order (see the per-`FormAction` field layout). The New-session
    /// enqueue (Task 5.1) and Create-worktree create+enqueue (Task 5.2) wiring
    /// fill these arms.
    fn fire_form_action(&mut self, action: FormAction, values: Vec<String>) -> Update {
        match action {
            // Fields: [model dropdown, prompt textarea]. Enqueue into the frozen
            // worktree, folding in the picked model and (when resuming) the pinned
            // session id.
            FormAction::NewSession { repo, worktree, resume_session_id } => {
                let model = values.first().cloned().unwrap_or_default();
                let prompt = values.get(1).cloned().unwrap_or_default();
                let mut params =
                    serde_json::json!({ "prompt": prompt, "repo": repo, "worktree": worktree });
                if !model.is_empty() {
                    // A concrete pick (not the head "" default) is an explicit
                    // dialog choice: pin it so the worker runs it exactly, no
                    // active-provider re-head, no fallback.
                    params["model_pinned"] = serde_json::Value::Bool(true);
                    params["model"] = serde_json::Value::String(model);
                }
                if let Some(sid) = resume_session_id {
                    params["resume_session_id"] = serde_json::Value::String(sid);
                }
                let cmd = self.dispatch_rpc("enqueue task", "enqueue", params, RpcOpts::default());
                Update { dirty: true, cmds: vec![cmd] }
            }
            // Fields: [model dropdown, branch/name input, prompt textarea]. The
            // name is validated in `submit_form` before we get here. Create the
            // worktree, then (Option A) the handler enqueues the first task into
            // it using the create reply's path basename.
            FormAction::CreateWorktree { repo } => {
                let model = values.first().cloned().unwrap_or_default();
                let name = values.get(1).cloned().unwrap_or_default();
                let prompt = values.get(2).cloned().unwrap_or_default();
                self.status_line = Some(format!("creating worktree {name}…"));
                let cmd = Self::create_worktree_cmd(
                    &repo,
                    &name,
                    Some(crate::event::EnqueueAfter { prompt, model }),
                );
                Update { dirty: true, cmds: vec![cmd] }
            }
            // Fields: `[target combobox, session picker, model dropdown, prompt
            // textarea]` (see `adhoc_field`). The target resolves to a canonical
            // ref (empty → temp); the pinned session is honored only when the
            // resolved target names the worktree it was picked for. Model options
            // were already scoped to the session provider at pick time.
            FormAction::AdhocTask { repo, resume_session_id, resume_worktree, .. } => {
                use crate::app::mode::adhoc_field;
                let target = values.get(adhoc_field::TARGET).cloned().unwrap_or_default();
                let model = values.get(adhoc_field::MODEL).cloned().unwrap_or_default();
                let prompt = values.get(adhoc_field::PROMPT).cloned().unwrap_or_default();

                let mut params = serde_json::json!({ "prompt": prompt, "repo": repo });
                // A non-empty target → its canonical ref (`worktree:`/`pr:`/
                // `ticket:`); an empty target sends no ref, so the daemon spawns a
                // fresh `temp` worktree (the legacy adhoc behavior). Mirrors
                // `run_definition_cmd`: send `ref`, never `worktree`.
                let rows = self.active_worktree_rows();
                let names = Self::worktree_names(&rows);
                let aliases = crate::worktree_context::worktree_ref_aliases(&rows);
                let resolved = (!target.trim().is_empty()).then(|| {
                    super::def_args::resolve_target_ref(target.trim(), &names, &aliases)
                });
                if let Some(r) = &resolved {
                    params["ref"] = serde_json::Value::String(r.clone());
                }
                if !model.is_empty() {
                    // A concrete pick (not the head "" default) is an explicit
                    // dialog choice: pin it so the worker runs it exactly, no
                    // active-provider re-head, no fallback.
                    params["model_pinned"] = serde_json::Value::Bool(true);
                    params["model"] = serde_json::Value::String(model);
                }
                // The session pin is only valid on the worktree it was picked for
                // (`resume_worktree`); honor it only when the resolved target
                // still names that worktree.
                if let (Some(sid), Some(wt)) = (resume_session_id, resume_worktree)
                    && resolved.as_deref() == Some(format!("worktree:{wt}").as_str())
                {
                    params["resume_session_id"] = serde_json::Value::String(sid);
                }
                let cmd = self.dispatch_rpc("enqueue task", "enqueue", params, RpcOpts::default());
                Update { dirty: true, cmds: vec![cmd] }
            }
            // Fields: [provider dropdown]. Look up the picked provider's
            // resolved bin in the frozen `choices` and fire the SAME
            // `Cmd::Goto` the old `Mode::ProviderPick` fired (fresh
            // interactive — no resume). A picked name absent from `choices`
            // (shouldn't happen — the dropdown only offers `choices`' names)
            // is a silent no-op, matching the old picker's index-miss guard.
            FormAction::GotoProvider {
                path,
                choices,
                juice_base,
            } => {
                let name = values.first().cloned().unwrap_or_default();
                let cmd = choices.iter().find(|(n, _)| *n == name).map(|(_, bin)| bin.clone());
                match cmd {
                    Some(cmd) => Update {
                        dirty: true,
                        cmds: vec![Cmd::Goto {
                            path,
                            cmd,
                            juice_base,
                        }],
                    },
                    None => Update { dirty: true, cmds: vec![] },
                }
            }
            // Fields: [provider dropdown]. Apply only when the pick differs
            // from the current active provider — same-selection is a silent
            // close (no RPC, no optimistic write). Optimistic update writes
            // BOTH the live snapshot (indicator source) and the cached
            // settings payload (so the `,` overlay agrees); the daemon's next
            // state broadcast overwrites the snapshot field authoritatively.
            FormAction::SwitchProvider => {
                let target = values.first().cloned().unwrap_or_default();
                let current = self.active_provider().unwrap_or_default();
                if target.is_empty() || target == current {
                    return Update { dirty: true, cmds: vec![] };
                }
                if let Some(snap) = self.snapshot.as_mut() {
                    snap.active_provider = Some(target.clone());
                }
                if let Some(Some(p)) = self.settings.as_mut() {
                    p.active_provider = target.clone();
                }
                let cmd = self.dispatch_rpc(
                    "switch provider",
                    "set_active_provider",
                    serde_json::json!({ "provider": target }),
                    RpcOpts::default(),
                );
                Update { dirty: true, cmds: vec![cmd] }
            }
            // Fields: [provider dropdown]. One `retry` per frozen id. Provider
            // is resolved per-task to a concrete model ref (stamp entry or
            // ad-hoc default). Pin only when switching provider so same-provider
            // re-run keeps multi-model stamps; tasks without that provider in
            // their stamp get a bare retry (keep stamp).
            FormAction::Requeue { task_ids } => {
                let provider = values.first().cloned().unwrap_or_default();
                let snap = self.snapshot.as_ref();
                let calls: Vec<RpcCall> = task_ids
                    .into_iter()
                    .map(|id| {
                        let mut params = serde_json::json!({ "id": id });
                        if !provider.is_empty() {
                            let task = snap.and_then(|s| {
                                s.tasks
                                    .iter()
                                    .chain(s.archived_recent.iter())
                                    .find(|t| t.id == id)
                            });
                            if let Some(task) = task {
                                if let Some(model_ref) =
                                    self.resolve_requeue_model_for_provider(task, &provider)
                                {
                                    // Prefer last-run provider when available for
                                    // "is this a switch?" so list-head ≠ last-run
                                    // does not false-pin on same-provider Enter.
                                    let current_provider = self
                                        .requeue_preferred_model(std::slice::from_ref(&id))
                                        .or_else(|| {
                                            task.model
                                                .as_ref()
                                                .and_then(|m| m.refs().into_iter().next())
                                                .map(|r| provider_of_ref(&r).to_string())
                                        });
                                    if current_provider.as_deref() != Some(provider.as_str()) {
                                        params["model"] =
                                            serde_json::Value::String(model_ref);
                                        params["model_pinned"] =
                                            serde_json::Value::Bool(true);
                                    }
                                }
                            }
                        }
                        RpcCall {
                            method: "retry".into(),
                            params,
                        }
                    })
                    .collect();
                self.clear_range_and_marks(ListPane::Queue);
                Update {
                    dirty: true,
                    cmds: vec![Cmd::RpcSeq {
                        verb: "reran".into(),
                        calls,
                        invalidate_defs_for: None,
                    }],
                }
            }
        }
    }
}
