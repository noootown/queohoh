# Model Aliases + TUI Settings Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Alias→model resolution (built-in defaults ← global config ← per-project override) applied at the worker spawn point, surfaced in TASKS rows and a new read-only `s` settings overlay in the TUI.

**Architecture:** Pure resolve/merge helpers in `packages/core/src/models.ts`; config loaders gain a `models:` map (global `config.yaml`) and a reserved `models:` block in per-project `vars.yaml`; the daemon resolves definition-summary models per repo and exposes a `settings` RPC; the Rust TUI mirrors the payload, fetches lazily (same dedup pattern as `reconcile_full_def`), and renders an overlay modeled on the `?` help overlay.

**Tech Stack:** TypeScript (zod, js-yaml, vitest) in packages/core + packages/daemon; Rust (ratatui, serde, insta) in crates/qoo-tui.

**Spec:** `docs/superpowers/specs/2026-07-10-model-aliases-design.md`

## Global Constraints

- Built-in defaults, verbatim: `fable→claude-fable-5`, `sonnet→claude-sonnet-5`, `opus→claude-opus-4-8`, `haiku→claude-haiku-4-5` (plain ids — these models are 1M-context natively; no suffix).
- Resolution is `table[name] ?? name` — unknown names (including full ids) pass through untouched. Single lookup, never chained.
- **Trap:** `loadProjectVars` (`packages/core/src/config.ts:103`) throws `non-scalar var: models` if vars.yaml gains a `models:` block. Task 2 must skip the reserved key.
- Old-daemon compat: TUI treats a missing `settings` RPC / missing field as "settings unavailable", never a crash.
- Verify gate per task: `pnpm -r build && pnpm -r test` (TS tasks), `cargo test -p qoo-tui` (Rust tasks). Final task runs `mise run check`.
- Commit after each task (conventional message, no Co-Authored-By trailer). Do not run `INSTA_UPDATE=always` blindly — inspect snapshot diffs first.

---

### Task 1: core — resolve + merge helpers

**Files:**
- Create: `packages/core/src/models.ts`
- Test: `packages/core/src/__tests__/models.test.ts`
- Modify: `packages/core/src/index.ts` (add `export * from "./models.js";` beside the existing exports)

**Interfaces:**
- Produces: `DEFAULT_MODEL_ALIASES: Record<string, string>`, `resolveModel(name: string, table: Record<string, string>): string`, `effectiveModelTable(global: Record<string, string>, project: Record<string, string>): Record<string, string>`.

- [x] **Step 1: Write the failing tests**

```ts
// packages/core/src/__tests__/models.test.ts
import { describe, expect, it } from "vitest";
import {
	DEFAULT_MODEL_ALIASES,
	effectiveModelTable,
	resolveModel,
} from "../models.js";

describe("resolveModel", () => {
	it("resolves a known alias", () => {
		expect(resolveModel("sonnet", { sonnet: "claude-sonnet-5" })).toBe(
			"claude-sonnet-5",
		);
	});
	it("passes unknown names through untouched (full ids keep working)", () => {
		expect(resolveModel("claude-fable-5", { sonnet: "x" })).toBe(
			"claude-fable-5",
		);
	});
	it("passes through on an empty table", () => {
		expect(resolveModel("opus", {})).toBe("opus");
	});
});

describe("effectiveModelTable", () => {
	it("layers defaults <- global <- project per key", () => {
		const t = effectiveModelTable(
			{ sonnet: "claude-sonnet-4-6" },
			{ opus: "claude-opus-4-7" },
		);
		expect(t.sonnet).toBe("claude-sonnet-4-6"); // global override
		expect(t.opus).toBe("claude-opus-4-7"); // project override wins
		expect(t.fable).toBe(DEFAULT_MODEL_ALIASES.fable); // default inherited
		expect(t.haiku).toBe("claude-haiku-4-5");
	});
	it("project overrides global for the same key", () => {
		const t = effectiveModelTable({ sonnet: "a" }, { sonnet: "b" });
		expect(t.sonnet).toBe("b");
	});
});
```

- [x] **Step 2: Run to verify failure** — `pnpm --filter @queohoh/core test` → FAIL (module not found).

- [x] **Step 3: Implement**

