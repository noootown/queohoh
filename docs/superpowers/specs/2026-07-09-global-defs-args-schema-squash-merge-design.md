# Global Task Definitions, Rich Args & Squash-Merge — Design

**Date:** 2026-07-09
**Status:** Draft, awaiting review

## Goal

Move daily skill-driven operations into queohoh. Three enablers, one ported skill:

1. **Rich args schema** — definition args gain `default`, `options`, `description`;
   the TUI collects them in a per-arg form (defaults prefilled, options cycled).
2. **Global task definitions** — a shared `<workspace>/global/tasks/` library that
   appears under every project, so cross-project tasks are defined once.
3. **Primary-checkout execution** — a definition can run in the project's main
   checkout (`worktree: repo`), which squash-merge requires.
4. **`squash-merge`** ships as the first global definition, wired to a worktree
   action-menu entry; `pr-ready` is upgraded to use the new args schema.

Long-term intent: most `~/.claude/skills` become queohoh definitions. Spawned
Claude inherits the user's environment (skills work inside tasks), so porting a
skill is definition authoring, not code — these three enablers are the code.

## 1. Rich args schema

### Core (`packages/core/src/definition.ts`)

`args` accepts strings (back-compat) or objects, normalized to `ArgSpec[]`:

```yaml
args:
  - pr                      # shorthand: required, no default
  - name: mode
    default: ready
    options: [ready, create]
    description: hand off or keep WIP
```

```ts
interface ArgSpec {
  name: string;
  default?: string;      // absent → required
  options?: string[];    // enum; default must be a member (validated at load)
  description?: string;
}
```

- Load-time validation: `default` ∈ `options` when both present; unique names.
- `instantiateDefinition` (args mode): trigger values may be shorter than
  `args` — missing tail values fill from defaults; a missing value with no
  default throws; a value outside `options` throws. Values remain positional.
- Dedup-key fallback and `{{name}}` template substitution unchanged.

### Daemon

`definitions` RPC summary carries `args: ArgSpec[]` (was `string[]`).
`runDefinition` is unchanged except values-shorter-than-args is now legal.

### TUI (`def-args` → per-arg form)

New `ArgsForm` component replaces the single whitespace-split text box:

```
┌ pr-ready — platform ─────────────────┐
│ pr>     1841█        (PR number)     │
│ mode>   ‹ready›      ready | create  │
│ review> ‹auto›       auto|light|full │
│ tab next · ←/→ cycle · enter submit  │
└──────────────────────────────────────┘
```

- One row per arg. Text args: TextInput-style, prefilled with `default`.
  Enum args: value cycles through `options` with ←/→ (starts at `default`).
- Tab/↓ next field, shift-tab/↑ previous, enter submits (all rows), esc cancels.
- Required-and-empty blocks submit with an inline error on that row.
- `def-pick` rows render args with defaults visible: `pr-ready (pr, mode=ready,
  review=auto)`.
- The `def-args` Mode gains `initial?` (editable prefill) and `fixed?`
  (dimmed read-only rows, still submitted positionally) so callers can seed
  fields from context.

### Worktree-context auto-fill

When a def is run with a worktree in scope, args named by convention auto-fill
from that worktree's branch via `contextArgValues(branch)` (core): `source` and
`branch` are the branch itself, `ticket` is the ticket token extracted from it
(omitted when the branch carries none). ArgsForm ignores keys the def does not
declare, so callers pass the whole map.

Whether the fill is `fixed` or `initial` depends on how directed the run is:

- **Explicit worktree target → `fixed`** (read-only): the worktree action-menu
  entries (`Run task definition…`, `Squash merge into…`) — the user picked this
  worktree, so its branch decides `source`/`branch`/`ticket` and is not asked.
- **Ambient tasks-pane run → `initial`** (editable): the TASKS-pane `Run` action
  borrows the current worktrees-pane selection as a convenience default the user
  can edit or clear. Here `selectors.ambientRunArgs` also overlays `source`/
  `branch` (when the def declares no `options` of its own) with a **dropdown of
  the repo's worktree branches**, so the user picks the source rather than
  retyping it; the selected row seeds the initial value. `main`/`master` and
  session rows contribute no candidate and no prefill (a wrong prefill of the
  primary checkout only invites a wasted run). The overlaid `options` are TUI-
  side only — the def declares none, so the daemon never validates them and
  submission stays positional.

Identifier hygiene: worktree rows carry both a stripped display `name` and a raw
`<repo>.<branch>` identifier (`rawName`). Everything dispatched to the daemon —
the run-def worktree override, `task-fresh`/`task-main` enqueue, worktree removal,
and lane keys — uses `rawName`; only titles and row text use the stripped `name`.
The stripped form is a display convenience and does not resolve as a worktree ref
(`resolveTarget` matches raw names).

## 2. Global task definitions

- New conventional dir: `<workspace>/global/tasks/<name>/` — same format as
  project definitions. No new config key.
- `definitions` RPC: for each project, project-local defs ∪ global defs;
  a project-local name shadows the global one. Summaries gain
  `scope: "project" | "global"`; def-pick shows global entries with a `(g)`
  marker.
