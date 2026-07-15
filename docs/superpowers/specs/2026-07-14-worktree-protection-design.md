# Worktree Protection — Design

**Date:** 2026-07-14
**Status:** Approved (pending spec review)

## Problem

Any git worktree in a project can be removed today — the `x` key in the TUI
WORKTREES pane (or a bulk selection) dispatches a `removeWorktree` RPC, and
`engine.removeWorktree` only refuses when the worktree is busy with a running
task. Nothing protects the project's main checkout or other long-lived
worktrees (e.g. `platform`'s `legal-lake` and `testing1`) from accidental
deletion. `removeWorktree` also force-cleans the worktree
(`git reset --hard` + `git clean -fd`) before removal, so an accidental delete
discards uncommitted work.

We want a per-project list of **protected worktrees that cannot be deleted**,
plus the main checkout protected by default for every project.

## Goals

- Per-project config declaring extra protected worktrees, in each project's
  `vars.yaml`.
- The main checkout is protected by default for every project, with no config.
- Deletion of a protected worktree is impossible (authoritative engine guard).
- The TUI shows which worktrees are protected and gates the remove action:
  - remove is enabled only if the selection contains at least one **removable**
    worktree;
  - protected worktrees are always silently skipped from any removal set;
  - a selection of only-protected worktrees makes remove a no-op with a status
    message.

## Non-Goals

- Global (cross-project) protected list in `config.yaml`. Scope is per-project
  `vars.yaml` only (YAGNI).
- Protecting against direct `git worktree remove` outside queohoh, or against
  the force-clean itself. This guards the queohoh deletion path only.
- Any MCP tool for worktree deletion (none exists; deletion is TUI-only).

## Architecture

The feature has two layers over a single shared predicate:

1. **Predicate (shared):** `isProtectedWorktree(repoPath, protectedNames, wt)`.
2. **Enforcement (Part A, TypeScript daemon):** engine hard block — the
   authoritative guard, independent of any client.
3. **Affordance (Part B, TS daemon + Rust TUI):** a `protected` flag flows
   through the snapshot so the TUI can render a lock glyph and gate the remove
   action before any RPC.

Both the enforcement guard and the snapshot enrichment call the same predicate,
so they can never disagree.

### The protection predicate

A worktree is protected when **either**:

- **`wt.path === repoPath`** — it is the project's main checkout. We use
  path-equality, not `wt.name === projectName`: a project is `{ name, path }`
  where `name` is a user-chosen label and the checkout's worktree name is
  `basename(path)`. These usually match but are not guaranteed to; path-equality
  is always correct and covers the real main checkout regardless. (`repoPath` is
  `config.projects.find(p => p.name === repo).path`, already resolved inside
  `engine.removeWorktree` and available per-repo in `worktreesByRepo`.)
- **`protectedNames.includes(wt.name)`** — it is in the project's configured
  `protected_worktrees` list. Matching is by worktree name (the TUI display
  name / branch basename), which is how the config author refers to them.

Session "You" rows in the TUI are synthetic and never real worktrees; they
default to unprotected and are already non-removable.

## Config

### Schema

New reserved key in per-project `vars.yaml`, a list of worktree names:

```yaml
# <workspace>/platform/vars.yaml
protected_worktrees:
  - legal-lake
  - testing1
```

Absent key → empty list → only the main checkout is protected.

### Loader

New loader in `packages/core/src/config.ts`, mirroring the existing reserved
scalar loaders (`loadProjectGithubId`, `loadProjectDefaultModel`,
`loadProjectModels`):

```ts
export function loadProjectProtectedWorktrees(projectDir: string): string[]
```

- Reads `<projectDir>/vars.yaml`; returns `[]` if the file or key is absent.
- **Tolerant**, matching the sibling reserved-key loaders (`loadProjectModels`,
  `loadProjectGithubId`, `loadProjectDefaultModel`) rather than the strict
  `loadProjectVars`: a non-list value → `[]`; within a list, non-string or empty
  entries are skipped; it never throws. A malformed value therefore only
  disables the extra protections (the main checkout stays protected via
  path-equality) — it never wedges config loading or snapshot generation. This
  supersedes the spec's earlier "throws on malformed" and resolves the snapshot
  degrade-quietly concern at the loader.
- `<projectDir>` is `projectWorkspaceDir(config, projectName)` — the existing
  `join(config.workspace, projectName)`.

`protected_worktrees` is added to the reserved-key skip list in
`loadProjectVars` (`config.ts:129-131`, alongside `models` / `github_id` /
`default_model`). Without this, `loadProjectVars` would hit its "non-scalar
value" throw on the list, and the key would otherwise leak into template vars.

## Part A — Engine hard block (enforcement)

In `Engine.removeWorktree(repo, name)` (`packages/daemon/src/engine.ts:191-206`),
after the worktree `wt` is resolved and next to the existing busy-guard:

```ts
const protectedNames = loadProjectProtectedWorktrees(projectWorkspaceDir(this.config, repo));
if (isProtectedWorktree(repoPath, protectedNames, wt)) {
  throw new Error(`Worktree "${wt.name}" is protected and cannot be removed`);
}
```

This is authoritative: it holds even if a client (a future MCP tool, a direct
RPC, a stale TUI that hasn't seen the `protected` flag) tries to remove a
protected worktree. It covers both the single and bulk delete paths, since bulk
issues N single `removeWorktree` RPCs. The thrown error surfaces to the TUI the
same way the busy-guard error already does.

## Part B — Snapshot enrichment (daemon → TUI)

### TypeScript

- Add `protected?: boolean` to `interface WorktreeInfo`
  (`packages/core/src/resolver.ts:13-36`). This is the shared type serialized
  verbatim into the snapshot.
- Populate it in `Engine.worktreesByRepo()`
  (`packages/daemon/src/engine.ts:118-127`). `protected` is **not** a git fact,
  so it is set on the base `wt` here (not in the `GitEnrichment` overlay at
  `engine.ts:38-51`) — the `{ ...wt, ...e }` merge then carries it through. For
  each worktree: `protected: isProtectedWorktree(repoPath, protectedNames, wt)`.
- No envelope change — `StateSnapshot.worktrees`
  (`packages/daemon/src/api.ts:53`, assembled at `api.ts:124`) already carries
  `WorktreeInfo[]`.

**Performance note.** `worktreesByRepo()` feeds `Api.snapshot()`, broadcast on
each mutation (not a hot loop). Reading a small `vars.yaml` per repo per
snapshot is cheap. If profiling shows it is chatty, memoize the parsed
`protected_worktrees` per repo and invalidate on config reload (the reload path
already exists). Start without the cache.

### Rust TUI (`crates/qoo-tui/`)

- **Deserialize:** add `#[serde(default)] pub protected: bool` to
  `WorktreeInfo` (`src/ipc/types.rs:135-166`). The container already uses
  `#[serde(rename_all = "camelCase", default)]`; `default` keeps it
  back-compatible with an older daemon that omits the field.
- **Row state:** add `pub protected: bool` to `WorktreeRow`
  (`src/selectors.rs:48-87`), copied from `wt.protected` in `worktree_rows`
  (`src/selectors.rs:547-553`). Session rows keep the `false` default.
- **Glyph:** 🔒 (`GLYPH_PROTECTED` in `src/view/theme.rs`, alongside
  `GLYPH_DIRTY = '±'` at `theme.rs:49`; mirrors the existing double-width
  `GLYPH_SEARCH = '🔍'` precedent). Rendered by **reusing the existing
  2-cell front-marker region** the `±` dirty marker already occupies
  (`worktree_line`, `src/view/panes.rs:540-547`), not a new column. That region
  is always `[glyph cell][space cell]` = 2 display cells, reserved whenever the
  pane has rows (`dirty_w0 = rows.is_empty() ? 0 : 1`, `selectors.rs:1436`). 🔒
  is exactly 2 display columns, so it fills the whole region with no trailing
  space. This needs **zero changes** to `WtColLayout`, the width drop-ladder,
  `pr_col_x`, or `wt_header` — the region width is unchanged.
  - **Precedence:** in that slot, `protected` wins over `dirty`: a protected row
    shows 🔒 (even if also dirty); a dirty-and-unprotected row shows `± `; a
    plain row shows two spaces. Losing the `±` marker on a protected-and-dirty
    row (e.g. a dirty main checkout) is an accepted minor trade-off for the
    lowest-risk layout integration.
  - **Width caveat:** 🔒 relies on the terminal rendering U+1F512 as 2 columns
    (what `unicode-width` reports). Terminals that render it as 1 column will
    shift that row's name by one cell — an accepted risk (the user chose the
    padlock over a single-width glyph). Under width pressure the marker slot can
    drop (`Drop::Dirty`), taking the lock with it — acceptable.
- **Gating** (remove enabled iff ≥1 removable selected; protected always
  skipped):
  - **Single select** (`src/app/actions.rs:878-885`, `remove_selected_worktree`):
    add a `row.protected` refusal alongside the existing `is_session` and
    `WtState::Busy` checks — set the status line, do not open the confirm
    dialog. A lone protected worktree → `x` is a no-op with a message.
  - **Bulk** (`src/app/menus.rs:59-67`, `open_bulk_menu`): extend the
    eligibility filter with `&& !r.protected` so protected rows are dropped from
    `remove_names`. The existing "no eligible rows" status when the filtered set
    is empty (`menus.rs:68-71`) delivers the all-protected-selection no-op for
    free.

## Error Handling

- Malformed `protected_worktrees` (not a list, or entries that are non-string /
  empty): handled entirely inside the tolerant loader — bad values yield `[]`,
  bad entries are skipped, no throw. So neither the snapshot path
  (`worktreesByRepo`) nor the guard path (`removeWorktree`) can be crashed by a
  malformed vars file; the main checkout stays protected regardless via
  path-equality. No extra guard/try-catch is needed at the call sites.
- Engine guard throw: message `Worktree "<name>" is protected and cannot be
  removed`, surfaced to the TUI status line as the busy-guard already is.

## Testing

### TypeScript

- `loadProjectProtectedWorktrees`: absent file → `[]`; absent key → `[]`;
  valid list → names; non-list value → throws; list with non-string/empty →
  throws.
- `isProtectedWorktree`: main checkout by path-equality (even when `name` ≠
  project name); configured name; unprotected normal worktree.
- `Engine.removeWorktree`: throws for main checkout; throws for a configured
  name; still removes a normal worktree; unchanged busy-guard behavior.
- `Engine.worktreesByRepo`: emits `protected: true` for main checkout and
  configured names, `false` otherwise.

### Rust

- `ipc/types.rs`: extend `modern_json` fixture + assertions; a snapshot
  omitting `protected` deserializes to `false` (back-compat).
- `selectors.rs`: `worktree_rows` copies `protected` onto the row.
- Gating: single protected worktree → refused with status, no confirm; bulk
  selection with mixed protected/removable → protected dropped from names;
  all-protected selection → "no eligible rows".
- Insta snapshots: refresh WORKTREES-pane renders for the new lock column
  (`view_default_80x24`, `view_wide_140x30`, `view_collapsed_queue_tasks`) and
  `confirm_bulk_remove` if protected rows are dropped from the listed names.

## Files Touched

**TypeScript**
- `packages/core/src/config.ts` — `loadProjectProtectedWorktrees`, reserved-skip
  key, `isProtectedWorktree` (or colocate the predicate near the engine).
- `packages/core/src/resolver.ts` — `protected?: boolean` on `WorktreeInfo`.
- `packages/daemon/src/engine.ts` — guard in `removeWorktree`, enrichment in
  `worktreesByRepo`.

**Rust**
- `crates/qoo-tui/src/ipc/types.rs` — `protected` field + tests.
- `crates/qoo-tui/src/selectors.rs` — `WorktreeRow.protected` + copy.
- `crates/qoo-tui/src/view/theme.rs` — `GLYPH_PROTECTED`.
- `crates/qoo-tui/src/view/panes.rs` — lock render in the existing marker slot
  (`worktree_line`); no header or layout change.
- `crates/qoo-tui/src/app/actions.rs` — single-select refusal.
- `crates/qoo-tui/src/app/menus.rs` — bulk eligibility filter.
- Snapshot fixtures / insta `.snap` files as listed under Testing.

**Config (user data, not code)**
- `<workspace>/platform/vars.yaml` — add `protected_worktrees: [legal-lake,
  testing1]`. Every project's main checkout is protected without any config.
