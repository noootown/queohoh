# Model Catalog + Provider Switch — Design

**Date:** 2026-07-16
**Status:** Approved (design), pending implementation plan
**Replaces:** the tier-alias / cross-provider-equivalence model system (`resolveProviderChain` tier fan-out, per-provider tier tables, `models:` alias tables)

## Motivation

The tier-equivalence abstraction ("`opus` in claude = `grok-4.5` in grok") caused real operational confusion: a grok-only `providers:` block in config.yaml silently reordered the effective fallback chain and put grok-4.5 at the head of every bare-tier task machine-wide (2026-07-16 incident: `pr-resolve-bot-comments` runs all landed on grok-4.5 and failed). The equivalence table is a hidden indirection nobody actually wants: cross-provider "same tier" is not a real thing, and the chain it generates is invisible at the point where a task declares `model: opus`.

This design kills the tier vocabulary entirely and replaces it with two explicit concepts:

1. A **model catalog** — the flat, ordered list of concrete models the daemon knows how to run.
2. **Per-task model lists** — each task/definition names concrete models in top→bottom priority order; the daemon rotates down the list only on availability failures.

Plus one operational control: a **provider switch** in the TUI (`p` key) that re-heads every chain to a chosen provider.

## Decisions log (from design discussion)

- Kill tiers entirely; concrete models are the only vocabulary. Existing defs are migrated in the same change.
- Catalog is built-in with config override (add/hide/reorder), not config-only and not derived from tier tables.
- Catalog ordering is **grouped by provider** (provider precedence: claude → grok → codex), each group internally ordered most→least powerful. Cross-provider power comparison is deliberately not modeled.
- Explicit task list = exact intent (a single-model list never rotates); absent list = global/per-project `default_models`.
- Provider switch: no "auto" state — it always points at a concrete enabled provider (default: precedence head, claude). The underlying list/rotation machinery is the "auto" behavior and keeps running beneath the switch.
- Switch semantics: **re-head, not filter** — matching-provider entries are promoted to the front of the task's chain; rotation across providers still applies after promotion. A task with no entry for the switched provider gets that provider's catalog group head prepended.
- Rotation trigger unchanged from today: availability-classified failures only (spawn failure + each adapter's `classifyUnavailable`). An availability failure skips the whole provider group for that task, not just the failed model.
- TUI pickers show every enabled catalog model as `label (provider)`, grouped in catalog order, with a "default" head option that leaves the model unset (def list / `default_models` applies). No list-builder widget (YAGNI).

## 1. The model catalog

One flat, ordered list of concrete models replaces all per-provider tier tables. Grouping and order are structural: providers appear in fixed precedence order (claude → grok → codex), and each provider's entries are ordered most→least powerful within the group.

Built-in catalog (shipped in `packages/core`):

```yaml
catalog:
  # claude (provider group 1)
  - { provider: claude, id: claude-fable-5,          label: fable }
  - { provider: claude, id: claude-opus-4-8,         label: opus }
  - { provider: claude, id: claude-sonnet-5,         label: sonnet }
  - { provider: claude, id: claude-haiku-4-5,        label: haiku }
  # grok (provider group 2)
  - { provider: grok,   id: grok-4.5,                label: grok-4.5 }
  - { provider: grok,   id: grok-composer-2.5-fast,  label: composer }
  # codex (provider group 3; provider ships enabled: false)
  - { provider: codex,  id: gpt-5.6-sol,             label: sol }
  - { provider: codex,  id: gpt-5.6-terra,           label: terra }
  - { provider: codex,  id: gpt-5.6-luna,            label: luna }
```

- **References** (defs, task fields, MCP params, TUI values): `provider/label` (e.g. `claude/opus`), with `provider/id` accepted as exact-match fallback. **Display**: `label (provider)`.
- **Config override** (`config.yaml` `catalog:`): per-entry merge keyed on `provider/id`. Config can add new entries, set `hidden: true`, and reorder. Entries config doesn't mention keep their built-in position; the merged list is re-grouped by provider precedence so a config reorder cannot interleave providers. `hidden` affects pickers only — a hidden entry still resolves when a def/task references it explicitly (hiding declutters, it never breaks a def). Labels must be unique within a provider; a config that collides two labels in one provider fails validation at load.
- `providers:` config keeps only spawn/enablement concerns: `enabled`, `bin`, `args`, `system_prompt`. The `models:` tier maps are removed. A disabled provider's catalog entries are skipped everywhere (pickers, resolution, switch cycle).
- "A provider's most powerful model" (used by the switch-miss rule) = first entry of that provider's group.

## 2. Per-task model lists & rotation

**Spec syntax** — the `model:` field (definitions, ad-hoc tasks, MCP `enqueue_task`/`run_task_definition`, chain steps) accepts a string or a list; a string is a 1-entry list:

```yaml
model: claude/opus                      # exactly this model, no rotation
model: [claude/opus, grok/grok-4.5]    # top→bottom priority, rotates on availability failure
```

- Bare tiers are gone. Unknown or unqualified values fail the task fast: `unknown model: opus (did you mean claude/opus?)`. Raw model ids no longer pass through silently — an unlisted model must be added to the catalog via config first.
- **Defaults**: new `default_models:` ordered list in global config, overridable per project in `vars.yaml`. It replaces `defaults.model`, the legacy global `models:` alias table, per-project `models:`/`default_model`, and per-project provider tier overrides — all removed. Tasks/defs with no `model:` use it. Initial value: `[claude/opus, grok/grok-4.5]`.
- **Explicit = exact**: a def that lists one model gets no rotation — it fails terminal when that model's provider is unavailable. What you write is what runs.

**Rotation** — trigger unchanged: only availability-classified failures rotate (runner spawn failure, or the provider adapter's `classifyUnavailable` matching out-of-credit / session-limit / quota wording); ordinary failures still settle `failed`.

- `attemptedProviders` becomes `attemptedModels` (`provider/id` strings) in the task record, but an availability failure marks the **whole provider group** as attempted for that task — out-of-credit/session-limit is account-scoped, so trying a second model on the same provider would just burn an attempt. Rotation therefore means: next list entry whose provider has not availability-failed this task.
- List exhausted → terminal failure, keeping today's per-attempt report trail (`attempt 1: claude/opus — session limit → falling back`, terminal attempt without the suffix) and today's rule that the terminal provider is not recorded into the skip set (a manual re-run retries it).
- **Resume tasks** are unchanged: pinned to the session lineage's provider, never rotate, run the model the session already ran on.

## 3. The provider switch

- **State**: `active_provider` — always a concrete enabled provider name. Default: the provider-precedence head (claude). Owned by the daemon, persisted in the daemon state dir, global (not per-project), so it applies to every launch path: TUI, MCP (`/qoo`), cron workers. Changed via a new IPC command; reported in the settings payload.
- **TUI**: `p` (unbound today) cycles through enabled providers in precedence order; the current value renders as a top-right header indicator; clicking the indicator also cycles (header chips already hit-test).
- **Engine behavior** (at run resolution, where the chain is built): stable-partition the task's effective list — entries whose provider is `active_provider` first (keeping their relative order), all other entries after. If the list contains no entry for the active provider, prepend that provider's catalog group head. Rotation (Section 2) then walks the reordered chain normally — the switch re-heads, it never filters, so fallback across providers keeps working underneath.
  - Example: `active_provider: grok`, task list `[claude/opus, grok/grok-4.5]` → effective chain `[grok/grok-4.5, claude/opus]`.
- Resume tasks ignore the switch (session-pinned).
- If config disables the currently-active provider, the daemon snaps the switch to the precedence-first enabled provider and logs it.

## 4. TUI surfaces

- **Settings payload** grows `catalog` (merged, ordered, hidden entries excluded), `active_provider`, and `default_models`, so the TUI renders from daemon truth (existing pattern).
- **Model dropdowns** (run form, ad-hoc create, create-worktree — all via `model_field`): options replace the hardcoded `MODEL_OPTIONS` tier array:
  - Head option `default (claude/opus → grok/grok-4.5)` — leaves the model unset so the def list / `default_models` applies. For a definition run the label shows the def's own list: `default (def: …)` (deferred: no def-launch model picker exists yet; seam in place). This is the preselect.
  - Then every visible catalog model of every enabled provider, in catalog order, rendered `label (provider)`; stored value `provider/label`.
- Picking a concrete model = a 1-entry list (exact, no rotation), per Section 2. Custom multi-model lists are authored in def yaml / MCP params only; no TUI list-builder (YAGNI).
- **Resume flows** preselect the session's actual model via the existing `preferred` mechanism, validated against the catalog.
- **Run detail / info tab** displays `label (provider)`; the raw model id stays available in the info block.
- **TUI fallback catalog**: the TUI carries a mirror of the built-in catalog for when the daemon's settings payload predates this change (same pattern as the current `MODEL_OPTIONS` fallback).

## 5. Migration & back-compat

- **config.yaml schema**: `providers[].models` is ignored with a startup warning (not a hard error). New optional `catalog:` overlay and `default_models:` list. Removed: global `models:` alias table, per-project `models:`, `default_model`, per-project provider tier overrides.
- **Definitions**: every def under the config workspace (`~/workspace/queohoh/*/tasks/*/config.yaml`) is migrated in the same change (`model: opus` → `model: claude/opus`, etc.).
- **Live config**: `~/workspace/queohoh/config.yaml` is rewritten to the new shape; grok's current `enabled: false` and bin pin carry over verbatim.
- **MCP/API**: `model` param accepts `provider/label`, `provider/id`, or a list; anything else errors with a did-you-mean naming catalog entries. Verify the `/qoo` skill passes no bare tiers.
- **Task store**: `attempted_providers` → `attempted_models`; the reader accepts the old key so queued tasks survive the upgrade. Session lineage provider tags unchanged.
- **Daemon/TUI skew**: handled by the TUI fallback catalog (Section 4).

## 6. Testing

- **Core** (`models.test.ts` rewritten; `config-providers.test.ts` adapted): catalog layering (built-in ⊕ config add/hide/reorder, provider re-grouping), reference parsing (`provider/label`, `provider/id`, string vs list, unknown-model error with did-you-mean, defaults resolution), active-provider partition + group-head prepend, rotation with provider-group skip, single-model no-rotation, resume pinning unaffected by switch, disabled-provider snap.
- **Worker** (`worker-fallback.test.ts` adapted): hop-trail wording with `provider/label`, exhausted-list terminal behavior, old `attempted_providers` key accepted on read.
- **TUI** (`form_tests.rs` + snapshot tests): grouped dropdown rendering with the `default (…)` head option, def-list vs ad-hoc default labeling, `p` cycle + indicator render + click-to-cycle, resume preselect validated against the catalog.

## Out of scope

- A TUI widget for composing custom multi-model lists.
- codex/OpenAI enablement (adapter exists; stays `enabled: false` until a subscription exists).
- Any change to resume/lineage semantics or to the availability-classification regexes (e.g. 529/overloaded handling is a separate concern).
- Per-model (rather than per-provider) availability skip granularity.