- `definition` / `runDefinition` lookup order: project tasks dir, then global.
  `repo` param stays the **target project**; `cwd` and `repoVars` are the
  target project's (workspace dir + its `vars.yaml`) so discovery commands and
  templates resolve against the project being operated on.
- **Builtin vars** (needed for portable global defs): `render()` receives
  `project` (name) and `repo_path` (the project's code-repo path from
  config.yaml), injected below global vars in precedence (any explicit var
  wins).

## 3. Primary-checkout execution (`worktree: repo`)

- New ref kind `repo` (alongside `worktree:`/`pr:`/`ticket:`/`temp`): resolves
  to the project's **primary checkout** (`config.projects[].path`), never
  spawns, never ephemeral.
- Resolver returns the sentinel worktree name `@repo`; the engine's
  name→path lookup special-cases `@repo` → project path. Snapshot/queue rows
  render it as the repo name.
- Scheduler lane: `@repo` is a lane like any worktree name, so two
  primary-checkout tasks on one project serialize — desired (they share a
  checkout).
- Hazard (accepted, documented): a task in the primary checkout shares the tree
  with the user's own work. The worker's existing dirty-tree post-guard stays;
  definitions targeting `repo` must precondition-check cleanliness (squash-merge
  does).

## 4. `squash-merge` global definition + menu entry

### Definition (`<workspace>/global/tasks/squash-merge/`)

```yaml
args:
  - name: source
    description: branch to squash
  - name: target
    default: main
dedup: none
worktree: repo
model: opus
timeout: 15m
```

Prompt (mirrors the `/squash-merge` skill, adapted for worktrees):

1. Preconditions — abort with a clear message if: `source == target`; the
   primary checkout tree is dirty; `source` doesn't exist.
2. `git checkout <target>` (primary checkout; fails loudly if target is
   checked out in another worktree), list `git log --oneline target..source`.
3. `git merge --squash <source>`; generate a conventional-commit message from
   the staged diff (title + short body); commit.
4. Cleanup (auto-remove decision): `wt remove <source> --yes`, then
   `git branch -D <source>` if the branch survives. The worktree pane updates
   on the next engine refresh.
5. Summarize: commits squashed, target commit, worktree/branch removed.

The definition ships in-repo under `library/tasks/squash-merge/` (source of
truth, versioned) and is copied to `<workspace>/global/tasks/` for use; docs
note the copy step. (No auto-sync in this iteration.)

### Action-menu entry

- Worktree context gains `Squash merge into…` (ActionId `squash-merge`),
  **disabled** with reason when a task is running in that worktree (it will
  remove the worktree) or when the worktree has no branch.
- Selecting it opens the `def-args` form for the global `squash-merge`
  definition with `fixed: contextArgValues(<worktree branch>)` — `source` is the
  worktree's branch, shown read-only (this worktree is the explicit target);
  `target` stays editable, default `main`. Submit calls `runDefinition`
  **without** a worktree override
  (the def's `worktree: repo` governs), so the task runs in the primary
  checkout, not the selected worktree. As a backstop, `runDefinition` in the
  daemon **ignores** any `worktree` param when the resolved def declares
  `worktree: repo`: such a def is location-critical (it checks out the target
  branch in the primary checkout), and the picker's worktree only ever served as
  arg context — pinning the run to it would land it in the wrong cwd, where it
  could never succeed. So `Run task definition…` on a `repo`-pinned def from a
  worktree menu still runs in the primary checkout, with `source` pre-seeded from
  the worktree.
- If the global definition is missing from the workspace, the menu action
  surfaces "squash-merge definition not found — copy library/tasks/squash-merge
  to <workspace>/global/tasks/" on the status line.

## 5. `pr-ready` upgrade (definition-only, user workspace)

`~/workspace/queohoh/platform/tasks/pr-ready/config.yaml` becomes:

```yaml
args:
  - name: pr
    description: PR number
  - name: mode
    default: ready
    options: [ready, create]
  - name: review
    default: auto
    options: [auto, light, full]
```

`prompt.md` maps `mode`/`review` onto the `/pr-ready` skill's tokens (`create`
when mode=create; `light`/`full` when review≠auto). Stays project-local
(platform vars).

## Error handling

- Load-time arg-spec violations fail `loadDefinition` → surfaced per existing
  RPC error paths (def-pick shows nothing for a broken def; `definition` RPC
  returns the error).
- Instantiate-time validation (missing required, value ∉ options) throws →
  `runDefinition` error string → TUI status line.
- `repo` ref on an unknown project path: resolver returns needs-input.

## Testing

- core: ArgSpec normalization/validation (shorthand, default∈options, dup
  names); instantiate default-fill + option rejection; `repo` ref resolution +
  `@repo` sentinel; builtin vars precedence; global-vs-local definition
  shadowing in a temp workspace.
- daemon: `definitions` merge + scope field; `runDefinition` with short values;
  `@repo` path lookup.
- tui: ArgsForm (prefill, cycle, required-block, tab order, submit values);
  def-pick default display; squash-merge menu entry (prefill, disabled-when-
  busy, missing-def status line).

## Out of scope

- Auto-sync of `library/tasks/` into the workspace.
- Named (non-positional) args on the RPC/MCP surface.
- Porting further skills (follow-up definitions once this lands).
- Multi-repo/global-project selection UI beyond the existing project tabs.
