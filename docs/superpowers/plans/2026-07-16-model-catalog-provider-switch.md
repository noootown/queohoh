# Model Catalog + Provider Switch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the tier-alias / cross-provider-equivalence model system with a concrete-model catalog, per-task priority lists with provider-group rotation, and a daemon-owned provider switch (TUI `p` cycle).

**Architecture:** A new `catalog.ts` core module owns the flat provider-grouped model list (built-in ⊕ config overlay). `models.ts` is rewritten around `resolveModelChain` (explicit list or `default_models`, partitioned by the daemon's persisted `active_provider`). The worker rotates on availability failures with provider-group skip via `attempted_models`. The daemon exposes `active_provider` over IPC; the TUI renders catalog-driven dropdowns (`label (provider)`), a top-right provider indicator, and a `p` cycle key.

**Tech Stack:** TypeScript (packages/core, packages/daemon — vitest), Rust (crates/qoo-tui — ratatui, cargo test).

**Spec:** `docs/superpowers/specs/2026-07-16-model-catalog-provider-switch-design.md` — read it before starting any task.

## Global Constraints

- Model references are `provider/label` (canonical) or `provider/id` (exact-match fallback); display form is `label (provider)`.
- Catalog order is grouped: provider precedence `claude → grok → codex`, most→least powerful within a group. A merge/overlay must never interleave providers.
- Bare tiers (`opus`) and raw model ids no longer resolve — they fail with `unknown model: <ref>` plus a did-you-mean naming a catalog entry.
- Explicit task list = exact (single entry ⇒ no rotation). Absent ⇒ `default_models` (project override > global > built-in `["claude/opus", "grok/grok-4.5"]`).
- Rotation triggers ONLY on availability-classified failures (unchanged vocabulary); an availability failure skips the whole provider group for that task.
- Resume tasks never rotate and ignore the provider switch (unchanged).
- All removed vocabulary (DEFAULT_MODEL_ALIASES, resolveModel, effectiveModelTable, resolveProviderChain, `models:` alias tables, provider tier maps, `default_model`) must be deleted, not deprecated.
- Old task files with `attempted_providers` must still parse (read-compat); new writes use `attempted_models`.
- Repo conventions: no Co-Authored-By trailers; commit per task with explicit paths (`git add <paths>`); TS suites `pnpm --filter @queohoh/core test` / `pnpm --filter @queohoh/daemon test`; Rust `cargo test -p qoo-tui`.

## File Structure

```
packages/core/src/catalog.ts            (new)  catalog types, built-ins, merge, lookup, formatting
packages/core/src/models.ts             (rewrite) resolveModelChain + chain types; tier machinery deleted
packages/core/src/config.ts             (modify) catalog overlay + default_models; alias/tier loaders deleted
packages/core/src/task.ts               (modify) model: string|string[]; attempted_models (+read-compat)
packages/core/src/worker.ts             (modify) chain build via resolveModelChain; provider-group skip
packages/daemon/src/engine.ts           (modify) worker deps: catalog/defaultModels/activeProvider
packages/daemon/src/api.ts              (modify) settings payload; set_active_provider; enqueue model param
packages/daemon/src/state.ts or daemon.ts (modify) persisted active_provider
crates/qoo-tui/src/ipc/types.rs         (modify) settings payload: catalog/active_provider/default_models
crates/qoo-tui/src/app/form.rs          (modify) catalog-driven dropdown with default head option
crates/qoo-tui/src/app/{actions.rs,keymap.rs,mod.rs} (modify) p-cycle action + IPC call
crates/qoo-tui/src/view/ (header site)  (modify) top-right active-provider indicator (+ click hit)
~/workspace/queohoh/**                  (migrate) defs model: values; config.yaml new shape
```

---

### Task 1: Core catalog module

**Files:**
- Create: `packages/core/src/catalog.ts`
- Test: `packages/core/src/__tests__/catalog.test.ts`

**Interfaces:**
- Consumes: `ProviderConfig` (type-only) from `./config.js` — mirror models.ts's type-only import rule so catalog.ts pulls in no fs/yaml runtime.
- Produces (later tasks rely on these exact names):

```ts
export interface CatalogEntry {
	provider: string;
	id: string;
	label: string;
	hidden?: boolean;
}
export const PROVIDER_PRECEDENCE: string[] = ["claude", "grok", "codex"];
export const BUILTIN_CATALOG: CatalogEntry[]; // grouped, per spec Section 1
export function effectiveCatalog(overlay: CatalogEntry[] | undefined): CatalogEntry[] | { error: string };
export function findModel(catalog: CatalogEntry[], ref: string): CatalogEntry | undefined;
export function unknownModelError(catalog: CatalogEntry[], ref: string): string; // "unknown model: X (did you mean provider/label?)"
export function groupHead(catalog: CatalogEntry[], provider: string): CatalogEntry | undefined;
export function formatModel(e: CatalogEntry): string; // "label (provider)"
export function modelRef(e: CatalogEntry): string;    // "provider/label"
```

- `BUILTIN_CATALOG` content (exact, from spec): claude → `claude-fable-5`/fable, `claude-opus-4-8`/opus, `claude-sonnet-5`/sonnet, `claude-haiku-4-5`/haiku; grok → `grok-4.5`/grok-4.5, `grok-composer-2.5-fast`/composer; codex → `gpt-5.6-sol`/sol, `gpt-5.6-terra`/terra, `gpt-5.6-luna`/luna.
- `effectiveCatalog` merge rules: overlay entries merge by `provider + "/" + id` (overlay wins per field; unmentioned built-ins keep position); overlay-new entries append at the END of their provider's group; unknown provider in overlay creates a trailing group after `PROVIDER_PRECEDENCE`; result is re-grouped by provider precedence so ordering within a group is (built-in order, then overlay-added order) and groups never interleave. Duplicate `label` within one provider → `{ error: "catalog: duplicate label <label> in provider <p>" }`.
- `findModel`: split on first `/`; match label first, then id, within that provider only; hidden entries still match (spec: hidden is picker-only).
- `unknownModelError` did-you-mean: if the part after `/` (or the whole ref when no `/`) equals some entry's label or id in ANY provider, suggest that entry's `provider/label`; otherwise no suggestion suffix.

- [ ] **Step 1: Write failing tests** in `packages/core/src/__tests__/catalog.test.ts` covering: built-in grouping order; overlay reorder-within-group; overlay add (appends to its group); overlay `hidden: true` preserved on entry; overlay cannot interleave groups; duplicate-label error; `findModel` by label, by id, with hidden entry, unknown → undefined; `unknownModelError("opus")` → `unknown model: opus (did you mean claude/opus?)`; `groupHead` returns first entry of group and `undefined` for unknown provider; `formatModel`/`modelRef` shapes.
- [ ] **Step 2: Run** `pnpm --filter @queohoh/core test -- catalog` → FAIL (module not found).
- [ ] **Step 3: Implement `catalog.ts`** exactly to the Produces block above.
- [ ] **Step 4: Run the catalog tests** → PASS; run the whole core suite → no regressions (nothing imports catalog yet).
- [ ] **Step 5: Commit** `feat(core): model catalog module (built-in + overlay merge)` with explicit paths.

### Task 2: Core chain resolution (models.ts rewrite)

**Files:**
- Modify: `packages/core/src/models.ts` (full rewrite)
- Test: `packages/core/src/__tests__/models.test.ts` (full rewrite)

**Interfaces:**
- Consumes: Task 1's catalog API; `ProviderConfig` (type-only, for `enabled`).
- Produces:

```ts
export interface ChainEntry { provider: string; model: string; ref: string } // model = concrete id, ref = "provider/label"
export type ChainResult = { ok: true; chain: ChainEntry[] } | { ok: false; error: string };
export function resolveModelChain(
	spec: string | string[] | null,      // task/def model field; null ⇒ defaults
	catalog: CatalogEntry[],
	providers: ProviderConfig[],          // for enabled checks
	defaultModels: string[],              // refs, already project-resolved by caller
	activeProvider: string,               // always a concrete provider name
): ChainResult;
```

- Resolution algorithm (implement exactly):
  1. `refs = spec === null ? defaultModels : (typeof spec === "string" ? [spec] : spec)`.
  2. Map each ref via `findModel`; any miss ⇒ `{ ok: false, error: unknownModelError(catalog, ref) }`.
  3. Drop entries whose provider is disabled/unknown in `providers`.
  4. Stable-partition: entries with `provider === activeProvider` first (keeping order), rest after.
  5. If no entry has `provider === activeProvider` AND that provider is enabled: prepend `groupHead(catalog, activeProvider)` (skip prepend if the group is empty).
  6. Dedup by `provider/id` keeping first occurrence. Empty final chain ⇒ `{ ok: false, error: "no runnable model: all configured models are on disabled providers" }`.
- Delete: `DEFAULT_MODEL_ALIASES`, `resolveModel`, `effectiveModelTable`, `resolveProviderChain`. Grep the whole repo for each name and fix every importer within the tasks that own those files (worker in Task 4, daemon in Task 5, TUI fallback in Task 7); THIS task only updates `packages/core/src/index.ts`-style barrel exports if any exist (grep first) and must leave the core package compiling (`pnpm --filter @queohoh/core build`) — stale importers in core get fixed here, cross-package importers in their own tasks.
- [ ] **Step 1: Rewrite `models.test.ts`** — cases: null spec uses defaults; string spec = 1-entry chain; list order kept; unknown ref error text; disabled-provider entries dropped; active-provider partition (`[claude/opus, grok/grok-4.5]` + grok ⇒ grok head); switch-miss prepends group head; switch-miss with disabled active provider does NOT prepend; dedup; all-disabled ⇒ error.
- [ ] **Step 2: Run** → FAIL (resolveModelChain undefined).
- [ ] **Step 3: Implement** per the algorithm. **Step 4: Core suite green** (worker tests will break only after Task 4 — if they break now, you removed something Task 4 owns; re-read step 3's scoping note). 
- [ ] **Step 5: Commit** `feat(core): resolveModelChain over the catalog; tier machinery removed`.

### Task 3: Config + task schema (catalog overlay, default_models, attempted_models)

**Files:**
- Modify: `packages/core/src/config.ts`, `packages/core/src/task.ts`
- Test: `packages/core/src/__tests__/config-providers.test.ts` (adapt), task schema test file (locate via `grep -rl attempted_providers packages/core/src/__tests__`)

**Interfaces:**
- Produces on `GlobalConfig`: `catalog: CatalogEntry[]` (already merged via `effectiveCatalog`; a malformed/duplicate-label overlay warns and falls back to `BUILTIN_CATALOG` — mirror the existing tolerant-providers pattern), `defaultModels: string[]` (config `default_models:`, default `["claude/opus", "grok/grok-4.5"]`). REMOVED from `GlobalConfig`: `models`.
- Produces in config.ts: `loadProjectDefaultModels(projectDir: string): string[] | undefined` (vars.yaml `default_models:`, tolerant). DELETED: `loadProjectModels`, `loadProjectDefaultModel`, `loadProjectProviderModels`, the `models:` field parse in `loadGlobalConfig`, and `ProviderConfigSchema.models` (schema now warns `providers[].models is no longer read; use catalog:` when the key is present, then ignores it — keep the key out of the parsed `ProviderConfig`; update `effectiveProviders` and `DEFAULT_PROVIDERS` to drop the `models` field entirely). Keep the reserved-key skips in `loadProjectVars` for `models`/`default_model`/`providers` (old vars.yaml files must not crash var loading) and add `default_models`.
- Produces on the task schema (task.ts): `model: z.union([z.string(), z.array(z.string())]).nullable().default(null)` (TaskInstance field `model: string | string[] | null`); `attempted_models: z.array(z.string()).default([])` with read-compat: the frontmatter parser must accept legacy `attempted_providers` (map bare provider names into `attemptedModels` verbatim — the worker's group-skip in Task 4 treats a bare provider name as "whole group attempted"); writes emit only `attempted_models`. `TaskInstance.attemptedProviders` renames to `attemptedModels` (fix all core-internal usages; cross-package in Tasks 4–5).
- [ ] **Step 1: Failing tests**: config catalog overlay parsed+merged; malformed overlay falls back with warning; `default_models` global + project override loader; `providers[].models` warn-and-ignore; task frontmatter round-trips list `model`; legacy `attempted_providers` frontmatter parses into `attemptedModels`; new writes emit `attempted_models` key.
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4: Core suite green** (worker.ts may need mechanical renames to keep compiling — do the minimal rename here; behavior change is Task 4's).
- [ ] **Step 5: Commit** `feat(core): catalog overlay + default_models config; task model lists + attempted_models`.

### Task 4: Worker chain build + provider-group rotation

**Files:**
- Modify: `packages/core/src/worker.ts` (`WorkerDeps`, `resolveRunContext` ~lines 171–236, `finalizeRun` retry block ~lines 480–640)
- Test: `packages/core/src/__tests__/worker-fallback.test.ts` (adapt), `execute-run.test.ts` (adapt as needed)

**Interfaces:**
- Consumes: `resolveModelChain` (Task 2), `GlobalConfig.catalog`/`defaultModels` shapes (Task 3).
- Produces on `WorkerDeps`: `catalog: CatalogEntry[]`, `defaultModels: string[]` (already project-resolved by the engine), `activeProvider: string`. REMOVED: `modelTable`, the `defaults.model` default-model leg (keep `defaults.timeoutMs`).
- `resolveRunContext` changes: `modelSpec = def?.model ?? task.model ?? null` (def schema: definition.ts `model` field must also accept `string | string[]` — include that schema change here since the worker owns def model consumption; grep `definition.ts` for the field). Chain = `resolveModelChain(modelSpec, deps.catalog, providers, deps.defaultModels, deps.activeProvider)`; then filter `!task.attemptedModels.includes(e.ref) && !task.attemptedModels.includes(e.provider)` (the second clause implements provider-group skip AND legacy bare-provider compat in one predicate — attempted entries are written as bare provider names, see finalizeRun below). Resume pinning keeps its shape but the pinned fallback entry (chain.find by provider / groupHead construction) resolves via the catalog: pinned provider's entry for the task's ref if present, else `groupHead(catalog, pinnedProvider)`, else the existing `resume provider unavailable` failure.
- `finalizeRun` changes: on retry-eligible availability failure, append the failed **provider name** (not the ref) to `attemptedModels` (this is the group-skip: the filter clause above matches it against `e.provider`); hop-trail wording becomes `attempt N: <ref> — <reason> → falling back` (ref = `chain[0].ref`, e.g. `claude/opus`); terminal attempt line likewise uses the ref. The existing rule stands: the terminal provider is NOT appended.
- [ ] **Step 1: Adapt/extend worker-fallback tests**: two-entry list rotates claude→grok on session-limit; single-entry list settles terminal (no retry) on availability failure; provider-group skip (list `[claude/opus, claude/sonnet, grok/grok-4.5]`, claude availability-fails ⇒ next attempt is grok, sonnet never tried); legacy task with `attemptedProviders: ["claude"]` (now surfaced as `attemptedModels: ["claude"]`) skips all claude entries; hop-trail wording; resume pin unaffected by `activeProvider`; activeProvider=grok re-heads.
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4:** full core suite green.
- [ ] **Step 5: Commit** `feat(core): worker chains via resolveModelChain with provider-group rotation`.

### Task 5: Daemon — active_provider state, settings payload, enqueue validation

**Files:**
- Modify: `packages/daemon/src/engine.ts` (worker-deps block ~lines 1110–1150), `packages/daemon/src/api.ts` (settings case ~line 216, enqueue case, new `set_active_provider` case), `packages/daemon/src/daemon.ts` (state wiring)
- Test: daemon test files (locate via `grep -rln "settings" packages/daemon/src/__tests__` and follow the existing API-method test idiom)

**Interfaces:**
- Consumes: everything above.
- Produces: persisted `active_provider` in `<state>/daemon/settings.json` (`{ "active_provider": "claude" }`; missing/corrupt file ⇒ precedence-first enabled provider; write-through on change). New API method `set_active_provider { provider: string }` → validates the provider exists AND is enabled (error string otherwise), persists, returns the new value, and pushes a state broadcast to subscribers (follow the existing subscribe/broadcast pattern). Config-load snap: if the persisted provider is disabled/unknown after a (re)load, snap to precedence-first enabled and log.
- Settings payload (`settings` case) REPLACES the `models:` block with: `catalog` (merged entries incl. `hidden` flag — the TUI filters), `active_provider`, `default_models: { global: string[], projects: [{ name, default_models, source }] }`, and keeps `providers` (name/enabled only — drop the `models` map). Engine worker-deps: pass `catalog: deps.config.catalog`, `defaultModels: loadProjectDefaultModels(projectWorkspaceDir(...)) ?? deps.config.defaultModels`, `activeProvider: <current persisted value>`; delete the `modelTable`/`defaultModel`/`loadProjectProviderModels` plumbing.
- Enqueue/MCP `model` param (api.ts enqueue case; also `run_task_definition`/chain paths — grep `params.model`): accept string or string[]; validate every ref via `findModel` against the merged catalog; invalid ⇒ the enqueue fails with `unknownModelError` text (existing error-return idiom). MCP tool schema in `packages/daemon/src/mcp.ts`: widen the `model` input to `string | string[]` and update its description to name the `provider/label` form.
- [ ] **Step 1: Failing tests**: settings payload shape; set_active_provider happy path + disabled-provider rejection + persistence round-trip; snap-on-reload; enqueue rejects `model: "opus"` with did-you-mean; enqueue accepts `model: ["claude/opus","grok/grok-4.5"]`.
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4:** daemon + core suites green.
- [ ] **Step 5: Commit** `feat(daemon): active_provider state + catalog settings payload + model ref validation`.

### Task 6: Config-repo migration (defs + live config.yaml)

**Files:**
- Modify: every `~/workspace/queohoh/*/tasks/*/config.yaml` with a `model:` key, `~/workspace/queohoh/*/vars.yaml` with `models:`/`default_model:`/`providers:` keys, `~/workspace/queohoh/config.yaml`
- No test files — verification is behavioral (this is the user's config repo, a separate git repo; commit there, do not push unless the repo's existing hooks do).

**Interfaces:** consumes the new vocabulary only; produces nothing for later tasks (Tasks 7–8 are TUI-side).

- [ ] **Step 1: Inventory** — `grep -rn "^model:" ~/workspace/queohoh/*/tasks/*/config.yaml` and `grep -rn "models:\|default_model\|providers:" ~/workspace/queohoh/*/vars.yaml ~/workspace/queohoh/config.yaml`. Also `grep -rn "model" ~/.claude/skills/qoo/SKILL.md` (bare-tier mentions in the /qoo skill need the new refs).
- [ ] **Step 2: Migrate defs** — `model: opus` → `model: claude/opus`, `model: sonnet` → `model: claude/sonnet`, etc. Any def that WANTS fallback gets a list (decide per def comment intent; default: keep single = exact, and note it in the commit message).
- [ ] **Step 3: Rewrite `config.yaml`** `providers:` block to the new shape — claude first, grok `enabled: false` + `bin:` preserved verbatim, add `default_models: [claude/opus]` (grok disabled today, so listing it would be dropped at resolve time anyway — add `grok/grok-4.5` back when re-enabling), delete any `models:` keys. Keep the 2026-07-16 gotcha comment, updated to the new vocabulary.
- [ ] **Step 4: Verify** — `queohoh reload` (rebuilds daemon on the new code — coordinate with Task 8's final reload if executing sequentially; running it twice is harmless), then `queohoh status`; enqueue a trivial ad-hoc task via MCP with `model: claude/haiku` and confirm it runs; confirm a def-launched task resolves (definition list in TUI or `run_task_definition`).
- [ ] **Step 5: Commit** (config repo) `config(queohoh): migrate defs and config to catalog model refs`.

### Task 7: TUI — settings types + catalog dropdown

**Files:**
- Modify: `crates/qoo-tui/src/ipc/types.rs` (settings payload structs), `crates/qoo-tui/src/app/form.rs` (MODEL_OPTIONS/model_options/resolve_default_model/model_field_defaulting)
- Test: `crates/qoo-tui/src/app/form_tests.rs` (adapt + extend)

**Interfaces:**
- Consumes: Task 5's settings payload JSON shape (field names verbatim: `catalog[].{provider,id,label,hidden}`, `active_provider`, `default_models.{global,projects}`).
- Produces (Rust, used by Task 8): `SettingsPayload.catalog: Vec<CatalogEntry>`, `SettingsPayload.active_provider: String`; form value vocabulary `provider/label` submitted as the task/def `model` value; fn `model_display(entry) -> String` (`"label (provider)"`); the dropdown's head option value is the empty string `""` (= leave model unset) displayed as `default (<refs joined with → >)` — resolve the shown refs from the def's model when launching a definition, else the repo's `default_models` (project override, else global).
- `MODEL_OPTIONS` is deleted; the TUI fallback when settings are absent/stale is a hardcoded mirror of `BUILTIN_CATALOG` (claude+grok groups only — codex is disabled by default and invisible anyway; comment points at catalog.ts as the source to keep in sync). Hidden entries and disabled providers are filtered from options. `resolve_default_model` becomes `default_refs_for(repo) -> Vec<String>` feeding the head-option label; `model_field_defaulting`'s `preferred` (resume) validates against the option values (`provider/label` refs).
- [ ] **Step 1: Failing tests** in form_tests.rs: options = head `""` + grouped `provider/label` values in catalog order; display strings `label (provider)`; hidden filtered; disabled provider filtered; def-launch head label shows def list; resume preferred selects its ref; stale-settings fallback mirrors built-ins.
- [ ] **Step 2: Run** `cargo test -p qoo-tui` → FAIL. **Step 3: Implement** (types.rs serde structs first — keep old fields `#[serde(default)]`-tolerant so an old daemon payload deserializes; form.rs second). **Step 4:** `cargo test -p qoo-tui` green.
- [ ] **Step 5: Commit** `feat(tui): catalog-driven model dropdown with default head option`.

### Task 8: TUI — provider switch key + indicator

**Files:**
- Modify: `crates/qoo-tui/src/keymap.rs` (bind `p` in list mode), `crates/qoo-tui/src/app/actions.rs` (new action `CycleProvider`), `crates/qoo-tui/src/ipc/` (client call for `set_active_provider`), the top-bar render site (locate: `grep -rn "build_header\|top" crates/qoo-tui/src/view/panes.rs` and the frame-level header in `view/mod.rs`), `crates/qoo-tui/src/hit.rs` + `app/mouse.rs` (click target)
- Test: `crates/qoo-tui/src/app/tests.rs` + snapshot tests in `selectors.rs`/view tests (follow whichever file style-tests the header today)

**Interfaces:**
- Consumes: `SettingsPayload.active_provider` + enabled providers list (Task 7 types); daemon method name `set_active_provider` with param `{ provider: <name> }` (Task 5).
- Produces: `Action::CycleProvider` — computes next = enabled providers in precedence order, cyclic from current, sends `set_active_provider`, optimistically updates the local settings copy (daemon broadcast reconciles). Indicator: top-right, text `⚡ <provider>`, distinct style per provider name (reuse the theme's accent colors; exact styling is implementer's judgment against `view/theme.rs`), always visible. New `HitTarget::ProviderIndicator` cycles on click.
- [ ] **Step 1: Failing tests**: `p` triggers CycleProvider and the IPC call payload names the next enabled provider (skip disabled); indicator renders current provider; click on indicator cycles; single-enabled-provider cycle is a no-op (stays put, no IPC).
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4:** `cargo test -p qoo-tui` green + `cargo clippy -p qoo-tui` clean.
- [ ] **Step 5: Commit** `feat(tui): provider switch — p cycles active provider, top-right indicator`.

### Task 9: Integration pass + docs

**Files:**
- Modify: `AGENTS.md` / `README` sections that document `model:`/tiers (locate: `grep -rn "opus\|model:" AGENTS.md README.md docs/ --include="*.md" -l`, skip specs/plans), `.mise.toml` comments if they mention tiers.
- No new tests; this is the whole-branch verification gate.

- [ ] **Step 1:** `grep -rn "resolveProviderChain\|DEFAULT_MODEL_ALIASES\|effectiveModelTable\|resolveModel\b\|modelTable\|attempted_providers\|attemptedProviders\|MODEL_OPTIONS\|default_model\b" packages crates --include="*.ts" --include="*.rs"` — every hit must be either the read-compat parser (task.ts) or deliberately dead; fix stragglers.
- [ ] **Step 2:** Full suites: core, daemon, `cargo test -p qoo-tui`, `cargo clippy -p qoo-tui`. All green.
- [ ] **Step 3:** Update stale docs found by the grep in Files.
- [ ] **Step 4:** `queohoh reload`; `mise run tui` smoke: dropdown shows `opus (claude)` form, `p` cycles, indicator renders; enqueue an ad-hoc `claude/haiku` task end-to-end.
- [ ] **Step 5: Commit** `chore: catalog cutover integration pass — dead vocabulary removed, docs updated`.

---

## Self-review notes (already applied)

- Spec coverage: Section 1 → Tasks 1/3; Section 2 → Tasks 2/3/4 (+def schema in Task 4); Section 3 → Tasks 5/8; Section 4 → Tasks 7/8; Section 5 → Tasks 3 (read-compat), 5 (MCP), 6 (config repo), 7 (skew fallback); Section 6 → per-task tests.
- Type consistency: `ChainEntry.ref` introduced in Task 2 is what Task 4's hop trail and attempted-filter use; `attemptedModels` holds refs OR bare provider names, and Task 4's single filter predicate handles both — this is deliberate (group skip and legacy compat share one representation).
- Sequencing: Tasks 1→2→3→4→5 are strictly ordered (each consumes the previous); 7→8 ordered; 6 needs 5 deployed to fully verify but its file edits can be prepared any time after 2; 9 last.
