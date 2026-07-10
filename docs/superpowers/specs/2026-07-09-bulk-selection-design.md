# Bulk Selection & Bulk Actions — TUI Side Panel

Date: 2026-07-09
Status: approved

## Goal

Shift+arrow multi-row selection in the three left-column list panes (queue,
tasks, worktrees), with bulk actions applied to the whole selection at once.
The driving flow: search a pane by some criterion (`/` filter), shift+arrow
across the filtered rows, and bulk-remove all matching worktrees.

Scope decisions (confirmed with user):

- All three list panes get the same selection mechanism.
- Contiguous range selection only — shift+arrow extends from an anchor. No
  space-toggle marks, no select-all key.
- Bulk actions skip inapplicable rows and surface counts (never block the
  batch, never error row-by-row).

## 1. Selection state

`TabUiState.selections` changes from `Record<pane, number>` to:

```ts
interface PaneSelection {
	cursor: number;
	anchor: number | null; // null = single selection (today's behavior)
}
```

- Selected range = `[min(anchor, cursor), max(anchor, cursor)]` over the
  **visible (filtered)** rows.
- Both indices clamp when the visible row count changes; if a pane empties,
  the anchor resets to null.
- Indices are position-based, not identity-based. This is acceptable because
  bulk targets are resolved to concrete ids/names at menu-open time (§4), so
  daemon pushes mid-flow cannot retarget a batch.

## 2. Keymap

- `KeyInput` gains `shift: boolean` (ink 6.8 parses `ESC[1;2A`-style
  modified arrows into `key.shift` + `key.upArrow`).
- New `KeymapAction`: `{ type: "extend-selection"; delta: 1 | -1 }`.
  - Emitted for shift+↑/↓ in a focused list pane.
  - `J` / `K` (shift+j/k) map to the same for vim parity.
- Semantics of `extend-selection`: if anchor is null, set anchor = cursor;
  then move cursor by delta, clamped.
- Plain ↑/↓/j/k (`move-selection`) collapses the range: cursor moves,
  anchor → null. Mouse wheel does the same (it dispatches move-selection).
- **Esc layering** (decided in `App.dispatch`, which owns the state): with a
  range active, Esc clears only the range; otherwise it clears the search
  filter (existing behavior).
- Editing the search query resets cursor to 0 **and** clears the anchor
  (extends the existing selection reset in search mode).
- Focus, pane, and tab changes leave anchors alone — selection is per-tab,
  per-pane state and preserving it is harmless.

## 3. Rendering

- Every row in the range renders `inverse` while its pane is focused — the
  same treatment as today's single selected row. No separate cursor marker;
  the range grows from its end, which is where the user is looking.
- Pane title shows a count while the range spans >1 row:
  `WORKTREES · 3 selected` (composes with the search suffix from
  `paneTitle`).
- Footer shows a contextual hint while a range is active:
  `a bulk actions · esc clear selection`.
- The detail pane continues to show only the cursor row's context.

## 4. Bulk action menu

Pressing `a` while the last-focused list pane has a range spanning >1 row
opens a **bulk** action menu instead of the single-item menu. Eligibility is
computed per action at menu-open time; the `MenuTarget` carries the resolved
ids so execution operates on a frozen set.

| Pane      | Bulk actions | Eligible rows                          | Skipped                        |
| --------- | ------------ | -------------------------------------- | ------------------------------ |
| queue     | Rerun, Skip  | rerun: failed/needs-input; skip: +done | archived rows, other statuses  |
| tasks     | Run          | definitions with no args               | definitions requiring args     |
| worktrees | Remove…      | kind "worktree" and not busy           | busy rows, session ("you") rows |

- Labels show counts: `Rerun (3 of 5)`. An action with 0 eligible rows
  renders disabled with a reason (existing disabled-row convention).
- Single-only actions (assign-worktree, tmux-open, task-fresh, task-main,
  run-def) do not appear in bulk menus.
- A new `buildBulkActions()` lives beside `buildActions()` in
  `action-menu.ts`; new `MenuTarget` variants carry the per-action eligible
  id lists (e.g. `{ kind: "bulk-worktrees"; remove: string[]; total: number }`).

## 5. Execution & confirm

- **Rerun / Skip / Run**: execute immediately (their single-item versions do
  not confirm either). Sequential `await` per id; then one summary status
  line: `reran 3` or `reran 2, 1 failed: <first error>`.
- **Remove worktrees**: new mode `confirm-bulk-remove` reusing the confirm
  modal. It lists up to 8 worktree names plus `…and N more`, with the
  existing "discards uncommitted changes / deletes the local branch"
  warning. `y` runs `removeWorktree` sequentially against the exact names
  captured at menu-open; `n`/Esc cancels.
- After any bulk action completes: anchor clears; cursor clamps naturally as
  rows disappear from the snapshot.

No daemon or protocol changes: bulk execution is a client-side loop over the
existing per-item actions.

## 6. Testing

- `keymap.test.ts`: shift+arrow and `J`/`K` emit `extend-selection`; plain
  arrows still emit `move-selection`; detail-pane focus unaffected.
- `action-menu.test.ts`: `buildBulkActions` labels, counts, and
  disabled-at-zero behavior per pane.
- `app.test.tsx`:
  - shift+↓ renders two inverse rows and the title count;
  - plain arrow collapses the range;
  - Esc clears the range before it clears search;
  - `a` with a range opens the bulk menu;
  - mixed worktree selection shows skip counts;
  - `y` in bulk confirm calls `removeWorktree` once per name;
  - editing the filter clears the range.
