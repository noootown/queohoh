# Create Worktree & Adhoc Run — Design

**Date:** 2026-07-09
**Status:** Draft, awaiting review

## Goal

Two TUI entry points, both on hotkey `c`, dispatched by the focused pane:

1. **Worktrees pane → `c`**: open a dialog asking for a branch name, then create a
   worktree for the active project.
2. **Queue pane → `c`**: open a prompt dialog, then enqueue an adhoc run that spawns
   an ephemeral worktree named `qoo-<slug>` and executes the prompt there.

## Background

- `c` is unbound everywhere in `keymap.ts` / `App.tsx` — free in both panes.
- Adhoc runs already exist end-to-end: `enqueue` with no worktree sets `ref: "temp"`,
  the resolver spawns an ephemeral worktree (`tmp-<ulid6>` today) via `wt switch -c`,
  the engine runs the prompt there, and ephemeral worktrees are cleaned up after the
  run. Only the TUI entry point is missing (dropped during action-menu consolidation).
- Worktree creation plumbing exists in `resolver-io.ts` (`createWorktree` →
  `wt switch -c <branch>`); post-create setup runs via the target repo's `wt.toml`
  hooks. There is no user-initiated "create worktree" path — creation only happens
  implicitly during ref resolution.
- agent247's setup/cleanup shell scripts were never copied; cleanup semantics were
  ported to TS (`resolver-io.ts:88-105`, commit `3f4e1e4`) and setup is delegated to
  `wt` + per-repo `wt.toml`. No new dependency on agent247 is introduced here.

## Feature A — Create worktree (worktrees pane)

### UX

- With the worktrees pane focused, `c` opens a modal: `branch> █` (existing
  `Modal` + `TextInput` components, same pattern as `worktree-input`).
- Enter submits; Esc cancels. On success the modal closes and the new worktree
  appears in the pane on the next daemon snapshot. On failure the modal stays open
  (or reopens) showing the error line.
- Also added to the action menu (`a`) for worktree-context rows as
  `Create worktree…`, always enabled — it targets the active project, not the row,
  but the menu is the discoverable surface for pane capabilities.

### Implementation

| Layer | Change |
| --- | --- |
| `tui/keymap.ts` | New keymap action `create` on `c` (unprefixed, list-mode only — not while search or a modal is active). |
| `tui/App.tsx` | Dispatch `create` by last-focused pane: worktrees → new `Mode` variant `create-worktree`; queue → existing `add-task` mode (Feature B). Other panes: no-op. New modal render block + submit handler calling `actions.createWorktree(activeName, branch)`. |
| `tui/action-menu.ts` | New `ActionId` `"create-worktree"`, added to the worktree-context item list. `runMenuAction` maps it to the same modal. |
| `tui/actions.ts` | New client method `createWorktree(repo, branch)` → RPC `createWorktree {repo, name}`. |
| `daemon/api.ts` | New method `createWorktree` → `engine.createWorktree(repo, name)`. |
| `daemon/engine.ts` | New `createWorktree(repo, name)`: resolve repo path, reject if a worktree with that branch already exists, delegate to `resolverIO.createWorktree`. |
| `core/resolver-io.ts` | No change — `createWorktree` (`wt switch -c`) already exists. |

### Validation

Branch name must be non-empty and git-ref-safe: no whitespace, no `..`, no leading
`-` or `/`, no trailing `.lock`, printable ASCII. Validated in the TUI before the RPC
(inline error in the modal); the engine re-checks existence and surfaces `wt` errors
verbatim.

## Feature B — Create adhoc run (queue pane)

### UX

- With the queue pane focused, `c` opens the existing `add-task` prompt modal with
  **no worktree preselected** and the active project as target.
- Enter enqueues via the existing `enqueue` RPC with `ref: "temp"`; the run appears
  in the queue immediately and executes in a fresh ephemeral worktree.

### Ephemeral worktree naming: `qoo-<slug>`

Replace the current `tmp-<ulid6>` naming for temp refs with `qoo-<slug>`:

- Slug derived from the task prompt: lowercase, non-alphanumeric → `-`, collapsed,
  trimmed, truncated to 24 chars (whole words preferred).
- A 4-char ulid suffix is appended (`qoo-fix-login-redirect-01hx`) to guarantee
  uniqueness across runs with similar prompts.
- Empty/unusable prompt → fall back to `qoo-<ulid6>`.
- The resolver already has the `TaskInstance` in scope at spawn time, so the prompt
  is available where the name is generated (`resolver.ts` temp branch).
- `ephemeralWorktree` flag and post-run cleanup behavior are unchanged — only the
  name changes. Cleanup matching is by recorded worktree name on the task, not by
  prefix, so the rename is safe; any code that special-cases the `tmp-` prefix (to be
  verified during implementation) is updated to use the task's `ephemeralWorktree`
  flag instead of the name.

## Error handling

- `createWorktree` RPC failures (branch exists, `wt` non-zero exit) return the error
  string; the TUI shows it in the modal and keeps the input for correction.
- Enqueue failures reuse the existing `add-task` error path.
- Engine refuses nothing new for creation (no busy-guard needed — creation can't
  collide with a running task the way removal can).

## Testing

- `core`: unit tests for the slugify helper (truncation, collapsing, empty prompt
  fallback) and resolver temp-ref naming (`qoo-` prefix, uniqueness suffix).
- `tui`: keymap test — `c` produces `create` in list mode, is inert during search
  and modal modes; action-menu test updated for the new worktree-context item.
- `daemon`: api/engine test — `createWorktree` rejects existing branch, delegates to
  `resolverIO.createWorktree` with the resolved repo path.

## Out of scope

- Choosing a base ref for the new worktree (always `wt switch -c` default base).
- Session choice (`fresh`/`main`) in the queue `c` flow — uses the `add-task`
  modal's existing default.
- Any changes to agent247's shell scripts or porting `setup-worktree.sh`.
