# Archive tasks whose worktree has been deleted

## Problem

Terminal tasks linger in the live queue and distract from active work. The existing auto-archive sweep (`packages/daemon/src/engine.ts:281–292`) only removes `done` and `cancelled` tasks older than `archive_after_days` (default 7). `failed` / `skipped` / `verify-failed` are deliberately kept visible forever ("they usually want attention"), so nothing ever sweeps them.

Deleting a task's worktree is a deliberate act in queohoh — `removeWorktree` is only ever an explicit TUI RPC (`packages/daemon/src/api.ts:638`); nothing auto-removes a worktree on task completion. So "the worktree is gone" is a strong, user-authored "I'm done with this / I've abandoned it" signal. We should treat it as an archive trigger, which also catches the failed/skipped set the 7-day timer never touches.

## Goal

When a terminal task's spawned worktree no longer exists, archive the task so it drops out of the live list. The task stays recoverable in the archived list (`archivedRecent`, `store.listArchived()`) — no data-model change, no new "hidden" state.

## Non-goals

- No TUI change. Archived tasks already leave the live list; an "unarchive" affordance is a separate, additive follow-up.
- No change to the existing 7-day age sweep. It keeps working for done/cancelled whose worktree still exists.
- No auto-removal of worktrees. Worktree deletion stays user-driven.

## Design

Add a second archive trigger in the engine tick, alongside the age-based sweep, in the same loop over `store.list()`.

**Archive a task when all hold:**

1. **Status is terminal** — one of `done`, `failed`, `skipped`, `cancelled`, `verify-failed`. Never `queued` / `running` / `needs-input`: a missing worktree there is a bug to surface, not clutter to hide.
2. **`target.worktree` is a real spawned worktree** — not `null` and not `REPO_SENTINEL` (`"@repo"`, from `packages/core/src/resolver.ts:11`). Primary-checkout tasks have no worktree to delete.
3. **The worktree is absent from a confirmed listing** — no entry in `worktreeCache` for the task's repo matches `target.worktree`, **and** that repo has had at least one successful `listWorktrees` this process (see the cold-cache guard below).

Action: reuse `store.archive(t.id)`. The task file moves to `archive/`; it remains visible via `store.listArchived()` / the API's `archivedRecent` (`api.ts:113`). Fire `onChange` when at least one task was archived, matching the surrounding sweep.

This **deliberately overrides** the age-sweep's "keep failed visible" rationale: the explicit worktree-deletion signal outranks "failed tasks want attention." For `done` / `cancelled` it simply means they archive *sooner* (at deletion rather than at 7 days) — consistent, no regression.

### Cold-cache / transient-failure guard

`refreshWorktreeCache` (`engine.ts:434`) already resists transient git failures: on a `listWorktrees` error for a repo with a prior entry it keeps the last-known list rather than clobbering to `[]`. But for a repo with **no** prior entry it seeds `[]` as "known-empty" (`engine.ts:447–448`). That seeded `[]` is indistinguishable from a genuine "listing succeeded, zero worktrees" — and would wrongly make every worktree look deleted on a cold start or a repo that has never listed successfully.

Fix: track a per-repo "listing has succeeded at least once" set, populated in the **success (try) branch** of `refreshWorktreeCache`, not the catch/seed path. The archive trigger only considers repos in that set. This is the precise signal that the empty-or-not listing we're reading is real.

### Ordering

Run the worktree-deletion sweep **after** `refreshWorktreeCache()` within the tick so it reads a fresh cache. It can share the same `store.list()` iteration as the age sweep (evaluate both conditions per task) or run as a second loop immediately after — either is fine; keep it adjacent to the age sweep for legibility.

### Repo-key mapping

`worktreeCache` is keyed by `project.name`, and `task.target.repo` **is** that key — the engine already does `worktreeCache.delete(task.target.repo)` at `engine.ts:664`. So the lookup is `worktreeCache.get(task.target.repo)` directly, no mapping.

### Worktree matching

`target.worktree` holds the worktree **name** (the resolver records `match.name` / `spawned.name`; discovery records `wt.name`). Match against `WorktreeInfo.name` (`resolver.ts:13`) in the cached list — "deleted" = no cached entry with `name === task.target.worktree`.

## Testing

Engine-level tests (mirror `packages/daemon/src/__tests__/engine.test.ts`), driving the tick with a stub `worktreeCache` / `listWorktrees`:

- Terminal task + worktree absent from a confirmed listing → archived (assert `store.archive` called, task off the live list, present in `listArchived`).
- Terminal task + worktree still present → not archived.
- `running` / `queued` / `needs-input` task + worktree absent → **not** archived.
- `target.worktree` `null` or `REPO_SENTINEL` → not archived.
- **Cold cache**: repo has never listed successfully (seeded `[]`) → not archived (guards the false-positive).
- Transient listing failure after a prior success (last-known kept) → not archived while the worktree is still in the last-known list.
- Each terminal status (`done`, `failed`, `skipped`, `cancelled`, `verify-failed`) archives on deletion.
- Age sweep still archives `done`/`cancelled` past `archive_after_days` when the worktree still exists (no regression).

## Files touched

- `packages/daemon/src/engine.ts` — the new sweep + the per-repo "listed successfully" set in `refreshWorktreeCache`.
- `packages/daemon/src/__tests__/engine.test.ts` — coverage above.

No config, schema, API, or TUI changes.
