# Cross-Row Bulk Marking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the TUI build a non-contiguous bulk selection — `Space` toggles the cursor row into a per-pane marked set, and every bulk verb acts on the marked set combined with the existing `Shift+Arrow` contiguous range.

**Architecture:** `TabUiState` gains `marks: [HashSet<String>; 3]`, keyed on the stable row identity that `App::row_identity` already produces (Queue `task_id`, Tasks `{repo}/{name}`, Worktrees `raw_name`). One new pure resolver, `view::selected_positions`, replaces the scattered `selection_range`-based row resolution; every consumer (rendering, pane title, queue requeue/cancel, worktree bulk-remove) reads through it. Because marks are identity-keyed, they survive search-filter edits and daemon snapshot reorders — the two things that currently reset the index-based range.

**Tech Stack:** Rust, ratatui 0.29, crossterm. Crate: `crates/qoo-tui`.

## Global Constraints

- Design spec: `docs/superpowers/specs/2026-07-13-cross-row-bulk-marking-design.md`.
- Build/test/lint from the repo root (`/Users/noootown/Downloads/agent247/queohoh.improvement`):
  - Tests: `cargo test -p qoo-tui --lib`
  - Lint: `cargo clippy -p qoo-tui --all-targets` (must report no issues)
- Do **not** run `cargo fmt` — the repo is not rustfmt-clean and formatting it would produce a huge unrelated diff. Match the surrounding code style by hand.
- Baseline before starting: 550 tests passing, clippy clean.
- `Space` is unbound today (verified against `keymap.rs`); it is the only new key.
- Marks are session-only. `TabUiState` is never serialized — nothing to persist or migrate.
- Every doc comment you touch must stay accurate; this codebase's comments carry load-bearing invariants.

## Critical Semantics (read before writing any code)

`view::selection_range(sel)` returns `(cursor, cursor)` when `sel.anchor` is `None` — **the cursor row is always inside "the range."** A naive `range ∪ marks` union would therefore mean: mark row 3, move the cursor to row 5, press `x` → row 5 gets removed too, even though the user never marked it. That is a data-loss bug.

The correct rule — and what `selected_positions` below implements — is:

> The cursor row is part of the selection **only** in the degenerate case where there is no anchor and no marks. Once marks exist (or an anchor exists), the selection is exactly `anchored-range ∪ marks`, and a bare cursor row is not implicitly included.

This reduces **exactly** to today's behavior when `marks` is empty, so no existing test changes meaning.

