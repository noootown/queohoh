# Explicit Discover Verb — Design

**Date:** 2026-07-14
**Status:** Approved

## Problem

Pressing `r` on the TASKS pane for a zero-arg cron definition (e.g. `slack-react-release-notes`, `workspace-sanitize`) fails with `definition <name> has no discovery`.

Root cause: the TUI dispatches `runDefinition` with `args: []` for zero-arg defs, and the daemon (`packages/daemon/src/api.ts:524-528`) infers the trigger mode purely from arg count — `args.length > 0` → args mode, otherwise → discover mode. A zero-arg def is assumed to be discovery-backed; `instantiateDefinition` (`packages/core/src/instantiate.ts:70-71`) then throws when no `discovery` block exists. The cron engine (`packages/daemon/src/engine.ts:396`) already picks correctly by `def.discovery ? discover : args`, so the same def works when fired by the clock but fails when run manually.

Rather than patching the inference, discovery becomes an explicit verb: `r` always means "plain run", a new `d` means "discover and fan out". "No args" is no longer ambiguous anywhere.

Dedup is explicitly out of scope: both current cron defs declare `dedup: none`, and the daily cron interval is itself the dedup for `slack-react-release-notes`. No dedup behavior changes in this design.

## Design

### 1. Daemon RPC

**`runDefinition` → always args mode.** Delete the `args.length > 0 ? args : discover` ternary. The trigger is always `{mode: "args", values: args.map(String)}` — for zero args, `buildItemFromArgs` fills declared defaults; a required arg without a default errors with `missing required arg: <name>` (accurate, unlike the current discovery error). All other `runDefinition` behavior (worktree/ref/cwd precedence, `resume_session_id`, source) is unchanged.

**New `discoverDefinition` method.** Params: `{repo, name, source}` (source `"mcp" | "tui"`, same coercion as `runDefinition`; only the TUI calls it for now). Resolves the def, calls `instantiateDefinition` with `{mode: "discover"}`, same globalVars/repoVars construction as `runDefinition`, returns created tasks. No worktree/ref/cwd overrides in v1 — discovery items resolve their refs per-item via the def's own `worktree:` setting. The `has no discovery` error remains reachable here and is now accurate: the caller explicitly asked to discover.

**Cron engine unchanged.** `engine.ts:396` keeps `def.discovery ? {mode:"discover"} : {mode:"args", values: []}` — with explicit verbs elsewhere, this branch is no longer an inference hack; it *defines* what `cron:` means per def shape (discovery def → scheduled fan-out, plain def → scheduled run).

### 2. MCP

`run_task_definition` keeps its schema; the "Without args, runs the definition's discovery command" behavior and description text are removed — it is now always a plain run. No MCP discover tool is added (the only discovery consumer, auto pr-review, is being retired; add one later if a need appears).

### 3. TUI — `d` verb

- Add `PaneButton::Discover` to `pane_buttons(PaneId::Tasks)`: chip row becomes `[r]un [d]iscover [z]collapse`. The chip/keymap single-source-of-truth in `hit.rs` gates the key automatically.
- Keymap: `Char('d')` → `gated(PaneButton::Discover, AppAction::DiscoverSelectedDef)`, TASKS-focus only (other panes have no Discover chip, so the gate makes it inert there).
- Single-row only — `Discover` is not added to `bulk_allowed`; a bulk selection dims the chip and the key refuses with the standard status line.
- Action handler: resolve the highlighted def from the filtered TASKS selection (same resolution as `run_selected_task_def`). If `hasDiscovery` → dispatch a fire-and-forget `discoverDefinition` RPC (label `"discover"`, 5s timeout, `timeout_is_ok: true` — discovery can outlive the client timeout, the push subscription re-syncs; invalidate the repo's def summaries on success, mirroring `run_definition_cmd`). If not → status line `"<name> has no discovery"`, no RPC.
- `r` flow is untouched TUI-side (zero-arg def → immediate run, def with args → run form). It stops erroring because of the daemon change alone.

### 4. TUI — display

- **Row glyph:** discovery-backed defs get a `⌕` marker in their TASKS row, driven by the existing `hasDiscovery` flag on the def summaries.
- **Discovery sub-tab:** `DEF_TABS` grows from `["prompt", "config"]` to `["prompt", "config", "discovery"]` (static list, matching the existing sub-tab architecture — no per-def dynamic tab counts). Content: the full multi-line `discovery.command`, plus the `itemKey` template when declared. For a def without discovery the tab body is a single dim `no discovery` line. The existing one-line `discovery` row in the config sub-tab stays.

### 5. Tests

- **Daemon (`api.test.ts`):**
  - `runDefinition` with zero args on a no-discovery def creates one task (regression test for the bug).
  - `runDefinition` with zero args on a *discovery* def plain-runs (creates from defaults, does NOT run discovery).
  - `discoverDefinition` on a discovery def runs the command and fans out N tasks.
  - `discoverDefinition` on a no-discovery def rejects with `has no discovery`.
- **Core:** `instantiate.ts` is unchanged; existing tests stand.
- **TUI:** keymap test (`d` fires on TASKS, inert elsewhere); action tests (dispatch on discovery def, status line on non-discovery def, bulk refusal); snapshot updates for the chip row, row glyph, and the new sub-tab.

## Known edges (deliberate)

- `r` on a **zero-arg discovery def** now plain-runs with an empty item. If its prompt template leans on discovery item vars, the rendered prompt is degraded — that is a def-authoring issue, not something the daemon guesses about.
- MCP callers that relied on "no args → discovery" (none known) now get a plain run, or `missing required arg` if the def has non-defaulted args.

## Out of scope

- Any dedup changes (`skip_seen` semantics, the `source === "cron"` bypass in `instantiate.ts:91-92`).
- MCP discover tool.
- Removing discovery from the `pr-review` def (user config repo, separate change).
