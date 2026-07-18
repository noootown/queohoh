use std::collections::HashMap;

use serde::{Deserialize, Deserializer};

/// `null` (or a missing field, via container `default`) → `T::default()`. Mirrors
/// `normalizeSnapshot`'s coercion of an old daemon's absent/nullish collections.
/// A *wrong-typed* value (e.g. a string where an array is expected) still errors;
/// the subscription's `unwrap_or_default()` is the crash-safety net for that
/// (the real daemon never sends wrong types — only missing fields on old builds).
fn nullable_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// Field-level serde default for a `bool` that should read `true` when absent
/// (the container `default` would give `false`). Used by `DefinitionSummary::
/// cron_enabled` so an old daemon that omits the field is treated as enabled.
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StateSnapshot {
    #[serde(deserialize_with = "nullable_default")]
    pub tasks: Vec<TaskInstance>,
    /// Full archived list from the daemon (wire name `archivedRecent` kept for
    /// compat; content is no longer a recent tail). TUI project-filters and
    /// shows all surviving archived rows in QUEUE.
    #[serde(deserialize_with = "nullable_default")]
    pub archived_recent: Vec<TaskInstance>,
    #[serde(deserialize_with = "nullable_default")]
    pub sessions: Vec<SessionEntry>,
    #[serde(deserialize_with = "nullable_default")]
    pub running: Vec<String>,
    pub max_concurrent: Option<u32>,
    #[serde(deserialize_with = "nullable_default")]
    pub projects: Vec<Project>,
    #[serde(deserialize_with = "nullable_default")]
    pub worktrees: HashMap<String, Vec<WorktreeInfo>>,
    pub build_id: Option<String>,
    /// The operator's currently-active provider (`SettingsStore`), echoed on
    /// EVERY state broadcast so a `set_active_provider` from any client
    /// re-renders the top-bar `↯ <provider>` indicator live — this is the
    /// reconcile source the optimistic switch writes to and the daemon overwrites
    /// on the next broadcast. `None`/empty on an old daemon that omits it (via the
    /// container `default`); the indicator then falls back to the `settings`
    /// payload's `active_provider`, or shows nothing.
    pub active_provider: Option<String>,
    // `gotoCommand` was removed from the state snapshot (Task 1 / general-provider):
    // workspace init-tab overrides are gone; interactive goto uses provider `bin`
    // instead. A stale daemon that still sends the key is silently ignored (no
    // `deny_unknown_fields`).
    /// Enabled provider names in config-precedence order (daemon
    /// `config.providers` with `enabled: true` only). The top-bar chip cluster
    /// renders exactly this list — disabled providers never appear. Optional —
    /// old daemons omit (`None` via container `default`); the header then falls
    /// back to the settings payload's enabled providers, then the active name.
    pub enabled_providers: Option<Vec<String>>,
    /// Active provider usage chip (optional; old daemons / single-chip era).
    /// Kept for wire compat: a modern daemon still publishes the active entry
    /// here alongside `provider_usages`. Prefer `provider_usages` for multi-chip
    /// headers. `None` when the poller has nothing, the active provider has no
    /// sample, or an old daemon omits the field (container `default`).
    pub provider_usage: Option<ProviderUsage>,
    /// Usage samples for every enabled provider the poller has data for, in
    /// config-precedence order (multi-chip header). Optional — old daemons omit
    /// (`None` via container `default`); empty vec when the poller has nothing
    /// yet. The TUI falls back to `provider_usage` when this is absent.
    pub provider_usages: Option<Vec<ProviderUsage>>,
}

/// Severity bucket for a provider usage sample. Wire values are lowercase
/// `"ok"|"warn"|"crit"|"unknown"` (single-word, so `camelCase` rename matches).
/// Unknown wire values (and the default) land on `Unknown` via `#[serde(other)]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum UsageSeverity {
    Ok,
    Warn,
    Crit,
    #[default]
    #[serde(other)]
    Unknown,
}