Second subtlety, on **clamping**: `is_bulk_selection` must keep reading the **unclamped** selection (today's `end > start` on the raw `Selection`), while `selected_positions` clamps internally against the current row count. This split is what makes the existing race test `queue_range_requeue_clamps_when_rows_shrink_below_frozen_start` still pass: it anchors rows 3..5, shrinks the visible set to 2 rows, and expects the **bulk** path (`Cmd::RpcSeq`) to fire with exactly 1 surviving call. If `is_bulk_selection` clamped first, the range would collapse to a single row and the code would take the single-target path instead, emitting `Cmd::Rpc` and failing that test.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/qoo-tui/src/app/mode.rs` | `TabUiState` state container | Add `marks: [HashSet<String>; 3]` + `Default` |
| `crates/qoo-tui/src/keymap.rs` | pure key → `AppAction` | Add `AppAction::ToggleMark` + `Space` arm |
| `crates/qoo-tui/src/app/actions.rs` | `apply_action` dispatch, queue bulk verbs | Add `A::ToggleMark` arm; widen `bulk_blocked`, `queue_selection_rows` |
| `crates/qoo-tui/src/view/mod.rs` | pure view-model resolution | Add `selected_positions`; widen `is_bulk_selection`; make `clamp_sel` `pub(crate)` |
| `crates/qoo-tui/src/selectors.rs` | `pane_title` | Take a precomputed count + bulk flag |
| `crates/qoo-tui/src/view/panes.rs` | list rendering | Thread `marks` + `id_of` into `render_list_pane` / `render_collapsed_pane` |
| `crates/qoo-tui/src/app/menus.rs` | worktree bulk-remove resolution, range cleanup | Widen `open_bulk_menu`; `clear_range` → `clear_range_and_marks` |
| `crates/qoo-tui/src/app/mod.rs` | `clear_esc` staging | Clear range + marks together in stage 1 |
| `crates/qoo-tui/src/app/mark_flow_tests.rs` | **new** — mark toggling, persistence, union | Create |

---

### Task 1: Marks state + `Space` toggle

Adds the data model and the keybinding. After this task you can mark rows and they persist across cursor movement, search edits, and snapshot pushes — nothing consumes them yet.

**Files:**
- Modify: `crates/qoo-tui/src/app/mode.rs:124-163` (`TabUiState` + its `Default`)
- Modify: `crates/qoo-tui/src/keymap.rs:19-82` (`AppAction`), `:96-161` (`list_mode_action`)
- Modify: `crates/qoo-tui/src/app/actions.rs:53+` (`apply_action`)
- Create: `crates/qoo-tui/src/app/mark_flow_tests.rs`
- Modify: `crates/qoo-tui/src/app/mod.rs` (register the new test module)

**Interfaces:**
- Consumes: `App::row_identity(&self, pane: ListPane, i: usize) -> Option<String>` (already exists, `app/mod.rs:425`). It resolves against the **filtered/visible** rows and returns `None` for an out-of-range index.
- Produces:
  - `TabUiState.marks: [HashSet<String>; 3]` — indexed by `ListPane::idx()`, parallel to `selections`.
  - `AppAction::ToggleMark`
  - `App::toggle_mark(&mut self) -> bool` (returns `dirty`)

- [ ] **Step 1: Write the failing tests**

Create `crates/qoo-tui/src/app/mark_flow_tests.rs`:

```rust
use super::*;
use crate::ipc::types::{Project, StateSnapshot, TaskInstance, TaskStatus, WorktreeInfo};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};

fn key(c: char) -> Event { Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) }
fn space() -> Event { Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)) }
fn down() -> Event { Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)) }
fn tab() -> Event { Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)) }

fn app_with(snap: StateSnapshot) -> App {
    let mut a = App::new("/tmp/runs".into(), "/tmp/s.sock".into());
    a.size = (120, 40);
    a.update(Event::Snapshot(snap));
    a
}

/// Three queued tasks on `platform`, ids t0/t1/t2 (queue rows, in order).
fn three_queued() -> StateSnapshot {
    let tasks = ["t0", "t1", "t2"]
        .iter()
        .map(|id| {
            let mut t = TaskInstance::default();
            t.id = (*id).into();
            t.status = TaskStatus::Queued;
            t.target.repo = "platform".into();
            t
        })
        .collect();
    StateSnapshot {
        tasks,
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    }
}

fn three_worktrees() -> StateSnapshot {
    let mut wts = HashMap::new();
    wts.insert("platform".into(), vec![
        WorktreeInfo { name: "wt-a".into(), path: "/wt/a".into(), branch: "wt-a".into(), ..Default::default() },
        WorktreeInfo { name: "wt-b".into(), path: "/wt/b".into(), branch: "wt-b".into(), ..Default::default() },
        WorktreeInfo { name: "wt-c".into(), path: "/wt/c".into(), branch: "wt-c".into(), ..Default::default() },
    ]);
    StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        worktrees: wts,
        ..Default::default()
    }
}

fn marks(a: &App, pane: ListPane) -> HashSet<String> {
    a.active_ui().marks[pane.idx()].clone()
}

#[test]
fn space_toggles_the_cursor_row_mark_on_and_off() {
    let mut a = app_with(three_queued());
    let u = a.update(space());
    assert!(u.dirty);
    assert_eq!(marks(&a, ListPane::Queue), HashSet::from(["t0".to_string()]));
    // Toggling the same row again removes it (idempotent round-trip).
    a.update(space());
    assert!(marks(&a, ListPane::Queue).is_empty());
}

#[test]
fn space_does_not_move_the_cursor_or_touch_the_anchor() {
    let mut a = app_with(three_queued());
    a.update(down()); // cursor → row 1
    a.update(space());
    let sel = a.active_ui().selections[ListPane::Queue.idx()];
    assert_eq!(sel, Selection { cursor: 1, anchor: None }, "mark is toggle-in-place");
    assert_eq!(marks(&a, ListPane::Queue), HashSet::from(["t1".to_string()]));
}

#[test]
fn marks_accumulate_across_non_adjacent_rows() {
    let mut a = app_with(three_queued());
    a.update(space()); // mark t0
    a.update(down());
    a.update(down()); // cursor → t2, skipping t1
    a.update(space()); // mark t2
    assert_eq!(marks(&a, ListPane::Queue), HashSet::from(["t0".to_string(), "t2".to_string()]));
}

#[test]
fn cursor_movement_preserves_marks() {
    // Moving the cursor clears the ANCHOR (set_cursor does that today) but must
    // NOT clear marks — moving between rows is exactly how you reach the next
    // row you want to mark.
    let mut a = app_with(three_queued());
    a.update(space());
    a.update(down());
    a.update(down());
    assert_eq!(marks(&a, ListPane::Queue), HashSet::from(["t0".to_string()]));
}

#[test]
fn marks_survive_a_snapshot_push_that_reorders_rows() {
    // Identity-keyed, so a daemon push that reshuffles row order can't
    // invalidate a mark the way an index-keyed one would.
    let mut a = app_with(three_queued());
    a.update(down()); // cursor → t1
    a.update(space()); // mark t1
    a.update(Event::Snapshot(three_queued()));
    assert_eq!(marks(&a, ListPane::Queue), HashSet::from(["t1".to_string()]));
}

#[test]
fn marks_survive_a_search_filter_edit() {
    // The contiguous range is wiped on every search keystroke (update.rs resets
    // Selection, since a filtered-index range is meaningless once the filter
    // changes). Identity-keyed marks must survive it.
    let mut a = app_with(three_queued());
    a.update(space()); // mark t0
    a.update(key('/')); // open search on the queue pane
    a.update(key('t')); // type into the filter
    assert_eq!(marks(&a, ListPane::Queue), HashSet::from(["t0".to_string()]));
}

#[test]
fn marks_are_scoped_per_pane() {
    let mut a = app_with(three_worktrees());
    a.update(space()); // mark queue row (queue is empty here → no-op)
    a.update(tab());
    a.update(tab()); // → worktrees
    a.update(space()); // mark wt-a
    assert_eq!(marks(&a, ListPane::Worktrees), HashSet::from(["wt-a".to_string()]));
    assert!(marks(&a, ListPane::Tasks).is_empty());
}

#[test]
fn space_on_an_empty_pane_is_inert() {
    // No snapshot rows → row_identity returns None → nothing to toggle, no panic.
    let snap = StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    let u = a.update(space());
    assert!(!u.dirty);
    assert!(marks(&a, ListPane::Queue).is_empty());
}
```

Register the module at the bottom of `crates/qoo-tui/src/app/mod.rs`, next to the existing `#[cfg(test)] mod bulk_flow_tests;` declarations:

```rust
#[cfg(test)]
mod mark_flow_tests;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p qoo-tui --lib mark_flow`
Expected: FAIL — compile errors (`no field 'marks' on TabUiState`, `HashSet` unresolved).

- [ ] **Step 3: Add the `marks` field**

In `crates/qoo-tui/src/app/mode.rs`, add the import at the top of the file (next to the existing `use ratatui::layout::Rect;`):

```rust
use std::collections::HashSet;
```

Add the field to `TabUiState` (after `pub selections: [Selection; 3],`):

```rust
    /// Individually MARKED rows per list pane (`Space`), keyed by the pane's
    /// stable row identity — the same string `App::row_identity` produces
    /// (Queue `task_id`, Tasks `{repo}/{name}`, Worktrees `raw_name`). Parallel
    /// to `selections`, indexed by `ListPane::idx()`.
    ///
    /// Identity-keyed rather than index-keyed on purpose: marks must survive a
    /// search-filter edit and a daemon snapshot reorder, both of which
    /// invalidate the index-based `selections` range (see `update.rs`'s
    /// `Mode::Search` handling, which resets `Selection` on every keystroke).
    /// A mark whose identity no longer resolves to any current row is inert by
    /// construction — it simply never matches — so no pruning pass is needed.
    ///
    /// The effective bulk selection is `anchored-range ∪ marks`; see
    /// [`crate::view::selected_positions`] for the exact rule (notably: the
    /// cursor row is NOT implicitly selected once marks exist).
    pub marks: [HashSet<String>; 3],
```

Add it to the `Default` impl (after `selections: [Selection::default(); 3],`):

```rust
            marks: [HashSet::new(), HashSet::new(), HashSet::new()],
```

Note `TabUiState` derives `PartialEq` — `HashSet<String>` is `PartialEq`, so the derive still holds. It also derives `Clone`, which `HashSet` satisfies.

- [ ] **Step 4: Add the keymap arm**

In `crates/qoo-tui/src/keymap.rs`, add the variant to `AppAction` (place it after `ExtendSelection(i32)` so the selection primitives sit together):

```rust
    /// `Space`: toggle the cursor row into the focused pane's marked set — the
    /// non-contiguous half of a bulk selection (`Shift+Arrow` covers the
    /// contiguous half). Toggle-in-place: the cursor does not move and the
    /// anchor is untouched. Live on all three list panes, since marking is a
    /// selection primitive independent of which bulk VERBS a pane supports
    /// (`hit::bulk_allowed` still governs that). Routes to `App::toggle_mark`.
    ToggleMark,
```

Add the key arm to `list_mode_action`. Put it directly above the `KeyCode::Down` arm, so it reads next to the other selection keys:

```rust
        // Space marks/unmarks the cursor row (non-contiguous bulk selection).
        // Ungated: every list pane can build a selection; whether a VERB may act
        // on a bulk selection is `hit::bulk_allowed`'s call, not the keymap's.
        KeyCode::Char(' ') => AppAction::ToggleMark,
```

**Placement matters:** it must come *before* the catch-all `_ => AppAction::None` (obviously), and it does not collide with any existing `Char` arm — `' '` is matched by none of them.

- [ ] **Step 5: Add the keymap unit test**

In `crates/qoo-tui/src/keymap.rs`'s `mod tests`, add:

```rust
    #[test]
    fn space_toggles_a_mark_on_every_list_pane() {
        // Ungated: marking is a selection primitive, live on all three panes.
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char(' ')), f), AppAction::ToggleMark);
        }
    }
```

- [ ] **Step 6: Add the `App` handler**

In `crates/qoo-tui/src/app/actions.rs`, add the dispatch arm to `apply_action`. Put it next to `A::ExtendSelection`:

```rust
            A::ToggleMark => self.toggle_mark(),
```

Add the method to the same `impl App` block, right after `bulk_blocked`:

```rust
    /// `Space`: toggle the focused pane's cursor row in/out of its marked set.
    /// The mark key is the row's stable identity ([`App::row_identity`]), so it
    /// survives search-filter edits and snapshot reorders. Toggle-in-place — the
    /// cursor and anchor are untouched, which is what makes "jump to a row, mark
    /// it, jump to another" work. Inert (not dirty) when the pane has no row
    /// under the cursor (empty pane / cursor past the end).
    pub(super) fn toggle_mark(&mut self) -> bool {
        let Some(pane) = self.focused_list() else { return false };
        let cursor = self.active_ui().selections[pane.idx()].cursor;
        let Some(id) = self.row_identity(pane, cursor) else { return false };
        let marks = &mut self.ui().marks[pane.idx()];
        if !marks.remove(&id) {
            marks.insert(id);
        }
        true
    }
```

`focused_list` and `ui()` are `&mut self` methods on `App` (see `app/mod.rs:434` and its `ui()` accessor); `row_identity` is `&self`. Resolve `id` before taking the `&mut` borrow of `marks`, exactly as written above, or the borrow checker will reject it.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p qoo-tui --lib`
Expected: PASS — all previous tests plus the 8 new `mark_flow_tests` and the keymap test (559 total).

Run: `cargo clippy -p qoo-tui --all-targets`
Expected: no issues.

- [ ] **Step 8: Commit**

```bash
git add crates/qoo-tui/src/app/mode.rs crates/qoo-tui/src/keymap.rs crates/qoo-tui/src/app/actions.rs crates/qoo-tui/src/app/mark_flow_tests.rs crates/qoo-tui/src/app/mod.rs
git commit -m "feat(tui): Space toggles per-pane row marks"
```

---

### Task 2: The union resolver (`selected_positions`)

The pure core. One resolver every consumer will read through, plus the widened `is_bulk_selection` and the `pane_title` count. This task changes two signatures, so it necessarily updates their call sites to keep the build green — but no *behavior* changes yet (marks are still empty everywhere at runtime until Task 4 wires the consumers).

**Files:**
- Modify: `crates/qoo-tui/src/view/mod.rs:49-56` (`clamp_sel`), `:268-283` (`selection_range`, `is_bulk_selection`)
- Modify: `crates/qoo-tui/src/selectors.rs:687-700` (`pane_title`) and its tests at `:2583-2600`
- Modify: `crates/qoo-tui/src/app/actions.rs:41-48` (`bulk_blocked`)
- Modify: `crates/qoo-tui/src/view/panes.rs:821`, `:831`, `:1049`, `:1061`, `:1095`, `:1107`, `:1140`, `:1152` (call sites)

**Interfaces:**
- Consumes: `TabUiState.marks` (Task 1); `view::selection_range(&Selection) -> (usize, usize)` (existing).
- Produces:
  - `view::selected_positions<T>(rows: &[T], sel: &Selection, marks: &HashSet<String>, id_of: impl Fn(&T) -> String) -> Vec<usize>` — visible-row positions in the selection, **ascending**.
  - `view::is_bulk_selection(sel: &Selection, marks: &HashSet<String>) -> bool` — signature widened by one param.
  - `selectors::pane_title(base: &str, selected: usize, bulk: bool) -> String` — signature changed; no longer derives the count itself.
  - `view::clamp_sel` becomes `pub(crate)`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/qoo-tui/src/view/mod.rs` (create a `#[cfg(test)] mod selection_tests` at the end of the file if one doesn't exist):

```rust
#[cfg(test)]
mod selection_tests {
    use super::*;
    use std::collections::HashSet;

    /// Rows are their own identity — keeps the tests about the selection rule,
    /// not about identity extraction.
    fn rows(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("r{i}")).collect()
    }
    fn marks_of(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }
    fn positions(rows: &[String], sel: Selection, marks: &HashSet<String>) -> Vec<usize> {
        selected_positions(rows, &sel, marks, |r| r.clone())
    }

    #[test]
    fn no_anchor_no_marks_selects_only_the_cursor_row() {
        // The degenerate case — exactly today's single-target behavior.
        let r = rows(5);
        let sel = Selection { cursor: 2, anchor: None };
        assert_eq!(positions(&r, sel, &HashSet::new()), vec![2]);
    }

    #[test]
    fn an_anchor_selects_the_inclusive_range() {
        let r = rows(5);
        let sel = Selection { cursor: 3, anchor: Some(1) };
        assert_eq!(positions(&r, sel, &HashSet::new()), vec![1, 2, 3]);
    }

    #[test]
    fn marks_alone_select_exactly_the_marked_rows_not_the_cursor() {
        // THE load-bearing rule: with marks present and no anchor, a cursor
        // parked on an unmarked row must NOT be swept into the selection.
        // Without this, "mark row 0, move to row 3, press x" would destroy row 3.
        let r = rows(5);
        let sel = Selection { cursor: 3, anchor: None };
        assert_eq!(positions(&r, sel, &marks_of(&["r0"])), vec![0]);
    }

    #[test]
    fn range_and_marks_union_in_ascending_order_without_duplicates() {
        // Range [2..=3] plus marks on r0 and r3 (r3 overlaps the range).
        let r = rows(6);
        let sel = Selection { cursor: 3, anchor: Some(2) };
        assert_eq!(positions(&r, sel, &marks_of(&["r0", "r3"])), vec![0, 2, 3]);
    }

    #[test]
    fn a_stale_mark_is_silently_excluded() {
        // "r9" isn't in the current rows (removed by another session / filtered
        // out of the snapshot) — it must resolve to nothing, not panic.
        let r = rows(3);
        let sel = Selection { cursor: 0, anchor: None };
        assert_eq!(positions(&r, sel, &marks_of(&["r9"])), vec![0]);
    }

    #[test]
    fn positions_clamp_against_a_shrunken_row_set() {
        // Race: the range was anchored at 3..=5, then the visible rows shrank to
        // 2. Clamping (mirroring `clamp_sel`) yields the surviving row, matching
        // what `clamp_span` does for the existing bulk paths.
        let r = rows(2);
        let sel = Selection { cursor: 5, anchor: Some(3) };
        assert_eq!(positions(&r, sel, &HashSet::new()), vec![1]);
    }

    #[test]
    fn empty_rows_select_nothing() {
        let r: Vec<String> = vec![];
        let sel = Selection { cursor: 0, anchor: None };
        assert!(positions(&r, sel, &marks_of(&["r0"])).is_empty());
    }

    #[test]
    fn is_bulk_is_true_for_a_range_or_any_mark() {
        let plain = Selection { cursor: 2, anchor: None };
        let ranged = Selection { cursor: 3, anchor: Some(1) };
        assert!(!is_bulk_selection(&plain, &HashSet::new()));
        assert!(is_bulk_selection(&ranged, &HashSet::new()));
        // A SINGLE mark is still a bulk selection: it must route through the
        // bulk path (which reads marks) rather than the single-target path
        // (which reads the cursor row) — otherwise the two would disagree.
        assert!(is_bulk_selection(&plain, &marks_of(&["r0"])));
    }

    #[test]
    fn is_bulk_reads_the_unclamped_selection() {
        // Deliberately NOT clamped: the shrink race (range 3..=5 over 2 rows)
        // must still report bulk so the caller takes the RpcSeq path, matching
        // `queue_range_requeue_clamps_when_rows_shrink_below_frozen_start`.
        let sel = Selection { cursor: 5, anchor: Some(3) };
        assert!(is_bulk_selection(&sel, &HashSet::new()));
    }
}
```

Update the existing `pane_title` tests in `crates/qoo-tui/src/selectors.rs` (they currently pass a `&Selection`; the new signature takes a count + flag). Replace the two tests at `:2586-2600` with:

```rust
    #[test]
    fn pane_title_plain_when_not_bulk() {
        assert_eq!(pane_title("QUEUE", 1, false), "QUEUE");
    }

    #[test]
    fn pane_title_selection_count() {
        assert_eq!(pane_title("WORKTREES", 3, true), "WORKTREES · 3 selected");
        assert_eq!(pane_title("WORKTREES", 2, true), "WORKTREES · 2 selected");
        // A single MARKED row is bulk — the title must say so, or the pane would
        // read "WORKTREES" while a row sits highlighted.
        assert_eq!(pane_title("WORKTREES", 1, true), "WORKTREES · 1 selected");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p qoo-tui --lib`
Expected: FAIL — `cannot find function 'selected_positions'`, `is_bulk_selection` takes 1 argument, `pane_title` argument mismatch.

- [ ] **Step 3: Implement the resolver**

In `crates/qoo-tui/src/view/mod.rs`, add the import at the top:

```rust
use std::collections::HashSet;
```

Make `clamp_sel` `pub(crate)` (it's currently private) — `selected_positions` reuses it, and keeping one clamp definition avoids the two drifting:

```rust
pub(crate) fn clamp_sel(sel: &Selection, len: usize) -> Selection {
```

Replace `is_bulk_selection` (`:280-283`) and add `selected_positions` beneath `selection_range`:

```rust
/// The visible-row positions that make up the effective selection, ASCENDING and
/// deduplicated: the anchored range unioned with the marked rows.
///
/// The rule, stated once (every bulk consumer reads through this function):
///
/// - An **anchored range** contributes `[start, end]` inclusive.
/// - **Marks** contribute any row whose identity is in `marks`.
/// - The **cursor row** contributes ONLY in the degenerate case — no anchor and
///   no marks — where it is the whole selection.
///
/// That last clause is load-bearing. `selection_range` reports `(cursor, cursor)`
/// when there is no anchor, so a naive `range ∪ marks` would silently sweep the
/// cursor row into every marked selection: "mark row 0, move the cursor to row 3,
/// press `x`" would remove row 3 as well. Once the user has marked anything, the
/// cursor is just a viewport — not a selection.
///
/// With `marks` empty this reduces exactly to today's range behavior.
///
/// `sel` is clamped against `rows.len()` internally (same rule as [`clamp_sel`] /
/// `App::clamp_span`), so a daemon snapshot that shrinks the row set between the
/// selection and its use resolves to the surviving rows rather than panicking. A
/// mark whose identity matches no current row is silently dropped.
pub(crate) fn selected_positions<T>(
    rows: &[T],
    sel: &Selection,
    marks: &HashSet<String>,
    id_of: impl Fn(&T) -> String,
) -> Vec<usize> {
    if rows.is_empty() {
        return Vec::new();
    }
    let sel = clamp_sel(sel, rows.len());
    let has_anchor = sel.anchor.is_some();
    let (start, end) = selection_range(&sel);
    let mut out: Vec<usize> = (0..rows.len())
        .filter(|&pos| {
            let in_range = has_anchor && pos >= start && pos <= end;
            in_range || marks.contains(&id_of(&rows[pos]))
        })
        .collect();
    // Degenerate case: nothing anchored and nothing marked → the cursor row IS
    // the selection (today's single-target behavior).
    if out.is_empty() && !has_anchor && marks.is_empty() {
        out.push(sel.cursor);
    }
    out
}

/// Whether the pane's selection is a BULK one — a multi-row range or ANY mark.
/// Drives the not-applicable title-bar chip dimming
/// ([`crate::hit::bulk_allowed`] / `view::panes::button_chip`), the
/// status-line refusal in `App::bulk_blocked`, and the bulk-vs-single-target
/// branch in the `r`/`x` verbs.
///
/// A SINGLE mark counts as bulk: the bulk path resolves rows from `marks`, the
/// single-target path resolves them from the cursor, and with a mark present
/// those two disagree — so the mark must win, or `x` would act on a row the user
/// never marked.
///
/// Reads the UNCLAMPED `sel` on purpose (matching the historical `end > start`
/// on the raw `Selection`): when a snapshot shrinks the rows under a frozen
/// range, the action must still take the bulk path and let
/// [`selected_positions`] clamp to the survivors — clamping here first would
/// collapse the range to one row and silently reroute to the single-target
/// dispatch.
pub(crate) fn is_bulk_selection(sel: &Selection, marks: &HashSet<String>) -> bool {
    let (start, end) = selection_range(sel);
    end > start || !marks.is_empty()
}
```

- [ ] **Step 4: Update `pane_title`**

In `crates/qoo-tui/src/selectors.rs`, replace `pane_title` (`:687-700`) — it no longer derives the count from a `Selection`, because the count is now a union over rows + marks that only the caller can compute:

```rust
/// The pane's border title: the base plus a `· N selected` suffix when the pane
/// holds a BULK selection. `selected` is the union count (range ∪ marks) the
/// caller resolved via `view::selected_positions`; `bulk` is
/// `view::is_bulk_selection`. Both are passed in rather than derived here: a
/// mark-aware count needs the pane's rows, which this pure helper doesn't see.
/// The `/filter` + cursor decoration lives in the inline hint row (see
/// `view::panes`), so it is not part of the title.
pub fn pane_title(base: &str, selected: usize, bulk: bool) -> String {
    if bulk {
        format!("{base} · {selected} selected")
    } else {
        base.to_string()
    }
}
```

- [ ] **Step 5: Update the call sites to keep the build green**

`crates/qoo-tui/src/app/actions.rs` — `bulk_blocked` (`:41-48`) now passes the pane's marks:

```rust
    pub(super) fn bulk_blocked(&mut self, pane: ListPane, btn: crate::hit::PaneButton) -> bool {
        let ui = self.active_ui();
        let sel = ui.selections[pane.idx()];
        let marks = &ui.marks[pane.idx()];
        if !crate::view::is_bulk_selection(&sel, marks) || crate::hit::bulk_allowed(pane.pane_id(), btn) {
            return false;
        }
        self.status_line = Some(BULK_NOT_APPLICABLE.into());
        true
    }
```

(`active_ui()` returns an owned `TabUiState` clone, so binding it to `ui` first keeps `marks` borrowed from a live local rather than a temporary.)

`crates/qoo-tui/src/view/panes.rs` — the three collapsed-pane blocks (`:1049/:1061`, `:1095/:1107`, `:1140/:1152`) each compute a title and a bulk flag. Update each to resolve marks and the union count. For QUEUE:

```rust
    if collapsed[ListPane::Queue.idx()] {
        let marks = &c.ui.marks[ListPane::Queue.idx()];
        let n = selected_positions(&c.queue, &c.queue_sel, marks, |r| r.task_id.clone()).len();
        let bulk = is_bulk_selection(&c.queue_sel, marks);
        let title = pane_title(TITLE_QUEUE, n, bulk);
        render_collapsed_pane(
            frame,
            regions[0],
            &title,
            &queue_summary,
            matches!(c.ui.focus, PaneId::Queue),
            PaneId::Queue,
            pane_buttons(PaneId::Queue),
            &mut btn_hits,
            p,
            spotlight && !c.searching[0],
            bulk,
        );
    } else {
```

TASKS is the same shape with `&c.defs`, `&c.tasks_sel`, `ListPane::Tasks`, `TITLE_TASKS`, and `|d| format!("{}/{}", d.repo, d.name)` as `id_of`. WORKTREES is the same shape with `&c.worktrees`, `&c.wt_sel`, `ListPane::Worktrees`, `TITLE_WORKTREES`, and `|r| r.raw_name.clone()`.

Add `selected_positions` to the `use crate::view::{...}` import at `panes.rs:48`.

`render_list_pane`'s own internals (`:821`, `:831`) still call `pane_title(title_base, sel)` and `is_bulk_selection(sel)`. **Task 3 threads marks into that function properly.** To keep *this* task compiling with no behavior change, pass an empty set at those two sites for now:

```rust
    let no_marks = std::collections::HashSet::new();
    let n_sel = selected_positions(rows, sel, &no_marks, |_| String::new()).len();
    let title = pane_title(title_base, n_sel, is_bulk_selection(sel, &no_marks));
```

and at `:831`, `is_bulk_selection(sel, &no_marks)`.

**This is a deliberate two-step:** Task 2 keeps `render_list_pane` behaviorally identical (empty marks ⇒ old semantics exactly), Task 3 replaces the placeholder with the real `marks` + `id_of` parameters. Do not skip Task 3 — leaving `no_marks` in place would mean marked rows never highlight.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p qoo-tui --lib`
Expected: PASS — 559 from Task 1, plus 9 `selection_tests` and the reworked `pane_title` tests.

Run: `cargo clippy -p qoo-tui --all-targets`
Expected: no issues.

- [ ] **Step 7: Commit**

```bash
git add crates/qoo-tui/src/view/mod.rs crates/qoo-tui/src/selectors.rs crates/qoo-tui/src/app/actions.rs crates/qoo-tui/src/view/panes.rs
git commit -m "feat(tui): mark-aware selection resolver and bulk predicate"
```

---

### Task 3: Render marked rows

Threads the real `marks` set and a per-pane `id_of` closure into `render_list_pane`, replacing Task 2's `no_marks` placeholder. Marked rows now highlight with the same accent tint the range uses, and the expanded pane's title shows the union count.

**Files:**
- Modify: `crates/qoo-tui/src/view/panes.rs` — `render_list_pane` signature + body (`:795-950`), its 3 call sites (`:1064`, `:1110`, `:1155`)

**Interfaces:**
- Consumes: `view::selected_positions`, `view::is_bulk_selection`, `selectors::pane_title` (Task 2); `TabUiState.marks` (Task 1).
- Produces: `render_list_pane` takes two new params — `marks: &HashSet<String>` and `id_of: impl Fn(&T) -> String`.

- [ ] **Step 1: Write the failing test**

Add to `crates/qoo-tui/src/app/mark_flow_tests.rs`:

```rust
/// Render the app to a test buffer and return every line as a String, paired
/// with whether that line carries the selection background (the same accent bg
/// the contiguous range paints).
fn rendered_selected_rows(a: &App) -> Vec<String> {
    use ratatui::{Terminal, backend::TestBackend};
    let mut term = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
    // NOTE: `render`'s signature is `render(app, frame)` — app first.
    term.draw(|f| { crate::view::render(a, f); }).expect("draw");
    let buf = term.backend().buffer().clone();
    // Selected rows are painted with `Palette::selection()`, whose bg is
    // `selection_bg` (see `view/theme.rs`).
    let sel_bg = crate::view::theme::Palette::default().selection_bg;
    let mut out = Vec::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let mut selected = false;
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            if cell.bg == sel_bg {
                selected = true;
            }
            line.push_str(cell.symbol());
        }
        if selected {
            out.push(line.trim().to_string());
        }
    }
    out
}

#[test]
fn a_marked_row_renders_with_the_selection_highlight() {
    let mut a = app_with(three_worktrees());
    a.update(tab());
    a.update(tab()); // → worktrees (focused; highlight only paints when focused)
    a.update(down()); // cursor → wt-b
    a.update(space()); // mark wt-b
    a.update(down()); // cursor → wt-c, leaving wt-b marked but not under the cursor
    let lines = rendered_selected_rows(&a);
    assert!(
        lines.iter().any(|l| l.contains("wt-b")),
        "marked row must stay highlighted after the cursor moves away: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("wt-c")),
        "the bare cursor row is NOT selected once marks exist: {lines:?}"
    );
}
```

These two identifiers were verified against the codebase while writing this plan: the render entry point is `pub fn render(app: &App, frame: &mut ratatui::Frame) -> HitMap` (`view/mod.rs:121` — **app first**, return value discarded inside the `draw` closure), and the selection background is `Palette::selection_bg`, applied via `Palette::selection()` (`view/theme.rs:285-286`). If either has drifted by the time you implement, grep `rg "pub fn render\(" crates/qoo-tui/src/view/mod.rs` and `rg "selection_bg|fn selection" crates/qoo-tui/src/view/theme.rs` and adjust — the test's *assertions* are what matter, not these identifiers.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p qoo-tui --lib a_marked_row_renders`
Expected: FAIL — `wt-b` is not highlighted (`render_list_pane` still resolves with the `no_marks` placeholder from Task 2), and `wt-c` IS highlighted (it's the cursor row).

- [ ] **Step 3: Thread `marks` + `id_of` through `render_list_pane`**

In `crates/qoo-tui/src/view/panes.rs`, add two params to `render_list_pane`. Put `marks` immediately after the existing `sel: &Selection` param, and `id_of` next to the other row closures (beside `dim_of` / `running_of`), so related args stay grouped:

```rust
    sel: &Selection,
    // The pane's MARKED row identities (`Space`). Combined with `sel` into the
    // effective selection by `selected_positions` — see its docs for the rule.
    marks: &HashSet<String>,
```

```rust
    dim_of: impl Fn(&T) -> bool,
    running_of: impl Fn(&T) -> bool,
    // The row's STABLE identity — the mark key. Must match what
    // `App::row_identity` produces for this pane, or a marked row won't
    // highlight (Queue `task_id`, Tasks `{repo}/{name}`, Worktrees `raw_name`).
    id_of: impl Fn(&T) -> String,
```

Add `use std::collections::HashSet;` to the file's imports.

Replace the Task 2 placeholder at `:821`/`:831` with the real resolution:

```rust
    let sel_positions: HashSet<usize> =
        selected_positions(rows, sel, marks, &id_of).into_iter().collect();
    let bulk = is_bulk_selection(sel, marks);
    let title = pane_title(title_base, sel_positions.len(), bulk);
```

and pass `bulk` to `build_header` in place of the old `is_bulk_selection(sel)` argument at `:831`.

Delete the now-unused `let (start_i, end_i) = selection_range(sel);` at `:893`, and replace the per-row `selected` test at `:934`:

```rust
                let selected = focused && sel_positions.contains(&idx);
```

`selection_range` may become an unused import in `panes.rs` — drop it from the `use` list if clippy flags it.

- [ ] **Step 4: Pass the new args at the three call sites**

`render_list_pane` is positional, so each call site gets `marks` after `&c.queue_sel` (etc.) and `id_of` after its `running_of` closure. QUEUE (`:1064`):

```rust
            &c.queue_sel,
            &c.ui.marks[ListPane::Queue.idx()],
            &c.queue,
```

and after its `|row| row.running,` line:

```rust
            |row| row.task_id.clone(),
```

TASKS (`:1110`): `&c.ui.marks[ListPane::Tasks.idx()]` after `&c.tasks_sel`, and `|d| format!("{}/{}", d.repo, d.name),` after its second `|_| false,`.

WORKTREES (`:1155`): `&c.ui.marks[ListPane::Worktrees.idx()]` after `&c.wt_sel`, and `|row| row.raw_name.clone(),` after its `running_of` closure.

**The `id_of` closures must exactly match `App::row_identity` (`app/mod.rs:427-431`)** — Queue `task_id`, Tasks `format!("{}/{}", repo, name)`, Worktrees `raw_name`. A mismatch means marks are stored under one key and looked up under another, and marked rows silently never highlight.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p qoo-tui --lib`
Expected: PASS — including `a_marked_row_renders_with_the_selection_highlight`.

Run: `cargo clippy -p qoo-tui --all-targets`
Expected: no issues.

- [ ] **Step 6: Commit**

```bash
git add crates/qoo-tui/src/view/panes.rs crates/qoo-tui/src/app/mark_flow_tests.rs
git commit -m "feat(tui): highlight marked rows and count them in the pane title"
```

---

### Task 4: Bulk verbs act on marks

Wires the two real consumers: the QUEUE `r`/`x` verbs (requeue / cancel) and the WORKTREES bulk-remove confirm. After this task, marking is functional end-to-end.

**Files:**
- Modify: `crates/qoo-tui/src/app/actions.rs:282-306` (`queue_selection_rows`)
- Modify: `crates/qoo-tui/src/app/actions.rs:486`, `:512`, `:691` (the three `end > start` bulk-path gates)
- Modify: `crates/qoo-tui/src/app/menus.rs` — `open_bulk_menu`'s `ListPane::Worktrees` arm

**Interfaces:**
- Consumes: `view::selected_positions`, `view::is_bulk_selection` (Task 2).
- Produces: `queue_selection_rows` returns `(Vec<QueueSelRow>, bool)` where the `bool` is now `is_bulk` (was `is_range`) — same type, widened meaning.

**Why the branch gates matter:** `open_actions_or_run` (`:486`), `run_or_bulk_selected_task_def` (`:512`), and `remove_selected_worktree` (`:691`) each decide "range → bulk path, else single-target" by testing `end > start` on the raw range. A **marks-only** selection has no anchor, so `end == start` — those gates would route it to the single-target path, and for `remove_selected_worktree` that means removing the *cursor* row while ignoring the marks (the exact data-loss bug `selected_positions` exists to prevent). Each gate must switch to `is_bulk_selection(sel, marks)`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/qoo-tui/src/app/bulk_flow_tests.rs`:

```rust
#[test]
fn worktree_bulk_remove_acts_on_marks_not_the_cursor_row() {
    // Mark wt-a and wt-c, leave the cursor on the UNMARKED wt-b, press x.
    // The confirm must name exactly the two marked worktrees — sweeping the
    // cursor row in would delete a worktree the user never selected.
    let mut a = app_with(three_worktrees());
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))); // → worktrees
    a.update(key(' ')); // mark wt-a (cursor row 0)
    a.update(down());
    a.update(down()); // cursor → wt-c
    a.update(key(' ')); // mark wt-c
    a.update(Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))); // cursor → wt-b (unmarked)
    a.update(key('x'));
    match &a.mode {
        Mode::Confirm { action: ConfirmAction::BulkRemoveWorktrees { names, .. }, .. } => {
            assert_eq!(names, &["wt-a".to_string(), "wt-c".to_string()]);
        }
        other => panic!("expected bulk-remove confirm, got {other:?}"),
    }
}

#[test]
fn worktree_bulk_remove_unions_a_range_with_marks() {
    // Range over wt-a..wt-b (shift+down), plus a mark on wt-c → all three,
    // in visible-row order, no duplicates.
    let mut a = app_with(three_worktrees());
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    a.update(shift_down()); // range = wt-a..wt-b
    a.update(down()); // NOTE: clears the anchor — see the assertion below
    a.update(key(' ')); // mark wt-c
    // Re-establish the range, since `down` collapsed it (set_cursor clears the
    // anchor; marks survive). Cursor is on wt-c: shift+up ranges wt-b..wt-c.
    a.update(Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT)));
    a.update(key('x'));
    match &a.mode {
        Mode::Confirm { action: ConfirmAction::BulkRemoveWorktrees { names, .. }, .. } => {
            // wt-b + wt-c from the range, wt-c also marked (deduped) → 2 names.
            assert_eq!(names, &["wt-b".to_string(), "wt-c".to_string()]);
        }
        other => panic!("expected bulk-remove confirm, got {other:?}"),
    }
}

#[test]
fn a_single_marked_worktree_still_routes_through_the_bulk_confirm() {
    // One mark, cursor elsewhere → bulk path (names the MARKED row), never the
    // single-target path (which would name the cursor row).
    let mut a = app_with(three_worktrees());
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    a.update(key(' ')); // mark wt-a
    a.update(down()); // cursor → wt-b
    a.update(key('x'));
    match &a.mode {
        Mode::Confirm { action: ConfirmAction::BulkRemoveWorktrees { names, .. }, .. } => {
            assert_eq!(names, &["wt-a".to_string()]);
        }
        other => panic!("expected bulk-remove confirm, got {other:?}"),
    }
}

#[test]
fn queue_cancel_acts_on_marks_not_the_cursor_row() {
    // t0 running, t1 queued, t2 queued. Mark t0 and t2, park the cursor on t1,
    // press x → exactly two RPCs (stop t0, skip t2); t1 untouched.
    let mut t0 = TaskInstance::default(); t0.id = "t0".into(); t0.status = TaskStatus::Running; t0.target.repo = "platform".into();
    let mut t1 = TaskInstance::default(); t1.id = "t1".into(); t1.status = TaskStatus::Queued; t1.target.repo = "platform".into();
    let mut t2 = TaskInstance::default(); t2.id = "t2".into(); t2.status = TaskStatus::Queued; t2.target.repo = "platform".into();
    let snap = StateSnapshot {
        tasks: vec![t0, t1, t2],
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.update(key(' ')); // mark t0
    a.update(down());
    a.update(down()); // cursor → t2
    a.update(key(' ')); // mark t2
    a.update(Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))); // cursor → t1 (unmarked)
    a.update(key('x')); // opens the cancel confirm
    let u = a.update(enter()); // confirm
    match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
        Cmd::RpcSeq { verb, calls, .. } => {
            assert_eq!(verb, "cancelled");
            assert_eq!(calls.len(), 2, "only the two MARKED tasks");
            assert_eq!(calls[0].method, "stop"); // t0 running
            assert_eq!(calls[0].params, serde_json::json!({ "id": "t0" }));
            assert_eq!(calls[1].method, "skip"); // t2 queued
            assert_eq!(calls[1].params, serde_json::json!({ "id": "t2" }));
        }
        _ => unreachable!(),
    }
}
```

`key(' ')` works with the existing `fn key(c: char)` helper already in that file. `down()` and `shift_down()` also already exist there.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p qoo-tui --lib bulk_flow`
Expected: FAIL — the confirm names the cursor row / the whole range, because neither consumer reads `marks` yet.

- [ ] **Step 3: Switch the three bulk-path gates to `is_bulk_selection`**

In `crates/qoo-tui/src/app/actions.rs`, three sites currently gate the bulk path on `end > start`. Each must instead ask `is_bulk_selection(sel, marks)` so a marks-only selection (no anchor) takes the bulk path.

`open_actions_or_run` (`:480-502`) — replace the `let (start, end) = ...; if end == start && ...` preamble and the `if end > start` gate:

```rust
    pub(super) fn open_actions_or_run(&mut self) -> Update {
        let ui = self.active_ui();
        let pane = ui.last_list_pane;
        let sel = ui.selections[pane.idx()];
        let marks = &ui.marks[pane.idx()];
        let bulk = crate::view::is_bulk_selection(&sel, marks);
        // Single-row TASKS runs the highlighted def directly (no menu hop).
        if !bulk && pane == ListPane::Tasks {
            return self.run_selected_task_def();
        }
        if bulk {
            let btn = match pane {
                ListPane::Queue => crate::hit::PaneButton::Actions,
                ListPane::Tasks => crate::hit::PaneButton::Run,
                ListPane::Worktrees => crate::hit::PaneButton::Remove,
            };
            if self.bulk_blocked(pane, btn) {
                return Update { dirty: true, cmds: vec![] };
            }
            // `open_bulk_menu` now resolves rows from `selection ∪ marks` itself
            // (the `start`/`end` args are used only by the unreachable Tasks arm),
            // so the frozen range is no longer the source of truth for Worktrees.
            let (start, end) = crate::view::selection_range(&sel);
            return self.open_bulk_menu(pane, start, end);
        }
        match self.open_action_menu() {
            Some(mode) => self.mode = mode,
            None => self.status_line = Some("nothing selected".into()),
        }
        Update { dirty: true, cmds: vec![] }
    }
```

`run_or_bulk_selected_task_def` (`:509-517`) — refuse a marks-only selection too, not just a range:

```rust
    fn run_or_bulk_selected_task_def(&mut self) -> Update {
        let ui = self.active_ui();
        let sel = ui.selections[ListPane::Tasks.idx()];
        let marks = &ui.marks[ListPane::Tasks.idx()];
        if crate::view::is_bulk_selection(&sel, marks) {
            self.status_line = Some(BULK_NOT_APPLICABLE.into());
            return Update { dirty: true, cmds: vec![] };
        }
        self.run_selected_task_def()
    }
```

`remove_selected_worktree` (`:683-693`) — the load-bearing one. Replace the `let (start, end) = ...; if end > start` gate:

```rust
    pub(super) fn remove_selected_worktree(&mut self) -> Update {
        let Some(repo) = self.active_repo() else {
            return Update::default();
        };
        // A bulk selection (multi-row range OR any mark) opens the bulk-remove
        // confirm, which resolves the exact rows from `selection ∪ marks`. A
        // single-row (non-bulk) selection removes just the cursor's worktree.
        let ui = self.active_ui();
        let sel = ui.selections[ListPane::Worktrees.idx()];
        let marks = &ui.marks[ListPane::Worktrees.idx()];
        if crate::view::is_bulk_selection(&sel, marks) {
            let (start, end) = crate::view::selection_range(&sel);
            return self.open_bulk_menu(ListPane::Worktrees, start, end);
        }
```

(The rest of `remove_selected_worktree` — the `selected_worktree_row_filtered` single-row path below the gate — is unchanged. `repo` is still bound for it.)

Run `cargo build -p qoo-tui --tests` after this step; it should compile (the `open_bulk_menu` signature is unchanged, and its Worktrees arm gets rewritten in Step 5). The new bulk tests still fail until Steps 4–5 land.

- [ ] **Step 4: Widen `queue_selection_rows`**

In `crates/qoo-tui/src/app/actions.rs`, replace `queue_selection_rows` (`:282-306`). It resolves visible rows first, then reads the selection through `selected_positions`:

```rust
    /// The QUEUE rows the `r`/`x` verbs act on, plus whether this is a BULK
    /// selection (a multi-row range OR any mark — see
    /// [`crate::view::is_bulk_selection`]). Rows come back in visible-row order.
    ///
    /// Resolution goes through [`crate::view::selected_positions`], so a marked
    /// row is included even when the cursor sits elsewhere, and — critically —
    /// a bare cursor row is NOT swept in once marks exist.
    fn queue_selection_rows(&self) -> Option<(Vec<QueueSelRow>, bool)> {
        let snap = self.snapshot.as_ref()?;
        let repo = self.active_repo()?;
        let ui = self.active_ui();
        let rows = crate::selectors::queue_rows(snap, &repo, self.now_epoch_s);
        let vis = crate::selectors::filter_rows(&rows, &ui.search[0], |r| r.summary.clone());
        // The VISIBLE rows, in view order — the coordinate space the selection
        // and the marks both live in.
        let visible: Vec<&crate::selectors::QueueRow> =
            vis.iter().filter_map(|&i| rows.get(i)).collect();
        let sel = ui.selections[0];
        let marks = &ui.marks[0];
        // `is_bulk` reads the UNCLAMPED selection (see its docs): a range frozen
        // over rows that have since shrunk must still take the bulk path.
        let is_bulk = crate::view::is_bulk_selection(&sel, marks);
        let sels = crate::view::selected_positions(&visible, &sel, marks, |r| r.task_id.clone())
            .into_iter()
            .filter_map(|pos| visible.get(pos).copied())
            .map(|r| {
                let status = snap
                    .tasks
                    .iter()
                    .chain(snap.archived_recent.iter())
                    .find(|t| t.id == r.task_id)
                    .map(|t| t.status)
                    .unwrap_or(TaskStatus::Unknown);
                (r.task_id.clone(), status, r.archived)
            })
            .collect();
        Some((sels, is_bulk))
    }
```

The two callers (`requeue_selected` at `:324`, `cancel_selected` at `:374`) destructure this as `(rows, is_range)` / `(rows, _is_range)`. Rename the binding to `is_bulk` at both sites — behavior is unchanged for them (a bulk selection takes the fan-out path either way), it just reads honestly now.

- [ ] **Step 5: Widen the worktrees arm of `open_bulk_menu`**

In `crates/qoo-tui/src/app/menus.rs`, replace the body of the `ListPane::Worktrees` arm. It currently does `clamp_span` + a `vis[start..=hi]` slice; it now resolves through `selected_positions` (which clamps internally), so `clamp_span` is no longer needed *here*:

```rust
            ListPane::Worktrees => {
                let rows = crate::selectors::worktree_rows(snap, &repo);
                let vis = crate::selectors::filter_rows(&rows, &ui.search[2], |r| r.name.clone());
                let visible: Vec<&crate::selectors::WorktreeRow> =
                    vis.iter().filter_map(|&i| rows.get(i)).collect();
                let sel = ui.selections[2];
                let marks = &ui.marks[2];
                let remove_names: Vec<String> =
                    crate::view::selected_positions(&visible, &sel, marks, |r| r.raw_name.clone())
                        .into_iter()
                        .filter_map(|pos| visible.get(pos).copied())
                        // Eligibility is applied AFTER selection (a session row or
                        // a busy worktree can be marked; it just isn't removable).
                        .filter(|r| !r.is_session && !matches!(r.state, crate::selectors::WtState::Busy))
                        .map(|r| r.raw_name.clone())
                        .collect();
                if remove_names.is_empty() {
                    self.status_line = Some("no eligible rows".into());
                    return Update { dirty: true, cmds: vec![] };
                }
                self.mode = Self::bulk_remove_confirm_mode(repo, remove_names);
                Update { dirty: true, cmds: vec![] }
            }
```

Leave the `ListPane::Tasks` arm alone — it is unreachable from live UI (`hit::bulk_allowed(Tasks, Run)` is `false`, so `bulk_blocked` refuses before any caller reaches it) and is kept only for shape parity. `clamp_span` is still used there, so do not delete it.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p qoo-tui --lib`
Expected: PASS — the 4 new bulk tests, plus every pre-existing bulk test (notably `queue_range_requeue_clamps_when_rows_shrink_below_frozen_start` and `bulk_remove_confirms_then_rpcseq_removes_each`, which pin the marks-empty behavior).

Run: `cargo clippy -p qoo-tui --all-targets`
Expected: no issues.

- [ ] **Step 7: Commit**

```bash
git add crates/qoo-tui/src/app/actions.rs crates/qoo-tui/src/app/menus.rs crates/qoo-tui/src/app/bulk_flow_tests.rs
git commit -m "feat(tui): queue and worktree bulk verbs act on marked rows"
```

---

### Task 5: Esc clears marks; dispatch cleans them up

Closes the loop: one Esc drops the whole bulk selection (range + marks together), and firing a bulk action leaves no stale marks behind for the next selection.

**Files:**
- Modify: `crates/qoo-tui/src/app/mod.rs:639-656` (`clear_esc`)
- Modify: `crates/qoo-tui/src/app/menus.rs` (`clear_range` → `clear_range_and_marks`) and its callers in `menus.rs` / `actions.rs`

**Interfaces:**
- Consumes: `TabUiState.marks` (Task 1).
- Produces: `App::clear_range_and_marks(&mut self, pane: ListPane)` — replaces `App::clear_range`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/qoo-tui/src/app/mark_flow_tests.rs`:

```rust
#[test]
fn esc_clears_range_and_marks_together_in_one_press() {
    // Range and marks are ONE selection from the user's side — a single Esc
    // drops both, rather than making them peel it back in two presses.
    let mut a = app_with(three_queued());
    a.update(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT))); // range t0..t1
    a.update(space()); // mark t1 (the cursor row)
    assert!(!marks(&a, ListPane::Queue).is_empty());
    a.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(marks(&a, ListPane::Queue).is_empty(), "marks cleared");
    assert_eq!(a.active_ui().selections[ListPane::Queue.idx()].anchor, None, "range cleared");
}

#[test]
fn esc_clears_marks_alone_before_falling_through_to_search() {
    // Marks with no range still occupy Esc's first stage — the search filter is
    // only cleared by a SECOND Esc, matching how a range behaves today.
    let mut a = app_with(three_queued());
    a.update(key('/'));
    a.update(key('t')); // filter = "t" (matches all three summaries)
    a.update(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))); // apply, back to List
    a.update(space()); // mark the cursor row
    a.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(marks(&a, ListPane::Queue).is_empty(), "first Esc clears marks");
    assert_eq!(a.active_ui().search[ListPane::Queue.idx()], "t", "search survives the first Esc");
    a.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(a.active_ui().search[ListPane::Queue.idx()].is_empty(), "second Esc clears search");
}