```ts
// packages/core/src/models.ts
/**
 * Model alias resolution (agent247-style). Definitions and tasks name models
 * by short alias ("sonnet"); the worker resolves the alias against the
 * effective per-project table just before spawning claude. Unknown names —
 * including full model ids — pass through untouched, so nothing breaks when a
 * caller already supplies a concrete id.
 */

/** Built-in defaults; global config.yaml `models:` and a project vars.yaml
 * `models:` block layer on top (later wins, merged per key). */
export const DEFAULT_MODEL_ALIASES: Record<string, string> = {
	fable: "claude-fable-5",
	sonnet: "claude-sonnet-5",
	opus: "claude-opus-4-8",
	haiku: "claude-haiku-4-5",
};

export function resolveModel(
	name: string,
	table: Record<string, string>,
): string {
	return table[name] ?? name;
}

export function effectiveModelTable(
	global: Record<string, string>,
	project: Record<string, string>,
): Record<string, string> {
	return { ...DEFAULT_MODEL_ALIASES, ...global, ...project };
}
```

- [x] **Step 4: Run to verify pass** — `pnpm --filter @queohoh/core test` → all green (202 + 5 new).
- [x] **Step 5: Commit** — `git add packages/core/src/models.ts packages/core/src/__tests__/models.test.ts packages/core/src/index.ts && git commit -m "feat(core): model alias resolve + three-layer merge helpers"` (check `index.ts` exists first — if core has no barrel, export from wherever `config.ts` exports are re-exported, or skip the barrel edit and import by path downstream).

---

### Task 2: core — config surfaces (global `models:`, project `models:` block)

**Files:**
- Modify: `packages/core/src/config.ts` (schema line ~20, `GlobalConfig` interface ~36-42, return ~60, `loadProjectVars` ~103-118)
- Test: extend the existing config test file (`packages/core/src/__tests__/config.test.ts` — locate with `ls packages/core/src/__tests__/`)

**Interfaces:**
- Consumes: nothing from Task 1 (independent).
- Produces: `GlobalConfig.models: Record<string, string>`; `loadProjectModels(projectDir: string): Record<string, string>`; `loadProjectVars` now SKIPS the reserved `models` key instead of throwing.

