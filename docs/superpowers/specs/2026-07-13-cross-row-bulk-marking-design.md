# Cross-Row Bulk Marking Design

Date: 2026-07-13
Status: Approved

## Goal

Let the TUI build a non-contiguous bulk selection by cherry-picking individual rows, not just a contiguous `Shift+Arrow` span. `Space` toggles the cursor row into a per-pane "marked" set; the effective bulk selection for any downstream action becomes the union of the active contiguous range and the marked set. Available uniformly on all three list panes (Queue/Tasks/Worktrees), consumed by whichever bulk verbs each pane already supports.

## Background

Today `TabUiState.selections: [Selection; 3]` holds one `{cursor, anchor}` pair per pane; `Shift+Arrow` extends `anchor..cursor` into a contiguous range, and `view::selection_range` turns that into an inclusive `(start, end)` over the pane's currently-*visible* (search-filtered) row indices. Every consumer — `App::queue_selection_rows` (feeds `requeue_selected`/`cancel_selected`), `App::open_bulk_menu`'s Worktrees arm (feeds the bulk-remove confirm dialog), `is_bulk_selection` (dims not-applicable title chips) — reads only that one span. There is no way to select row 2 and row 7 without also sweeping everything between them.

Because selection is index-based into the *filtered* view, it is fragile across filter/snapshot changes: `update.rs`'s `Mode::Search` key handling resets `Selection { cursor: 0, anchor: None }` on every search-box keystroke (`update.rs:266-284`), since a filtered-index selection is meaningless once the filter changes. `clamp_span` further defends the range against a daemon snapshot shrinking the visible set mid-selection. Any new non-contiguous mechanism needs to either accept the same fragility or sidestep it — this design sidesteps it by keying marks on stable row identity instead of index.

## Section 1 — Data model & keybinding

`TabUiState` gains a new field:

```rust
pub marks: [HashSet<String>; 3], // indexed by ListPane, mirrors `selections`
```

Each pane's mark key is a stable identity string it already produces elsewhere in the codebase:

- **Queue** → `task_id` (already the join key in `queue_selection_rows`)
- **Tasks** → def `name` (already the map key in `defs_by_project`)
- **Worktrees** → `raw_name` (already the frozen identity in `open_bulk_menu`'s `remove_names`)

Because marks are identity-keyed rather than index-keyed, they survive search-box edits, daemon snapshot pushes/reorders, and repo-tab switches for free — no `clamp_span`-style defensive clamping is needed for marks specifically (a mark for a row that no longer exists in the current row list simply never matches when the union is resolved; see Section 2). `TabUiState` is per-repo-tab already, so marks are naturally scoped per-repo-per-pane, same as `selections` and `search` are today.

`Space` toggles the cursor row's identity in `marks[pane]`:

- Unbound today (verified against `keymap.rs`), added as a new arm in `list_mode_action` alongside the existing single-char keys, active in all three panes uniformly (no per-pane gating — marking is a selection primitive, independent of which bulk verbs a pane currently supports, mirroring how `Shift+Arrow` already works everywhere even though only Queue and Worktrees currently have any bulk-doable verb).
- Toggle-in-place: the cursor does not move, and the anchor/range is untouched. This matches the "jump around, then mark" workflow (ranger/nnn/lf convention) rather than fzf's toggle-and-advance, since the point of this feature is deliberately non-sequential picking.
- No eligibility check at toggle time — Space always toggles regardless of whether the row is currently a valid bulk-remove/requeue/cancel target, exactly mirroring how the existing range can span busy/session rows today; eligibility filtering happens later, once, at resolution time (Section 2).

## Section 2 — Union resolution

Every site that currently resolves "the selected rows" from `selection_range` alone is widened to a single filter pass over the pane's *visible* rows: a row at visible position `pos` is included if `pos ∈ [start, end]` **or** its identity is in `marks[pane]`. This preserves visible-row order in the resolved output (unchanged from today) and only touches two existing chokepoints:

- `App::queue_selection_rows` (`app/actions.rs`) — feeds `requeue_selected`/`cancel_selected`. The `is_range: bool` return value becomes `is_bulk: bool` (`range non-trivial OR marks non-empty`), since a single marked row with no range is still a bulk (not single-target) action.
- `App::open_bulk_menu`'s `ListPane::Worktrees` arm (`app/menus.rs`) — feeds `bulk_remove_confirm_mode`. The `remove_names` computation folds in `marks[Worktrees]` the same way.

The Tasks pane gains marking (Space works, marked rows highlight) but stays inert for bulk *actions* — `hit::bulk_allowed(Tasks, Run)` is still `false`, so `run_or_bulk_selected_task_def` continues to refuse a bulk selection there with `"not applicable to bulk selection"`, exactly as it refuses a multi-row range today. Marking is a pane-wide selection primitive; which panes can *act* on a bulk selection remains entirely governed by the existing `hit::bulk_allowed` matrix — unchanged by this design.

A mark whose identity no longer resolves to a current row (removed by another session, filtered out of the daemon snapshot) is silently excluded when the union is resolved — it just never matches a `pos`. No pruning pass is needed; stale marks are inert by construction, not cleaned up.

`open_bulk_menu`'s `ListPane::Tasks` arm is intentionally left untouched (not folded into the union) — it is already unreachable from live UI (`hit::bulk_allowed(Tasks, Run)` is `false`, so no caller ever gets past the `bulk_blocked` guard to reach it) and is kept only for shape/API parity, per the prior bulk-menu-picker-removal work. If a future change ever makes a Tasks bulk verb reachable, wiring marks into that arm is a one-line addition mirroring the Worktrees arm.

`is_bulk_selection` (`view/mod.rs`) — used to dim not-applicable title-bar chips — becomes:

```rust
pub(crate) fn is_bulk_selection(sel: &Selection, marks: &HashSet<String>) -> bool {
    let (start, end) = selection_range(sel);
    end > start || !marks.is_empty()
}
```

All four call sites in `view/panes.rs` (the three per-pane dim checks plus the shared row-highlight computation) pass the pane's `marks` set alongside its `Selection`.

## Section 3 — Rendering

Marked rows reuse the exact highlight style the contiguous range already uses (accent background tint via `patch_line`) — no new column, no new glyph:

```rust
let selected = focused && (idx_in_range || marks.contains(identity_of(&rows[idx])));
```

`identity_of` is the same per-pane stable-key extraction used in Section 2 (`task_id` / def `name` / `raw_name`). A row that is both in-range and marked renders identically to a row that is only one or the other — the two mechanisms are visually indistinguishable by design, matching the union mental model: both just mean "part of the bulk selection." The eventual confirm dialog's explicit name list is where a user can double-check exactly what's included before firing.

## Section 4 — Esc staging & cleanup after dispatch

`App::clear_esc` today has two stages: (1) if an anchor is active, clear it; (2) else if search is non-empty, clear it. This design merges range-clearing and marks-clearing into stage 1 — **one Esc clears the whole bulk selection (range ∪ marks) as a unit**:

```rust
fn clear_esc(&mut self) -> bool {
    if !matches!(self.mode, Mode::List) { ... } // unchanged
    let Some(pane) = self.focused_list() else { return false };
    let sel = self.ui().selections[pane as usize];
    let has_range = sel.anchor.is_some();
    let has_marks = !self.ui().marks[pane as usize].is_empty();
    if has_range || has_marks {
        self.ui().selections[pane as usize] = Selection { cursor: sel.cursor, anchor: None };
        self.ui().marks[pane as usize].clear();
        return true;
    }
    // ...existing search-clear stage, unchanged
}
```

Rationale: from the user's side, range and marks together are *one* selection, not two independent things to peel back separately. The existing `esc_with_active_range_clears_range_before_it_can_open_bulk` test's contract (clear-before-open) extends naturally: after Esc, a subsequent `a`/`x` opens the single-target path, not a bulk one.

`App::clear_range` (called before every bulk RPC dispatch — `BulkRemoveWorktrees`, requeue-range, cancel-range) is renamed `clear_range_and_marks` and clears both fields, so a completed bulk action never leaves stale marks behind for the next selection.

## Section 5 — Testing

New/updated coverage, primarily in `bulk_flow_tests.rs` and a new `mark_flow_tests.rs`:

- `Space` toggles a mark on; `Space` again toggles it off (idempotent).
- A mark survives a `Snapshot` push that reorders rows (identity-based — no index to invalidate).
- A mark survives editing the pane's search filter (the one thing that currently nukes the contiguous range).
- Union: mark two non-adjacent rows, `Shift+Arrow` a separate contiguous pair, fire the bulk action — the resolved RPC set is exactly the 4 rows, in visible-row order, no duplicates when a marked row also falls inside the range.
- A single marked row with no active range is still treated as a bulk selection (routes through the same confirm/dispatch path as a 2+ row range, not the single-target path).
- Esc clears range and marks together in one press; a second Esc (with search active) clears search.
- A stale mark (its row's identity removed from the latest snapshot) is silently excluded from the resolved set — no panic, no phantom RPC call.
- `is_bulk_selection`-driven chip dimming reflects marks-only selections (no active range) the same as it does range-only selections today.

## Explicitly out of scope for this pass

- "Select all visible" / "invert selection" keys — no evidence yet that cherry-picking needs a bulk-toggle shortcut; easy to add later as another keymap arm over the same `marks` set.
- A dedicated mark-count indicator in the footer/status line — the "N selected" title the bulk confirm dialog already renders (post union) covers this; a live running counter while marking can follow if it turns out to be needed.
- Marks persisting across the daemon restarting / the TUI process restarting — `TabUiState` is already session-only (never serialized, per its existing doc comment), and marks inherit that.