#[test]
fn firing_a_bulk_action_clears_the_marks() {
    // A completed bulk action must not leave marks behind to silently widen the
    // NEXT action's selection.
    let mut a = app_with(three_worktrees());
    a.update(tab());
    a.update(tab()); // → worktrees
    a.update(space()); // mark wt-a
    a.update(key('x')); // bulk-remove confirm
    a.update(key('y')); // confirm
    assert!(marks(&a, ListPane::Worktrees).is_empty(), "marks cleared after dispatch");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p qoo-tui --lib mark_flow`
Expected: FAIL — marks survive Esc and survive a bulk dispatch.

- [ ] **Step 3: Merge marks into Esc's first stage**

In `crates/qoo-tui/src/app/mod.rs`, replace `clear_esc` (`:639-656`):

```rust
    /// Staged Esc in `Mode::List`: (1) drop the pane's bulk selection — the
    /// anchored range AND its marks, together, since from the user's side they
    /// are one selection, not two things to peel back separately; (2) else clear
    /// the pane's search filter. Returns whether anything changed (an Esc with
    /// nothing to clear is inert). Any non-List mode is dismissed first.
    fn clear_esc(&mut self) -> bool {
        if !matches!(self.mode, Mode::List) {
            self.mode = Mode::List;
            return true;
        }
        let Some(pane) = self.focused_list() else { return false };
        let sel = self.ui().selections[pane as usize];
        let has_marks = !self.ui().marks[pane as usize].is_empty();
        if sel.anchor.is_some() || has_marks {
            self.ui().selections[pane as usize] = Selection { cursor: sel.cursor, anchor: None };
            self.ui().marks[pane as usize].clear();
            return true;
        }
        if !self.ui().search[pane as usize].is_empty() {
            self.ui().search[pane as usize].clear();
            self.ui().selections[pane as usize] = Selection { cursor: 0, anchor: None };
            return true;
        }
        false
    }
```

- [ ] **Step 4: Clear marks after every bulk dispatch**

In `crates/qoo-tui/src/app/menus.rs`, rename `clear_range` and widen it:

```rust
    /// Collapse a list pane's bulk selection on the active tab — drop the range
    /// anchor AND the marks. Called before every bulk dispatch (mirroring the
    /// App.tsx `runBulk` clear-then-dispatch order) so a completed action never
    /// leaves a stale selection to widen the next one.
    pub(super) fn clear_range_and_marks(&mut self, pane: ListPane) {
        if let Some(repo) = self.active_repo()
            && let Some(ui) = self.ui_by_tab.get_mut(&repo) {
                ui.selections[pane.idx()].anchor = None;
                ui.marks[pane.idx()].clear();
            }
    }
```

Update every caller. Find them with:

```bash
rg "clear_range\(" crates/qoo-tui/src
```

Expected call sites: `menus.rs` (`execute_menu_action`'s `M::BulkRunDefs` arm) and `update.rs` (the `ConfirmAction::BulkRemoveWorktrees` and `ConfirmAction::CancelTasks` arms — these clear the WORKTREES / QUEUE range before firing the frozen `RpcSeq`). Also check `actions.rs` (`requeue_selected` / `cancel_selected` bulk paths). Rename each `self.clear_range(pane)` → `self.clear_range_and_marks(pane)`. Do not change *which* pane each one clears.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p qoo-tui --lib`
Expected: PASS — everything, including the pre-existing `esc_with_active_range_clears_range_before_it_can_open_bulk` (marks-empty ⇒ unchanged) and the range-cleared assertions in `bulk_remove_confirms_then_rpcseq_removes_each` / `queue_range_cancel_via_x_mixes_stop_and_skip_per_row`.

Run: `cargo clippy -p qoo-tui --all-targets`
Expected: no issues.

- [ ] **Step 6: Update the pane docs**

`crates/qoo-tui/src/keymap.rs`'s module-level doc comment (`:6-17`) describes the key model ("the LEFT-pane cursor moves with the ARROW keys (`shift` extends)"). Extend that sentence to mention `Space`:

```rust
/// ... the LEFT-pane cursor moves with the ARROW keys (`shift` extends the
/// contiguous range; `space` toggles the cursor row's mark, which builds a
/// NON-contiguous selection — the two combine, see `view::selected_positions`).
```

If the app has a help/keymap overlay listing bindings, add `Space` there too. Find it with:

```bash
rg -n "shift" crates/qoo-tui/src/view/help.rs
```

If `view/help.rs` doesn't exist, grep for the overlay that `Mode::Help` renders and add a `space  mark/unmark row` line alongside the existing selection keys, matching the surrounding format exactly.

- [ ] **Step 7: Run the full suite one last time**

Run: `cargo test -p qoo-tui --lib`
Expected: PASS.

Run: `cargo clippy -p qoo-tui --all-targets`
Expected: no issues.

- [ ] **Step 8: Commit**

```bash
git add crates/qoo-tui/src/app/mod.rs crates/qoo-tui/src/app/menus.rs crates/qoo-tui/src/app/update.rs crates/qoo-tui/src/app/actions.rs crates/qoo-tui/src/keymap.rs crates/qoo-tui/src/app/mark_flow_tests.rs
git commit -m "feat(tui): esc clears marks with the range; bulk dispatch resets them"
```

---

## Manual verification (after Task 5)

Run the TUI (`cargo run -p qoo-tui`, or the repo's usual launch task) against a repo with 3+ worktrees:

1. Focus WORKTREES (`Tab` twice). Press `Space` on the first row — it highlights, and the pane title reads `WORKTREES · 1 selected`.
2. Arrow down twice (past an unmarked row) and press `Space` — two non-adjacent rows highlighted, title reads `· 2 selected`. The row between them is **not** highlighted.
3. Move the cursor onto that unmarked middle row and press `x`. The confirm dialog must name **only the two marked worktrees** — not the cursor row.
4. `Esc` out. Press `Esc` again — the selection is gone in one press.
5. Type `/` and filter to a substring, `Enter`, `Space` a row, `Esc` (clears the mark), `Esc` (clears the filter) — and confirm a mark made *before* editing the filter survives the edit (mark a row, `/`, type, and check it's still highlighted).