- [x] **Step 1: Write the failing tests** (follow the file's existing tmpdir fixture style):

```ts
it("parses a global models: map and defaults to empty", () => {
	// write config.yaml with `models:\n  sonnet: claude-sonnet-4-6`
	const config = loadGlobalConfig(pathWithModels);
	expect(config.models).toEqual({ sonnet: "claude-sonnet-4-6" });
	const bare = loadGlobalConfig(pathWithoutModels);
	expect(bare.models).toEqual({});
});

it("loadProjectVars skips the reserved models block instead of throwing", () => {
	// vars.yaml: `ticket: JUS-1\nmodels:\n  sonnet: claude-sonnet-4-6`
	expect(loadProjectVars(dir)).toEqual({ ticket: "JUS-1" });
});

it("loadProjectModels reads the block and tolerates absence/garbage", () => {
	expect(loadProjectModels(dirWithBlock)).toEqual({
		sonnet: "claude-sonnet-4-6",
	});
	expect(loadProjectModels(dirWithoutVarsYaml)).toEqual({});
	// vars.yaml: `models: [not, a, map]` → {} (tolerant, like agent247)
	expect(loadProjectModels(dirWithGarbage)).toEqual({});
});
```

- [x] **Step 2: Run to verify failure** — `pnpm --filter @queohoh/core test` → FAIL.
- [x] **Step 3: Implement.** In `GlobalConfigSchema` add `models: z.record(z.string(), z.string()).default({}),`; add `models: Record<string, string>;` to `GlobalConfig`; add `models: config.models,` to the return. In `loadProjectVars`, before the non-scalar throw: `if (key === "models") continue; // reserved: read by loadProjectModels`. Add:

```ts
/** The project's `models:` alias overrides from vars.yaml. Tolerant: absent
 * file, absent key, or a non-map value all yield {} (a bad block must never
 * take down config loading — it only disables the override). Non-string
 * values are skipped. */
export function loadProjectModels(
	projectDir: string,
): Record<string, string> {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return {};
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return {};
	const block = (raw as Record<string, unknown>).models;
	if (block === null || typeof block !== "object" || Array.isArray(block))
		return {};
	const out: Record<string, string> = {};
	for (const [alias, id] of Object.entries(block)) {
		if (typeof id === "string" && id.length > 0) out[alias] = id;
	}
	return out;
}
```

- [x] **Step 4: Run to verify pass**, and run daemon tests too (`pnpm -r test`) — engine fixtures construct `GlobalConfig` literals which now need the `models: {}` field; fix those fixtures (search `workspace: join(base, "ws")` in `packages/daemon/src/__tests__/engine.test.ts` setup and any other `GlobalConfig` literal).
- [x] **Step 5: Commit** — `feat(core): global models: map + per-project models block in vars.yaml`.

---

### Task 3: core+daemon — apply resolution at the worker spawn point

**Files:**
- Modify: `packages/core/src/worker.ts` (deps interface ~24-27, choke point ~105)
- Modify: `packages/daemon/src/engine.ts` (worker-deps construction ~400-452, where `repoVars` is loaded)
- Test: extend `packages/core/src/__tests__/worker.test.ts` and `packages/daemon/src/__tests__/engine.test.ts`

**Interfaces:**
- Consumes: `resolveModel`, `effectiveModelTable`, `loadProjectModels`, `GlobalConfig.models` (Tasks 1–2).
- Produces: `WorkerDeps.modelTable?: Record<string, string>` — the EFFECTIVE table for the task's repo (already merged); worker applies `resolveModel(model, deps.modelTable ?? {})`.

- [x] **Step 1: Failing worker test** — construct the existing worker test harness with `modelTable: { sonnet: "claude-sonnet-4-6" }`, a def whose `model` is `"sonnet"`, and assert the snapshot/spawn model is `"claude-sonnet-4-6"`; second case: def model `"claude-fable-5"` with the same table stays `"claude-fable-5"`.
- [x] **Step 2: Run to verify failure.**
- [x] **Step 3: Implement.** worker.ts: add `modelTable?: Record<string, string>;` to the deps interface (doc comment: "effective alias→id table for the task's repo; absent = no resolution (old callers)"), then change the choke point to:

```ts
const model = resolveModel(
	def?.model ?? task.model ?? deps.defaults.model,
	deps.modelTable ?? {},
);
```

engine.ts: next to where `repoVars` is loaded for the worker (line ~401 — it comes from `loadProjectVars(projectWorkspaceDir(config, repo))`), compute and pass:

```ts
modelTable: effectiveModelTable(
	this.deps.config.models,
	loadProjectModels(projectWorkspaceDir(this.deps.config, task.target.repo)),
),
```

(match the actual local variable names at the call site — read the surrounding lines first).
- [x] **Step 4: Engine test** — fixture writes `ws/<repo>/vars.yaml` with a `models:` block, runs a task with def model `"sonnet"`, asserts the recorded run snapshot model is the override. Run `pnpm -r test` → green.
- [x] **Step 5: Commit** — `feat: resolve model aliases per project at worker spawn`.

---

### Task 4: daemon — resolved summaries + `settings` RPC

**Files:**
- Modify: `packages/daemon/src/api.ts` (`case "definitions"` ~233: wrap `model: def.model` with per-repo resolution; new `case "settings"` beside `case "ping"` ~175)
- Test: extend `packages/daemon/src/__tests__/api.test.ts` + `pr-review-shape.test.ts` (expected `model` values become resolved ids, e.g. `"sonnet"` → `"claude-sonnet-5"`, `"opus"` → `"claude-opus-4-8"`)

**Interfaces:**
- Consumes: Task 1–2 helpers.
- Produces: `definitions` summaries carry RESOLVED ids; `settings` RPC returns exactly the spec shape:

```jsonc
{ "models": {
    "defaults": { "fable": "claude-fable-5", ... },
    "global":   { "entries": {...}, "source": "<config.yaml path>" },
    "projects": [ { "repo": "...", "entries": {...}, "source": "<vars.yaml path>" } ] } }
```

- [x] **Step 1: Failing tests** — (a) shape test for `settings` with a global override + one project block (only overriding projects listed; empty `projects: []` otherwise); (b) definitions test asserting `model` is the resolved id under a project override fixture.
- [x] **Step 2: Run to verify failure.**
- [x] **Step 3: Implement.** In `case "definitions"`, compute `const table = effectiveModelTable(deps.config.models, loadProjectModels(projectWorkspaceDir(deps.config, project.name)))` once per project loop iteration and set `model: resolveModel(def.model, table)` in both `byName.set` calls. Add:

```ts
case "settings": {
	const projects = deps.config.projects
		.map((p) => ({
			repo: p.name,
			entries: loadProjectModels(projectWorkspaceDir(deps.config, p.name)),
			source: join(projectWorkspaceDir(deps.config, p.name), "vars.yaml"),
		}))
		.filter((p) => Object.keys(p.entries).length > 0);
	return {
		models: {
			defaults: DEFAULT_MODEL_ALIASES,
			global: { entries: deps.config.models, source: configPath() },
			projects,
		},
	};
}
```

(`configPath()` is the existing helper used at daemon boot — import it, or thread the resolved path through deps if it isn't importable in api.ts; check `packages/daemon/src/daemon.ts` line ~40 for its module.)
- [x] **Step 4: Run `pnpm -r build && pnpm -r test`** → green.
- [x] **Step 5: Commit** — `feat(daemon): resolved models in definition summaries + settings RPC`.

---

### Task 5: TUI — settings fetch plumbing + `s` overlay

**Files:**
- Modify: `crates/qoo-tui/src/ipc/types.rs` (settings payload mirror), `crates/qoo-tui/src/event.rs` (Cmd + Event + executor arm), `crates/qoo-tui/src/keymap.rs` (`s` → new action), `crates/qoo-tui/src/app.rs` (state, action handling, event arm), `crates/qoo-tui/src/view/help.rs` (add `("s", "settings — model table")` to `KEYMAP_ROWS`; also check the bottom footer hint string in `view/mod.rs` (~line with `[?] help · [q]uit`) — add `[s]ettings` only if it fits the narrow footer, otherwise the help row is the canonical discovery surface)
- Create: `crates/qoo-tui/src/view/settings.rs` (+ register in `view/mod.rs`)
- Test: unit tests in each touched module + an insta snapshot for the overlay

**Interfaces:**
- Consumes: daemon `settings` RPC (Task 4 shape).
- Produces: user-visible `s` overlay; `SettingsPayload` types.

- [x] **Step 1: Mirror types** (serde, all-optional-tolerant so an old daemon or partial payload never panics):

```rust
// ipc/types.rs
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct SettingsPayload {
    #[serde(default)]
    pub models: SettingsModels,
}
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct SettingsModels {
    #[serde(default)]
    pub defaults: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub global: SettingsLayer,
    #[serde(default)]
    pub projects: Vec<SettingsProjectLayer>,
}
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct SettingsLayer {
    #[serde(default)]
    pub entries: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub source: String,
}
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct SettingsProjectLayer {
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub entries: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub source: String,
}
```

(BTreeMap so iteration order — and the snapshot — is deterministic.) Add a round-trip deserialization test with a full payload and with `{}` (old daemon).
- [x] **Step 2: Plumbing.** event.rs: `Cmd::FetchSettings` + `Event::Settings { payload: Option<SettingsPayload> }`; executor arm mirrors `FetchDefinitions` (`rpc_once(&sock, &RpcCall { method: "settings".into(), params: serde_json::Value::Null }, 5_000)`, `.ok()` → `serde_json::from_value(...).ok()` → send). app.rs: field `pub settings: Option<Option<SettingsPayload>>` (outer None = never fetched, `Some(None)` = fetch failed/unsupported → render the "(settings unavailable — daemon predates the settings RPC)" line, `Some(Some(p))` = data); `Event::Settings` arm stores it (`dirty: true`).
- [x] **Step 3: Keybinding + mode.** keymap.rs: `KeyCode::Char('s') => AppAction::Settings,` + `Settings` variant + test (mirror `q_quits`). app.rs: handle `AppAction::Settings` exactly like the existing `AppAction::Help` handler (read it first — same open/close mechanism, whatever it is: mode variant or overlay flag), and on open push `Cmd::FetchSettings` when `self.settings.is_none()`. Any-key-closes: mirror help's close path.
- [x] **Step 4: Render.** `view/settings.rs` modeled on `view/help.rs::render` (centered block, dim backdrop if help has one). Content builder is a pure fn returning `Vec<(String, String)>` rows for testability:

```rust
/// Rows for the settings overlay: the effective global table first
/// (defaults ⊕ global, alias → id), then one section per overriding project
/// showing only its deltas. Pure, so the layout is unit-testable.
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
```

Style: alias in `fg`, id in `info`, section headers bold — reuse the semantic table (`view/theme.rs`). Unit-test `settings_rows` (defaults-only, global override, project delta) + one TestBackend snapshot of the open overlay.
- [x] **Step 5: Verify + commit.** `cargo test -p qoo-tui` green (inspect any snapshot diff — only the new overlay snapshot should be NEW, nothing else changed); `cargo build --release -p qoo-tui`. Commit: `feat(tui): settings overlay (s) showing the model alias table`.

---

### Task 6: end-to-end gate + docs

**Files:**
- Modify: `docs/superpowers/plans/2026-07-10-model-aliases.md` (check boxes), `~/workspace/queohoh/config.yaml` is USER-OWNED — do NOT edit it; instead note in the final report how to add overrides.

- [x] **Step 1:** `mise run check` → all six sub-gates green (TS test/typecheck/lint:ci + Rust test:rs/typecheck:rs/lint:rs).
- [x] **Step 2:** Manual smoke: `mise run daemon:restart`, then `node packages/daemon/dist/cli.js status`; confirm the TASKS pane shows resolved ids and `s` opens the overlay (or, headless: `echo '{"id":1,"method":"settings"}' | nc -U ~/.local/state/queohoh/daemon.sock` returns the payload — check the socket protocol framing in `ipc/client.rs` first; if it's not line-delimited JSON, skip the nc probe and rely on the TUI).
- [x] **Step 3:** Final commit of any stragglers; report: files changed, how to add a per-project override (3-line vars.yaml snippet), old-daemon behavior.