/// Active provider usage published on `StateSnapshot` for the top-bar chip.
/// Mirrors `ProviderUsage` in packages/core (provider, text, severity, fetchedAt,
/// stale). Container `default` so a partial payload still deserializes.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderUsage {
    pub provider: String,
    pub text: String,
    pub severity: UsageSeverity,
    pub fetched_at: u64,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    pub name: String,
    /// The project's optional author identity (its `vars.yaml` `github_id:` key,
    /// e.g. `noootown`). `None` on an old daemon that omits it (via the container
    /// `default`), or when the project has no `github_id` configured. The
    /// WORKTREES pane matches it against each worktree's last-commit author
    /// email/name to sort "my" worktrees first; absent → that tier is a no-op.
    pub github_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    Queued,
    NeedsInput,
    Running,
    Done,
    Failed,
    /// User-cancelled via the queue `x` action (skip on a queued/needs-input task,
    /// stop on a running one) — the daemon lands `cancelled`, distinct from a
    /// `failed` run. Rendered with its own glyph, never the red ✗.
    Cancelled,
    /// Skipped by a chain (an earlier step failed, so this one never ran). Wire
    /// value `skipped`; rendered dim, distinct from `cancelled`.
    Skipped,
    /// The worker claimed success (clean tree) but the task's `verify`
    /// (done-condition) command disagreed — non-zero exit or timeout. Wire value
    /// `verify-failed` (kebab, like `needs-input`). Rendered with its own glyph in
    /// error red, distinct from a worker `failed`.
    VerifyFailed,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskInstance {
    pub id: String,
    pub status: TaskStatus,
    pub definition: Option<String>,
    pub item: Option<HashMap<String, String>>,
    pub item_key: Option<String>,
    pub target: TaskTarget,
    pub priority: String,
    pub created: String,
    /// Start timestamp (ISO UTC) of the CURRENT run, re-stamped by the daemon each
    /// time the worker flips the task to `running`. `None` on an old daemon that
    /// omits it (via the container `default`) or on a task that has never run —
    /// the live `⏱` timer falls back to `created` in that case. Re-stamping on a
    /// re-run is what restarts the timer from the re-run, not the original create.
    pub started_at: Option<String>,
    /// Completion timestamp (ISO UTC), present once the task reaches a terminal
    /// status. `None` on an old daemon that omits it (via the container `default`)
    /// or on a task that hasn't finished — drives the FINISHED-section ordering.
    pub finished_at: Option<String>,
    pub source: String,
    pub ephemeral_worktree: bool,
    pub error: Option<String>,
    pub session: String,
    pub resume_session_id: Option<String>,
    pub model: Option<String>,
    pub prompt: String,
    /// Done-condition (`verify`) fields (additive; all `None` on an old daemon
    /// that omits them, via the container `default`). `verify` is the configured
    /// command; `verified` is the last verdict (true/false, `None` = never run);
    /// `verify_exit_code` is the command's exit (`None` on timeout / never run);
    /// `verify_output` is a bounded (~4 KB) tail of its combined output. The
    /// distinct `verify-failed` status carries the headline; these detail it.
    pub verify: Option<String>,
    pub verified: Option<bool>,
    pub verify_exit_code: Option<i64>,
    pub verify_output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskTarget {
    pub repo: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub worktree: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionEntry {
    pub kind: String,
    pub key: String,
    pub lane: Option<String>,
    pub cwd: Option<String>,
    pub pid: Option<u32>,
    pub started_at: String,
    pub heartbeat_at: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WorktreeInfo {
    pub name: String,
    pub path: String,
    pub branch: String,
    /// Daemon git enrichment (all `None` on an old daemon that omits them, via
    /// the container `default`): working tree has uncommitted changes,
    pub dirty: Option<bool>,
    /// unix epoch SECONDS of the last commit,
    pub last_commit_epoch: Option<u64>,
    /// the author name of the last commit. A stale daemon may still send the
    /// retired `ahead`/`behind` fields — serde ignores them (no deny_unknown).
    pub last_commit_author: Option<String>,
    /// and the author EMAIL of the last commit (git `%ae`; `None`/null when
    /// unknown or on an old daemon). Paired with `last_commit_author` for the
    /// WORKTREES "mine-first" sort — the project's `github_id` is matched as a
    /// case-insensitive substring of either.
    pub last_commit_author_email: Option<String>,
    /// Short hash of the last commit (git `%h`; `None`/null when unknown or on an
    /// old daemon). Shown in the worktree detail info tab.
    pub last_commit_hash: Option<String>,
    /// Open PR number for this worktree's branch (`None`/null when there is no
    /// open PR, `gh` is unavailable, or on an old daemon). Shown as `#<n>` in the
    /// worktree detail info tab.
    pub pr_number: Option<u64>,
    /// Web URL of that open PR (`None`/null when there is no open PR, `gh` is
    /// unavailable, or on an old daemon that predates the field). Paired with
    /// `pr_number` so the `#<n>` chip in the detail info tab and the WORKTREES
    /// PR column open the PR in a browser on a click.
    pub pr_url: Option<String>,
    /// PR author display name (the daemon's `prAuthor`: the PR's `author.name`,
    /// falling back to `author.login`). This is who OPENED the PR — for a
    /// squash-merged branch the local `last_commit_author` is instead an
    /// automation merge commit, so the Author column prefers this field (see
    /// `wt_author_text`). `None`/null when there is no PR, `gh` is unavailable,
    /// or on an old daemon that predates the field (via the container `default`).
    pub pr_author: Option<String>,
    /// True when queohoh must never delete this worktree (the project's main
    /// checkout or a name in the project's `protected_worktrees`). Absent on an
    /// old daemon → `false` via the container `default` (removable affordance;
    /// the daemon guard is the real block).
    pub protected: bool,
    /// Worktree HEAD is an ancestor of the project's default branch (vars.yaml
    /// `default_branch`, fallback `main`) — its committed work has been merged
    /// back. `None`/null = unknown, an old daemon, or the default-branch
    /// checkout itself. Drives the `↣` front-column marker.
    pub merged: Option<bool>,
    /// Whether this worktree's PR is APPROVED (the daemon's `approved`, reduced
    /// from `gh`'s `reviewDecision === "APPROVED"`). `Some(true)` = approved,
    /// `Some(false)` = a PR exists but isn't approved, `None`/null = unknown / no
    /// PR / `gh` unavailable / an old daemon that predates the field (via the
    /// container `default`). Drives the green approved marker, which shares the
    /// `↣` merged marker's front slot but yields to it (see `wt_merge_marker`).
    pub approved: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ArgSpec {
    pub name: String,
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    pub default: Option<String>,
    pub options: Option<Vec<String>>,
    pub description: Option<String>,
}

impl ArgSpec {
    /// True when this arg is the worktree/target selector (rendered as a
    /// combobox; resolves to a ref on submit).
    pub fn is_worktree(&self) -> bool {
        self.r#type.as_deref() == Some("worktree")
    }

    /// `type: branch` — rendered as a dropdown seeded with the repo's worktree
    /// branches (incl. main/master).
    pub fn is_branch(&self) -> bool {
        self.r#type.as_deref() == Some("branch")
    }

    /// `type: text` — the multiline, auto-growing textarea (the opt-in for
    /// free-text args that need more than one line, e.g. a problem scenario).
    /// Plain free-text args with no `type` render as a single-line input.
    pub fn is_text(&self) -> bool {
        self.r#type.as_deref() == Some("text")
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DefinitionSummary {
    pub repo: String,
    pub name: String,
    pub scope: String,
    pub args: Vec<ArgSpec>,
    pub has_discovery: bool,
    /// Human-editable cron expression, or `None` when the def has no schedule.
    /// `default` on the container covers old daemons that omit the field.
    pub cron: Option<String>,
    /// Whether this def's cron is currently ARMED. Meaningful only when
    /// `cron.is_some()`; the operator pauses/resumes it with the `[o]cron` toggle
    /// (`set_cron_enabled` RPC, persisted daemon-side). The Cron column renders
    /// DIMMED when this is `false`. Field-level `default = true` (not the
    /// container `default`, which would give `false`) so an old daemon that omits
    /// the field reads as enabled — matching pre-toggle behavior where every cron
    /// always fired.
    #[serde(default = "default_true")]
    pub cron_enabled: bool,
    /// One-line human description of the def, or `None` when unset. `default` on
    /// the container covers old daemons that omit the field.
    pub description: Option<String>,
    /// The def's authored `model:` — a single `provider/label` ref, an ordered
    /// fallback list of them, or `None` (no `model:` → resolves against
    /// `default_models` at run time; also the old-daemon default). The TASKS
    /// Model column shows only the **effective head** of
    /// [`crate::chain::resolve_model_chain`] under the active provider (one
    /// label via [`crate::chain::effective_model_head`]), not this list
    /// verbatim and not the full fallback chain. Was a resolved model id
    /// before Task 5; the flat catalog replaced the per-provider alias table,
    /// so the daemon now forwards the authored ref(s) (`string | string[] |
    /// null` on the wire).
    pub model: Option<ModelRef>,
    /// The def's `worktree:` setting (`"repo"`, `"temp"`, `"auto"`, or a
    /// `pr:{{…}}`-style template; schema default `"temp"`). `None` on an old
    /// daemon that omits it. The worktree-scoped task menu keeps only defs that
    /// consume the selected worktree — see `def_uses_worktree_context`.
    pub worktree: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskDefinition {
    pub name: String,
    pub repo: String,
    /// One-line human description of the def, or `None` when unset (via the
    /// container `default` — also covers an old daemon that omits it). Shown in
    /// the run detail's `info` sub-tab Config section.
    pub description: Option<String>,
    pub discovery: Option<Discovery>,
    pub cron: Option<String>,
    pub args: Vec<ArgSpec>,
    pub dedup: String,
    pub worktree: String,
    pub pre_run: Option<String>,
    pub post_run: Option<String>,
    /// The def's authored `model:` — a single `provider/label` ref, an ordered
    /// fallback list of them, or `None` (no `model:` → resolves against
    /// `default_models` at run time). The definition config row renders the
    /// **full** resolved chain under the active provider (see
    /// [`crate::chain::resolved_model_chain_display`]); the TASKS list column
    /// shows only the effective head. Not this list verbatim.
    /// Was a resolved-id string paired with a sibling `modelResolved` before
    /// Task 5; the flat catalog replaced the alias table, so `modelResolved`
    /// was REMOVED from the wire and the authored ref(s) (`string | string[] |
    /// null`) are what the daemon forwards.
    pub model: Option<ModelRef>,
    pub timeout_ms: u64,
    pub priority: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Discovery {
    pub command: String,
    pub item_key: String,
}

/// A definition's authored `model:` on the wire: a single `provider/label` ref,
/// or an ordered fallback list of them. Untagged so a bare JSON string and a
/// JSON array both deserialize into the right variant; a `null`/missing `model`
/// is represented by the enclosing `Option<ModelRef>` being `None`. Mirrors the
/// daemon's `model: string | string[] | null` (see packages/core/src/definition.ts).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(untagged)]
pub enum ModelRef {
    One(String),
    Many(Vec<String>),
}

impl ModelRef {
    /// The ref(s) in authored order (a single ref → a one-element list).
    pub fn refs(&self) -> Vec<String> {
        match self {
            ModelRef::One(s) => vec![s.clone()],
            ModelRef::Many(v) => v.clone(),
        }
    }

    /// Human display: the ref(s) joined with ` → ` (the fallback-order arrow).
    pub fn display(&self) -> String {
        self.refs().join(" → ")
    }
}

impl From<&str> for ModelRef {
    /// A bare ref string → a single-ref chain (`ModelRef::One`).
    fn from(s: &str) -> Self {
        ModelRef::One(s.to_string())
    }
}

impl From<String> for ModelRef {
    fn from(s: String) -> Self {
        ModelRef::One(s)
    }
}

/// One concrete model in the daemon's merged catalog — the Rust mirror of
/// `CatalogEntry` in packages/core/src/catalog.ts. `id` is the provider-specific
/// CLI model id; `label` is the short reference used in `provider/label` refs and
/// pickers. `hidden` hides it from PICKERS only (a hidden entry still resolves
/// when referenced explicitly), so the TUI filters it out of the model dropdown.
/// Wire keys are single lowercase words, so field names match 1:1 (no rename).
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct CatalogEntry {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    /// Picker-only hide flag; absent on the wire for a visible entry → `false`.
    #[serde(default)]
    pub hidden: bool,
}

impl CatalogEntry {
    /// Reference form `provider/label` — the value stored in (and submitted from)
    /// a `model:` dropdown option.
    pub fn model_ref(&self) -> String {
        format!("{}/{}", self.provider, self.label)
    }

    /// Display form `label (provider)` — the text shown in the model picker.
    pub fn model_display(&self) -> String {
        format!("{} ({})", self.label, self.provider)
    }
}

/// The `settings` RPC's `default_models` block (Task 5): the effective global
/// default-model chain plus any per-project overrides. Fields default so an old
/// daemon that omits the block deserializes to empties.
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct DefaultModels {
    /// The workspace-wide default chain (`provider/label` refs, fallback order).
    #[serde(default)]
    pub global: Vec<String>,
    /// Only projects whose `vars.yaml` sets a NON-EMPTY `default_models:`
    /// override; everyone else is described by `global`.
    #[serde(default)]
    pub projects: Vec<DefaultModelsProject>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct DefaultModelsProject {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub default_models: Vec<String>,
    /// Path the override was loaded from (its `vars.yaml`); provenance only.
    #[serde(default)]
    pub source: String,
}

impl DefaultModels {
    /// The effective default-model chain for `repo`: its project override when
    /// present and non-empty, else the global chain.
    pub fn refs_for(&self, repo: &str) -> Vec<String> {
        self.projects
            .iter()
            .find(|p| p.name == repo)
            .map(|p| p.default_models.clone())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| self.global.clone())
    }
}

/// Read-only mirror of the daemon's `settings` RPC (Task 4). Every field is
/// `#[serde(default)]` so an OLD daemon that omits the whole block — or any
/// subtree of it — deserializes to empties rather than erroring; the app stores
/// a *failed* fetch as `Some(None)` and never reaches `from_value`, but a
/// partial/forward-compatible payload from a mixed-version daemon still lands
/// cleanly. Keys are single lowercase words on the wire (`models`/`defaults`/
/// `global`/`entries`/`source`/`projects`/`repo`), so field names match 1:1 and
/// no `rename_all` is needed.
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct SettingsPayload {
    /// The merged, provider-precedence-grouped model catalog (Task 5). Hidden
    /// entries are INCLUDED — the model picker filters them out but still
    /// resolves a hidden ref when named explicitly. `#[serde(default)]` so an old
    /// daemon that omits it deserializes to an empty vec; the picker then falls
    /// back to the built-in mirror (`form::builtin_catalog`).
    #[serde(default)]
    pub catalog: Vec<CatalogEntry>,
    /// The operator's currently-active provider (`SettingsStore`). Empty string
    /// on an old daemon that omits it.
    #[serde(default)]
    pub active_provider: String,
    /// The effective default-model chains: global + per-project overrides. Feeds
    /// the dropdown's `default (…)` head-option label.
    #[serde(default)]
    pub default_models: DefaultModels,
    /// Configured providers: `name` + `enabled` + optional `bin`. The
    /// per-provider tier (alias→id) map was REMOVED in Task 5 — models now live
    /// in the flat `catalog`. `#[serde(default)]` so an old daemon that omits
    /// the field (or a stale one that still sends the retired `models` key per
    /// provider — serde ignores unknown fields) deserializes cleanly.
    #[serde(default)]
    pub providers: Vec<SettingsProvider>,
    // The pre-Task-5 top-level `models` block (alias→id defaults + global/
    // project layers) was removed entirely in this cutover — no live reader
    // consumed it (the picker reads `catalog` / `default_models` above). A
    // pre-Task-5 daemon that still sends the field lands on an unrecognized
    // key; serde silently drops it (no `deny_unknown_fields` on this struct),
    // so old-daemon payloads still deserialize cleanly.
}

#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct SettingsProvider {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    /// Optional CLI binary path for this provider (e.g. a pinned `grok` binary).
    /// Omitted on the wire when unset; `None` for old daemons that never send
    /// it. Interactive goto uses this as the right-pane command (fallback: the
    /// provider name).
    #[serde(default)]
    pub bin: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full modern snapshot: every field present, one task with every field.
    fn modern_json() -> &'static str {
        r#"{
          "tasks": [{
            "id": "01TASKAAA000000000000000000",
            "status": "running",
            "definition": "pr-ready",
            "item": {"pr": "257"},
            "itemKey": "pr:257",
            "target": {"repo": "platform", "ref": "worktree:platform.feat-a", "worktree": "platform.feat-a"},
            "priority": "normal",
            "created": "2026-07-08T10:00:00.000Z",
            "startedAt": "2026-07-08T10:00:30.000Z",
            "finishedAt": "2026-07-08T10:05:00.000Z",
            "source": "tui",
            "ephemeralWorktree": false,
            "error": null,
            "session": "main",
            "resumeSessionId": "sess-1",
            "model": "claude/claude-opus-4.8",
            "prompt": "do the thing\n",
            "verify": "gh pr view --json labels -q '.labels[].name' | grep -qx ready-for-review",
            "verified": false,
            "verifyExitCode": 1,
            "verifyOutput": "checking labels...\nno match\n"
          }],
          "archivedRecent": [],
          "sessions": [{
            "kind": "interactive", "key": "s1", "lane": "platform:platform.feat-a",
            "cwd": "/wt/platform.feat-a", "pid": 4242,
            "startedAt": "2026-07-08T09:00:00.000Z", "heartbeatAt": "2026-07-08T10:00:00.000Z"
          }],
          "running": ["01TASKAAA000000000000000000"],
          "maxConcurrent": 3,
          "projects": [{"name": "platform", "githubId": "noootown"}, {"name": "web"}],
          "worktrees": {"platform": [{"name": "platform.feat-a", "path": "/wt/platform.feat-a", "branch": "feat-a",
            "dirty": true, "lastCommitEpoch": 1751970000, "lastCommitAuthor": "Kevin O'Shea",
            "lastCommitAuthorEmail": "kevin@justicebid.com"}]},
          "buildId": "1751970000000"
        }"#
    }

    #[test]
    fn deserializes_a_full_modern_snapshot() {
        let s: StateSnapshot = serde_json::from_str(modern_json()).unwrap();
        assert_eq!(s.tasks.len(), 1);
        let t = &s.tasks[0];
        assert_eq!(t.id, "01TASKAAA000000000000000000");
        assert_eq!(t.status, TaskStatus::Running);
        assert_eq!(t.definition.as_deref(), Some("pr-ready"));
        assert_eq!(t.item.as_ref().unwrap().get("pr").map(String::as_str), Some("257"));
        assert_eq!(t.item_key.as_deref(), Some("pr:257"));
        assert_eq!(t.target.repo, "platform");
        assert_eq!(t.target.git_ref, "worktree:platform.feat-a");
        assert_eq!(t.target.worktree.as_deref(), Some("platform.feat-a"));
        assert!(!t.ephemeral_worktree);
        assert_eq!(t.session, "main");
        assert_eq!(t.resume_session_id.as_deref(), Some("sess-1"));
        assert_eq!(t.model.as_deref(), Some("claude/claude-opus-4.8"));
        assert_eq!(t.prompt, "do the thing\n");
        assert_eq!(t.started_at.as_deref(), Some("2026-07-08T10:00:30.000Z"));
        assert_eq!(t.finished_at.as_deref(), Some("2026-07-08T10:05:00.000Z"));
        assert_eq!(t.verify.as_deref(), Some(
            "gh pr view --json labels -q '.labels[].name' | grep -qx ready-for-review",
        ));
        assert_eq!(t.verified, Some(false));
        assert_eq!(t.verify_exit_code, Some(1));
        assert_eq!(t.verify_output.as_deref(), Some("checking labels...\nno match\n"));
        assert_eq!(s.sessions[0].kind, "interactive");
        assert_eq!(s.sessions[0].pid, Some(4242));
        assert_eq!(s.max_concurrent, Some(3));
        assert_eq!(
            s.projects,
            vec![
                Project { name: "platform".into(), github_id: Some("noootown".into()) },
                Project { name: "web".into(), github_id: None },
            ]
        );
        let wt = &s.worktrees["platform"][0];
        assert_eq!(wt.branch, "feat-a");
        assert_eq!(wt.dirty, Some(true));
        assert_eq!(wt.last_commit_epoch, Some(1_751_970_000));
        assert_eq!(wt.last_commit_author_email.as_deref(), Some("kevin@justicebid.com"));
        assert_eq!(wt.last_commit_author.as_deref(), Some("Kevin O'Shea"));
        assert_eq!(s.build_id.as_deref(), Some("1751970000000"));
    }

    #[test]
    fn old_daemon_snapshot_missing_new_fields_defaults_without_error() {
        // Predates projects/worktrees/maxConcurrent/buildId (mirrors
        // use-daemon.test's OLD-daemon case): only the original four fields.
        let old = r#"{"tasks": [{"id": "t1", "target": {"repo": "platform", "ref": "temp"}}],
                      "archivedRecent": [], "sessions": [], "running": []}"#;
        let s: StateSnapshot = serde_json::from_str(old).unwrap();
        assert_eq!(s.tasks.len(), 1);
        assert_eq!(s.tasks[0].id, "t1");
        // status absent → Unknown (default); target.worktree absent → None.
        assert_eq!(s.tasks[0].status, TaskStatus::Unknown);
        assert_eq!(s.tasks[0].target.worktree, None);
        // startedAt/finishedAt absent on an old daemon → None (additive fields).
        assert_eq!(s.tasks[0].started_at, None);
        assert_eq!(s.tasks[0].finished_at, None);
        // verify fields absent on an old daemon → None (additive fields).
        assert_eq!(s.tasks[0].verify, None);
        assert_eq!(s.tasks[0].verified, None);
        assert_eq!(s.tasks[0].verify_exit_code, None);
        assert_eq!(s.tasks[0].verify_output, None);
        assert_eq!(s.projects, vec![]);
        assert!(s.worktrees.is_empty());
        assert_eq!(s.max_concurrent, None);
        // buildId absent → None means "stale" for self-heal — must NOT default to "".
        assert_eq!(s.build_id, None);
    }

    #[test]
    fn snapshot_without_goto_command_deserializes() {
        // Task 1 dropped `gotoCommand` from the daemon snapshot. A modern
        // payload (and an empty one) must still parse; a stale daemon that
        // still sends the key is silently ignored (no deny_unknown_fields).
        let s: StateSnapshot = serde_json::from_str(
            r#"{"buildId":"1","activeProvider":"grok"}"#,
        )
        .unwrap();
        assert_eq!(s.build_id.as_deref(), Some("1"));
        assert_eq!(s.active_provider.as_deref(), Some("grok"));
        let stale: StateSnapshot =
            serde_json::from_str(r#"{"gotoCommand":"init-tab {cmd}"}"#).unwrap();
        assert_eq!(stale.build_id, None);
    }

    #[test]
    fn settings_provider_bin_present_and_absent() {
        // Optional `bin` on settings.providers[] (Task 1): present when the
        // operator pinned a CLI path; omitted → None via field-level default.
        let with: SettingsProvider = serde_json::from_str(
            r#"{"name":"grok","enabled":true,"bin":"/tmp/grok-bin"}"#,
        )
        .unwrap();
        assert_eq!(with.name, "grok");
        assert!(with.enabled);
        assert_eq!(with.bin.as_deref(), Some("/tmp/grok-bin"));
        let without: SettingsProvider =
            serde_json::from_str(r#"{"name":"claude","enabled":true}"#).unwrap();
        assert_eq!(without.bin, None);
    }

    #[test]
    fn active_provider_present_deserializes_to_some_and_absent_to_none() {
        // A modern daemon echoes camelCase `activeProvider` on the state
        // broadcast (the top-bar indicator's reconcile source)...
        let s: StateSnapshot =
            serde_json::from_str(r#"{"activeProvider":"grok"}"#).unwrap();
        assert_eq!(s.active_provider.as_deref(), Some("grok"));
        // ...and an old daemon that omits it defaults to None (container `default`).
        let old: StateSnapshot = serde_json::from_str("{}").unwrap();
        assert_eq!(old.active_provider, None);
    }

    #[test]
    fn provider_usage_present_and_absent() {
        // A modern daemon publishes camelCase `providerUsage` on the state
        // broadcast (single-chip back-compat for the active entry)...
        let s: StateSnapshot = serde_json::from_str(
            r#"{"providerUsage":{"provider":"claude","text":"100%/73%","severity":"crit","fetchedAt":1,"stale":false}}"#,
        )
        .unwrap();
        let u = s.provider_usage.unwrap();
        assert_eq!(u.provider, "claude");
        assert_eq!(u.text, "100%/73%");
        assert_eq!(u.severity, UsageSeverity::Crit);
        assert_eq!(u.fetched_at, 1);
        assert!(!u.stale);

        // ...and an old daemon that omits it defaults to None (container `default`).
        let old: StateSnapshot = serde_json::from_str(r#"{}"#).unwrap();
        assert!(old.provider_usage.is_none());
        assert!(old.provider_usages.is_none());
    }

    #[test]
    fn enabled_providers_present_and_absent() {
        let s: StateSnapshot = serde_json::from_str(
            r#"{"enabledProviders":["claude","grok"]}"#,
        )
        .unwrap();
        assert_eq!(
            s.enabled_providers.as_deref(),
            Some(&["claude".to_string(), "grok".to_string()][..])
        );
        let old: StateSnapshot = serde_json::from_str(r#"{}"#).unwrap();
        assert!(old.enabled_providers.is_none());
    }

    #[test]
    fn provider_usages_array_present_and_absent() {
        // Multi-chip header: camelCase `providerUsages` is a list in precedence order.
        let s: StateSnapshot = serde_json::from_str(
            r#"{"providerUsages":[
                {"provider":"claude","text":"10%","severity":"ok","fetchedAt":1,"stale":false},
                {"provider":"grok","text":"42% mo","severity":"warn","fetchedAt":2,"stale":true}
            ]}"#,
        )
        .unwrap();
        let us = s.provider_usages.unwrap();
        assert_eq!(us.len(), 2);
        assert_eq!(us[0].provider, "claude");
        assert_eq!(us[0].text, "10%");
        assert_eq!(us[1].provider, "grok");
        assert!(us[1].stale);
        assert_eq!(us[1].severity, UsageSeverity::Warn);

        let old: StateSnapshot = serde_json::from_str(r#"{}"#).unwrap();
        assert!(old.provider_usages.is_none());
    }

    #[test]
    fn null_valued_collections_coerce_to_empty() {
        // The nullable_default shim: `null` where an array/object is expected → default.
        let s: StateSnapshot = serde_json::from_str(
            r#"{"tasks": null, "running": null, "worktrees": null, "projects": null}"#,
        )
        .unwrap();
        assert_eq!(s.tasks, vec![]);
        assert_eq!(s.running, vec![] as Vec<String>);
        assert!(s.worktrees.is_empty());
        assert_eq!(s.projects, vec![]);
    }

    #[test]
    fn worktree_git_enrichment_absent_defaults_to_none() {
        // An old daemon emits only name/path/branch; the git-enrichment fields
        // default to None (container `default`) rather than erroring.
        let s: StateSnapshot = serde_json::from_str(
            r#"{"worktrees": {"platform": [{"name": "wt-a", "path": "/wt/wt-a", "branch": "a"}]}}"#,
        )
        .unwrap();
        let wt = &s.worktrees["platform"][0];
        assert_eq!(wt.dirty, None);
        assert_eq!(wt.last_commit_epoch, None);
        assert_eq!(wt.last_commit_author, None);
    }

    #[test]
    fn worktree_pr_author_present_parses_and_absent_defaults_none() {
        // A modern daemon sends camelCase `prAuthor` (the PR author display name,
        // which for a squash-merged branch differs from lastCommitAuthor)...
        let with: WorktreeInfo = serde_json::from_str(
            r#"{"name":"a","path":"/a","branch":"a","lastCommitAuthor":"Ian Chiu",
                "prAuthor":"Tim Kuminecz"}"#,
        )
        .unwrap();
        assert_eq!(with.pr_author.as_deref(), Some("Tim Kuminecz"));
        assert_eq!(with.last_commit_author.as_deref(), Some("Ian Chiu"));
        // ...and an old daemon that omits it defaults to None (container `default`),
        // leaving lastCommitAuthor intact.
        let without: WorktreeInfo = serde_json::from_str(
            r#"{"name":"a","path":"/a","branch":"a","lastCommitAuthor":"Ian Chiu"}"#,
        )
        .unwrap();
        assert_eq!(without.pr_author, None);
        assert_eq!(without.last_commit_author.as_deref(), Some("Ian Chiu"));
    }

    #[test]
    fn worktree_approved_present_parses_and_absent_defaults_none() {
        // A modern daemon sends camelCase `approved` (reduced from gh's
        // reviewDecision); true and false both parse...
        let approved: WorktreeInfo = serde_json::from_str(
            r#"{"name":"a","path":"/a","branch":"a","approved":true}"#,
        )
        .unwrap();
        assert_eq!(approved.approved, Some(true));
        let not_approved: WorktreeInfo = serde_json::from_str(
            r#"{"name":"a","path":"/a","branch":"a","approved":false}"#,
        )
        .unwrap();
        assert_eq!(not_approved.approved, Some(false));
        // ...and an old daemon that omits it defaults to None (container `default`).
        let old: WorktreeInfo =
            serde_json::from_str(r#"{"name":"a","path":"/a","branch":"a"}"#).unwrap();
        assert_eq!(old.approved, None);
    }

    #[test]
    fn worktree_protected_defaults_false_and_parses_true() {
        // Absent (old daemon) → false via the container `default`.
        let old: WorktreeInfo =
            serde_json::from_str(r#"{"name":"a","path":"/a","branch":"a"}"#).unwrap();
        assert!(!old.protected);
        // Present → parsed.
        let new: WorktreeInfo = serde_json::from_str(
            r#"{"name":"a","path":"/a","branch":"a","protected":true}"#,
        )
        .unwrap();
        assert!(new.protected);
    }

    #[test]
    fn stale_daemon_ahead_behind_fields_are_ignored() {
        // A daemon predating the author rewrite still emits ahead/behind; without
        // deny_unknown_fields serde silently drops them (new field stays None).
        let s: StateSnapshot = serde_json::from_str(
            r#"{"worktrees": {"platform": [{"name": "wt-a", "path": "/wt/wt-a", "branch": "a",
                "ahead": 3, "behind": 12, "lastCommitEpoch": 1751970000}]}}"#,
        )
        .unwrap();
        let wt = &s.worktrees["platform"][0];
        assert_eq!(wt.last_commit_epoch, Some(1_751_970_000));
        assert_eq!(wt.last_commit_author, None);
    }

    #[test]
    fn unknown_status_maps_to_unknown_variant() {
        let t: TaskInstance =
            serde_json::from_str(r#"{"id": "x", "status": "paused-by-alien"}"#).unwrap();
        assert_eq!(t.status, TaskStatus::Unknown);
    }

    #[test]
    fn definition_summary_cron_and_description_present_and_absent() {
        // camelCase `cron`/`description` deserialize into their fields...
        let with: DefinitionSummary = serde_json::from_str(
            r#"{"repo": "platform", "name": "pr-review", "scope": "project",
                "args": [], "hasDiscovery": true, "cron": "30 13 * * *",
                "description": "Review an open PR.", "model": "claude/claude-fable-5"}"#,
        )
        .unwrap();
        assert_eq!(with.cron.as_deref(), Some("30 13 * * *"));
        assert_eq!(with.description.as_deref(), Some("Review an open PR."));
        assert_eq!(with.model, Some(ModelRef::One("claude/claude-fable-5".into())));
        // No `cronEnabled` in the payload → field-level `default_true` → armed.
        assert!(with.cron_enabled);
        // ...and an old daemon that omits them defaults to None (container `default`).
        let without: DefinitionSummary = serde_json::from_str(
            r#"{"repo": "platform", "name": "lint", "scope": "global",
                "args": [], "hasDiscovery": false}"#,
        )
        .unwrap();
        assert_eq!(without.cron, None);
        assert_eq!(without.description, None);
        assert_eq!(without.model, None);
        // Old daemon (no field) still reads as enabled, NOT the bool `default`.
        assert!(without.cron_enabled);
    }

    #[test]
    fn definition_summary_cron_enabled_false_is_honored() {
        // An explicit `cronEnabled: false` (operator paused it) deserializes as
        // disabled — the signal the TASKS Cron column dims on.
        let paused: DefinitionSummary = serde_json::from_str(
            r#"{"repo": "platform", "name": "pr-review", "scope": "project",
                "args": [], "hasDiscovery": true, "cron": "30 13 * * *",
                "cronEnabled": false}"#,
        )
        .unwrap();
        assert_eq!(paused.cron.as_deref(), Some("30 13 * * *"));
        assert!(!paused.cron_enabled);
    }

    #[test]
    fn task_definition_model_is_ref_string_list_or_null() {
        // Post-Task-5: `model` is a `provider/label` ref (single)...
        let one: TaskDefinition = serde_json::from_str(
            r#"{"name": "pr-ready", "repo": "acme", "model": "claude/claude-opus-4.8", "timeoutMs": 1800000}"#,
        )
        .unwrap();
        assert_eq!(one.model, Some(ModelRef::One("claude/claude-opus-4.8".into())));
        // ...an ordered fallback LIST...
        let many: TaskDefinition = serde_json::from_str(
            r#"{"name": "pr-ready", "repo": "acme",
                "model": ["claude/claude-opus-4.8", "grok/grok-4.5"], "timeoutMs": 1800000}"#,
        )
        .unwrap();
        assert_eq!(
            many.model,
            Some(ModelRef::Many(vec!["claude/claude-opus-4.8".into(), "grok/grok-4.5".into()]))
        );
        assert_eq!(many.model.as_ref().unwrap().display(), "claude/claude-opus-4.8 → grok/grok-4.5");
        // ...or `null`/absent → None (no `model:`; also the old-daemon default).
        // `modelResolved` was removed from the wire — a stale daemon that still
        // sends it is silently ignored (no deny_unknown_fields).
        let null: TaskDefinition = serde_json::from_str(
            r#"{"name": "lint", "repo": "acme", "model": null, "modelResolved": "claude-opus-4-8"}"#,
        )
        .unwrap();
        assert_eq!(null.model, None);
        let absent: TaskDefinition =
            serde_json::from_str(r#"{"name": "lint", "repo": "acme"}"#).unwrap();
        assert_eq!(absent.model, None);
    }

    #[test]
    fn kebab_status_needs_input_round_trips() {
        let t: TaskInstance =
            serde_json::from_str(r#"{"id": "x", "status": "needs-input"}"#).unwrap();
        assert_eq!(t.status, TaskStatus::NeedsInput);
    }

    #[test]
    fn kebab_status_verify_failed_round_trips() {
        // The wire value is `verify-failed` (kebab, via rename_all), NOT the
        // Unknown fallback — a modern TUI renders it with its own glyph.
        let t: TaskInstance =
            serde_json::from_str(r#"{"id": "x", "status": "verify-failed"}"#).unwrap();
        assert_eq!(t.status, TaskStatus::VerifyFailed);
    }

    #[test]
    fn settings_payload_stale_top_level_models_block_is_ignored() {
        // A pre-Task-5 daemon still sends the whole legacy top-level `models`
        // block (defaults/default_model/global/project layers). The struct no
        // longer has that field at all — serde silently drops the unrecognized
        // key (no `deny_unknown_fields` on `SettingsPayload`) rather than
        // erroring, same as the stale per-provider `models` key below.
        let s: SettingsPayload = serde_json::from_str(
            r#"{"models": {
                "defaults": {"opus": "claude-opus-4-8"},
                "default_model": "opus",
                "global": {"entries": {"sonnet": "claude-sonnet-4-6"},
                           "source": "/home/ian/.config/qoo/config.yaml"},
                "projects": [{"repo": "acme", "entries": {"opus": "claude-opus-4-9"},
                              "default_model": "sonnet", "source": "/repos/acme/vars.yaml"}]
            }}"#,
        )
        .unwrap();
        // No catalog in this legacy shape → empty (picker falls back to built-ins).
        assert!(s.catalog.is_empty());
        // `providers` is absent in this payload (pre-Task-12 daemon shape) → an
        // old daemon that omits the whole field defaults to an empty vec.
        assert_eq!(s.providers, vec![]);
    }

    #[test]
    fn settings_payload_catalog_active_provider_and_default_models_parse() {
        // The exact shape the post-Task-5 `settings` RPC returns: a flat `catalog`
        // (hidden included), the active provider, a `default_models` block, and
        // `providers` reduced to name/enabled (the per-provider tier map is gone).
        let s: SettingsPayload = serde_json::from_str(
            r#"{
              "catalog": [
                {"provider": "claude", "id": "claude-fable-5", "label": "claude-fable-5"},
                {"provider": "claude", "id": "claude-opus-4-8", "label": "claude-opus-4.8"},
                {"provider": "grok", "id": "grok-4.5", "label": "grok-4.5"},
                {"provider": "grok", "id": "grok-legacy", "label": "legacy", "hidden": true}
              ],
              "active_provider": "grok",
              "default_models": {
                "global": ["claude/claude-opus-4.8", "grok/grok-4.5"],
                "projects": [
                  {"name": "acme", "default_models": ["grok/grok-4.5"],
                   "source": "/repos/acme/vars.yaml"}
                ]
              },
              "providers": [
                {"name": "claude", "enabled": true},
                {"name": "grok", "enabled": true},
                {"name": "codex", "enabled": false}
              ]
            }"#,
        )
        .unwrap();
        assert_eq!(s.catalog.len(), 4);
        assert_eq!(s.catalog[0].provider, "claude");
        assert_eq!(s.catalog[0].model_ref(), "claude/claude-fable-5");
        assert_eq!(s.catalog[0].model_display(), "claude-fable-5 (claude)");
        assert!(!s.catalog[0].hidden);
        // The hidden flag rides through so the picker can filter it.
        assert!(s.catalog[3].hidden);
        assert_eq!(s.active_provider, "grok");
        // default_models: project override wins, global otherwise.
        assert_eq!(s.default_models.global, vec!["claude/claude-opus-4.8", "grok/grok-4.5"]);
        assert_eq!(s.default_models.refs_for("acme"), vec!["grok/grok-4.5"]);
        assert_eq!(s.default_models.refs_for("other"), vec!["claude/claude-opus-4.8", "grok/grok-4.5"]);
        // providers reduced to name/enabled; a stale per-provider `models` key
        // would be silently ignored (no deny_unknown_fields).
        assert_eq!(s.providers.len(), 3);
        assert_eq!(s.providers[0].name, "claude");
        assert!(s.providers[0].enabled);
        assert_eq!(s.providers[0].bin, None);
        assert_eq!(s.providers[2].name, "codex");
        assert!(!s.providers[2].enabled);
    }

    #[test]
    fn settings_payload_stale_per_provider_models_key_is_ignored() {
        // A pre-Task-5 daemon still sends `models` per provider — the field is
        // gone from the struct, so serde drops it rather than erroring.
        let s: SettingsPayload = serde_json::from_str(
            r#"{"providers": [{"name": "grok", "enabled": true,
                "models": {"opus": "grok-code-fast-1"}}]}"#,
        )
        .unwrap();
        assert_eq!(s.providers.len(), 1);
        assert_eq!(s.providers[0].name, "grok");
        assert!(s.providers[0].enabled);
        // No catalog in this legacy shape → empty (picker falls back to built-ins).
        assert!(s.catalog.is_empty());
    }

    #[test]
    fn settings_payload_empty_object_defaults_without_error() {
        // An old daemon (predating the settings RPC) that somehow returns `{}` —
        // or any partial subtree — must default rather than panic. Every field is
        // `#[serde(default)]`, so the empty object is a fully-defaulted payload.
        let s: SettingsPayload = serde_json::from_str("{}").unwrap();
        assert_eq!(s, SettingsPayload::default());
        // The Task-5 fields all default: no catalog, empty active provider, empty
        // default_models — the picker then falls back to the built-in mirror.
        assert!(s.catalog.is_empty());
        assert_eq!(s.active_provider, "");
        assert!(s.default_models.global.is_empty());
        assert!(s.default_models.projects.is_empty());
        assert!(s.default_models.refs_for("anything").is_empty());
        // Pre-Task-12 daemon omits `providers` entirely → empty vec, not an error.
        assert!(s.providers.is_empty());
    }
}
