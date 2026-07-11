# Per-Project Model Aliases + TUI Settings Page — Design

Date: 2026-07-10
Status: approved (design review with Ian, this session)

## Problem

Task definitions and ad-hoc runs name models by short alias ("sonnet", "opus").
Today the worker passes that string to the claude CLI verbatim
(`worker.ts`: `def?.model ?? task.model ?? deps.defaults.model`), so the CLI's
own defaults decide what "sonnet" means — no pinning, no per-project cost
control. The user wants agent247-style resolution: a global alias table with
per-project opt-in overrides (e.g. one project pins sonnet to 4.6 to cut
cost), and a read-only TUI settings page showing the effective table.

## Non-goals

- Editing the table from the TUI (v1 is read-only; yaml stays the source of
  truth).
- Per-definition or per-task override syntax beyond what exists (a definition
  may already write a concrete id; that keeps working via passthrough).
- Validating ids against a live model catalog.

## Config schema

Three layers, later wins, merged per alias key:

1. **Built-in defaults** (in code, so zero-config works):

   | alias  | concrete id       |
   |--------|-------------------|
   | fable  | `claude-fable-5`  |
   | sonnet | `claude-sonnet-5` |
   | opus   | `claude-opus-4-8` |
   | haiku  | `claude-haiku-4-5`|

   Note: `claude-sonnet-5` and `claude-opus-4-8` carry 1M context natively —
   no `[1m]` suffix or beta opt-in exists for these models, so the "1M"
   requirement is satisfied by the plain ids.

2. **Global** — new optional `models:` map in `config.yaml`
   (`~/workspace/queohoh/config.yaml`). Schema: `Record<string, string>`,
   non-string values skipped with a load warning (agent247's tolerant-parse
   pattern).

3. **Per-project** — new optional `models:` block in
   `<workspace>/<project>/vars.yaml`, beside the existing template vars. Only
   the aliases a project overrides appear here.

Merging is per-key: a project that overrides `sonnet` still inherits the
global/default `opus`.

## Resolution semantics

One pure function, mirroring agent247's `resolveModel`:

```ts
resolveModel(name: string, table: Record<string, string>): string
// table[name] ?? name — unknown names (incl. full ids) pass through untouched
```

- **Where applied:** the single existing choke point in `packages/core/worker.ts`
  (`const model = def?.model ?? task.model ?? deps.defaults.model`) becomes
  `resolveModel(that, effectiveTableFor(repo))`. Definitions keep writing
  aliases; enqueued tasks that pass full ids (e.g. `/qoo` passing
  `claude-fable-5`) pass through unchanged.
- **Effective table per repo:** `defaults ⊕ global.models ⊕ project.models`,
  computed by a pure `effectiveModelTable(global, project)` helper next to the
  config loaders.
- No recursion: resolution is a single lookup (alias → id), never chained.

## Daemon surface

- **`definitions` RPC:** the `model` field on each summary (added earlier
  today) now carries the **resolved** id for that summary's repo, so the TASKS
  pane column truthfully shows the model that will run. The TUI's `claude-`
  prefix stripping keeps the column narrow.
- **New `settings` RPC** returning the model table view:

  ```jsonc
  {
    "models": {
      "defaults": { "fable": "claude-fable-5", ... },
      "global":   { "entries": {...}, "source": "~/workspace/queohoh/config.yaml" },
      "projects": [
        { "repo": "legacy-project",
          "entries": { "sonnet": "claude-sonnet-4-6" },
          "source": "~/workspace/queohoh/legacy-project/vars.yaml" }
      ]
    }
  }
  ```

  Only projects with a non-empty `models:` block appear in `projects`.

## TUI settings page

- Read-only overlay, same interaction pattern as the `?` help overlay
  (any key closes), bound to **`s`** in `Mode::List` (key is currently
  unbound; `ctrl+s` prefix behavior is untouched).
- Content: "models" section listing the effective global table
  (`alias → id`, defaults merged with global, source path shown once in the
  header), then one sub-section per overriding project showing only its
  deltas (`alias → id`).
- Data comes from the `settings` RPC, fetched lazily on first open with the
  same in-flight/dedup pattern as `reconcile_full_def`; a daemon that lacks
  the RPC (old daemon) shows "(settings unavailable — daemon predates the
  settings RPC)".
- Footer/help overlay gain the `s` binding line.

## Testing

- **core:** unit tests for `resolveModel` (hit, passthrough, empty table) and
  `effectiveModelTable` (three-layer merge, per-key override, tolerant parse
  of malformed blocks).
- **daemon:** `definitions` summaries carry resolved ids under a project
  override fixture; `settings` RPC shape test (defaults-only, global,
  global+project).
- **qoo-tui:** keymap test for `s`; overlay snapshot test (global table + one
  project delta); old-daemon fallback rendering test.

## Files expected to change

- `packages/core/src/config.ts` (global `models:`), a vars-loader touchpoint
  for the per-project block, `packages/core/src/models.ts` (new: resolve +
  merge helpers), `packages/core/src/worker.ts` (apply at choke point).
- `packages/daemon/src/api.ts` (resolved summary model, new `settings`
  method).
- `crates/qoo-tui`: `keymap.rs`, `app.rs` (mode/state + fetch), `event.rs`
  (Cmd/Event for settings fetch), new `view/settings.rs`, `view/help.rs`
  (binding line), `ipc/types.rs` (settings payload mirror).
