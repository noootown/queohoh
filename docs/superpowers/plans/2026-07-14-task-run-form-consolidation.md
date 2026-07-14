# Task-run Form Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify the two task-launch input surfaces onto one field engine, add a worktree/target arg type rendered as a type-or-pick combobox, and make the shared textarea Claude-Code-like (auto-grow + visual-line navigation).

**Architecture:** One field engine (`view::form::FormState` + `app/form.rs` `form_key`) drives both a centered-modal shell (`render_form`, unchanged look) and a two-panel picker shell (def-args: shared fields left, `prompt.md` preview right). `Mode::DefArgs` stops carrying `ArgsForm` and carries a `FormState` plus its launch context; `ArgsForm` and its bespoke editor retire. A new `FieldKind::Combobox` and an `ArgSpec.type = "worktree"` discriminator let pr-ready/pr-review target a worktree (existing or typed pr/ticket → daemon materializes via the existing `params.ref` path).

**Tech Stack:** Rust (`ratatui`, `insta` snapshots, `unicode-width`), TypeScript (`zod`, `vitest`) for the definition schema, plus YAML task definitions in the separate `~/workspace/queohoh` repo.

## Global Constraints

- No `Co-Authored-By` trailers on commits (global user rule).
- No inline glyph char literals in components — every glyph literal lives in `crates/qoo-tui/src/view/theme.rs`.
- Concurrency: the working tree already has an unrelated in-flight change to `crates/qoo-tui/src/view/menu.rs` from a concurrent session. NEVER `git add .`; stage only this plan's files by explicit path with `command git add <path>`. Never stage, revert, or build on `menu.rs`.
- TS type-shape changes must pass `pnpm -r build` (tsc) — `vitest` transpiles per-file and does NOT cross-file type-check.
- App-wide form key standard (already established, preserve it): Tab/Shift-Tab are the ONLY focus movers; arrow keys are inner-navigation only and never step focus; Shift+Enter inserts a newline; only the Primary button submits.
- Two repos: TUI + core/daemon live in `/Users/noootown/Downloads/agent247/queohoh.improvement`; task definitions + prompts live in `/Users/noootown/workspace/queohoh` (separate git repo, separate commits).
- Final verify gate (all green): `pnpm -r build && pnpm -r test && cargo test`.

---

## File Structure

TUI + core/daemon (`queohoh.improvement`):

- `packages/core/src/definition.ts` — add `ArgSpec.type?: "worktree"` + validation (mutually exclusive with `options`).
- `crates/qoo-tui/src/ipc/types.rs` — mirror `ArgSpec.type` (deserialize).
- `crates/qoo-tui/src/view/multiline_input.rs` — width-aware visual-line `move_up_visual`/`move_down_visual`.
- `crates/qoo-tui/src/view/form.rs` — `Field.readonly`, `FieldKind::Combobox`, auto-grow height, extracted `render_fields`/`render_open_dropdown`, combobox render + state, cached content width.
- `crates/qoo-tui/src/app/form.rs` — combobox key handling, width-aware vertical nav, cached width update.
- `crates/qoo-tui/src/view/args_form.rs` — retire `ArgsForm` + `render_run_form` + inline renderer; KEEP `wrap_value_cursor`/`caret_line` (relocate to `multiline_input.rs` if the file is deleted).
- `crates/qoo-tui/src/view/def_args.rs` — NEW: the two-panel picker shell that renders a `FormState` left + `prompt.md` preview right.
- `crates/qoo-tui/src/app/mode.rs` — `Mode::DefArgs { state, repo, def_name, args, initial_worktree, preview_scroll }`.
- `crates/qoo-tui/src/app/def_args.rs` — key/click handling routed through the shared `FormState`; build `FormState` from args in `open_def_args`; submit resolves the worktree field to a `ref`.
- `crates/qoo-tui/src/app/actions.rs` — `run_definition_cmd` gains a `ref` param; open paths build the `FormState`.
- `crates/qoo-tui/src/app/mouse.rs`, `menus.rs` — DefArgs click routing + preview scroll against the new mode shape.
- `crates/qoo-tui/src/ref_classify.rs` — NEW: TUI-side ref classifier (bare number / `#N` / PR URL / ticket / Linear URL → canonical ref string).

Definitions (`~/workspace/queohoh`):

- `platform/tasks/pr-ready/config.yaml`, `platform/tasks/pr-review/config.yaml`, `platform/tasks/pr-review/prompt.md`.

---

## Phase 0 — ArgSpec.type schema (foundation)

### Task 0: `ArgSpec.type` in both languages

**Files:**
- Modify: `packages/core/src/definition.ts:14-28` (interface + zod schema), `:84-107` (`normalizeArgs` validation)
- Modify: `crates/qoo-tui/src/ipc/types.rs:164-169` (`ArgSpec` struct)
- Test: `packages/core/src/definition.test.ts` (add cases; create if absent)

**Interfaces:**
- Produces: TS `ArgSpec.type?: "worktree"`; Rust `ArgSpec.type: Option<String>` (serde field `type`).

- [ ] **Step 1: Write the failing TS test**

In `packages/core/src/definition.test.ts` (find the existing `loadDefinition`/`normalizeArgs` tests; if the file does not exist, create it mirroring a sibling `*.test.ts` harness that writes a temp `tasks/<name>/config.yaml` + `prompt.md` and calls `loadDefinition`):

```ts
it("accepts type: worktree and rejects type+options together", () => {
  // a worktree-typed arg parses
  expect(() =>
    normalizeArgsForTest([{ name: "pr", type: "worktree" }]),
  ).not.toThrow();
  // type + options is contradictory
  expect(() =>
    normalizeArgsForTest([{ name: "pr", type: "worktree", options: ["a"] }]),
  ).toThrow(/type.*worktree.*options/i);
});
```

If `normalizeArgs` is not exported, export a thin `normalizeArgsForTest` or test through `loadDefinition` with a written YAML file. Prefer testing through the public surface if that is the file's existing convention.

- [ ] **Step 2: Run it — expect FAIL** `pnpm --filter @queohoh/core test definition` → FAIL (unknown key `type` rejected by `.strict()`).

- [ ] **Step 3: Implement**

`definition.ts` interface:

```ts
export interface ArgSpec {
	name: string;
	type?: "worktree";
	default?: string;
	options?: string[];
	description?: string;
}
```

zod schema (add to the `.object({...})` before `.strict()`):

```ts
const ArgSpecSchema = z
	.object({
		name: z.string().min(1),
		type: z.literal("worktree").optional(),
		default: z.string().optional(),
		options: z.array(z.string().min(1)).min(1).optional(),
		description: z.string().optional(),
	})
	.strict();
```

In `normalizeArgs`, inside the per-spec loop (after the duplicate-name check), add:

```ts
if (spec.type === "worktree" && spec.options) {
	throw new Error(
		`arg ${spec.name}: type "worktree" cannot combine with options`,
	);
}
```

- [ ] **Step 4: Run — expect PASS**, then `pnpm -r build` (tsc) — expect PASS (new optional field type-checks).

- [ ] **Step 5: Implement the Rust mirror** in `crates/qoo-tui/src/ipc/types.rs`:

```rust
pub struct ArgSpec {
    pub name: String,
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    pub default: Option<String>,
    pub options: Option<Vec<String>>,
    pub description: Option<String>,
}
```

Add a helper next to it:

```rust
impl ArgSpec {
    /// True when this arg is the worktree/target selector (rendered as a
    /// combobox; resolves to a ref on submit).
    pub fn is_worktree(&self) -> bool {
        self.r#type.as_deref() == Some("worktree")
    }
}
```

Update every literal `ArgSpec { .. }` construction in the crate to include `r#type: None` (grep `ArgSpec {` across `crates/qoo-tui` — the test helpers in `args_form.rs`, `worktree_context.rs`, and elsewhere use struct literals with `..arg(...)` spreads; only the base builders need the field).

- [ ] **Step 6: Run — expect PASS** `cargo test -p qoo-tui` (compiles + existing tests pass).

- [ ] **Step 7: Commit**

```bash
command git add packages/core/src/definition.ts packages/core/src/definition.test.ts crates/qoo-tui/src/ipc/types.rs
command git commit -m "feat: ArgSpec.type worktree discriminator (core + tui mirror)"
```

---

## Phase 1 — Claude-Code textarea (spec Part 3)

The shared `MultilineInput` + `FormState` gain visual-line navigation and auto-grow. This lands first and self-contained; both shells inherit it.

### Task 1: Visual-line navigation in `MultilineInput`

**Files:**
- Modify: `crates/qoo-tui/src/view/multiline_input.rs`
- Test: same file's `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::view::args_form::wrap_value_cursor(value, cursor, width) -> (Vec<String>, usize, usize)` (visual rows + caret row/col).
- Produces: `MultilineInput::move_up_visual(&mut self, width: usize)` and `move_down_visual(&mut self, width: usize)` — move the caret by one VISUAL (wrapped) row at `width`, preserving visual column; inert at the first/last visual row.

- [ ] **Step 1: Write the failing test** — the case that fails today (a single long logical line wrapping across visual rows):

```rust
#[test]
fn visual_move_traverses_wrapped_rows_of_one_logical_line() {
    // width 4, one logical line "abcdefghij" wraps to ["abcd","efgh","ij"].
    // caret at index 9 ('j') → visual row 2, col 1.
    let mut m = ml("abcdefghij", 9);
    m.move_up_visual(4); // → row 1 col 1 → index 5 ('f')
    assert_eq!(m.cursor, 5);
    m.move_up_visual(4); // → row 0 col 1 → index 1 ('b')
    assert_eq!(m.cursor, 1);
    m.move_up_visual(4); // already top → inert
    assert_eq!(m.cursor, 1);
    m.move_down_visual(4); // → row 1 col 1 → index 5
    assert_eq!(m.cursor, 5);
}

#[test]
fn visual_move_clamps_column_onto_short_last_row() {
    // width 4: rows ["abcd","efgh","ij"]; caret at index 2 (row0 col2).
    let mut m = ml("abcdefghij", 2);
    m.move_down_visual(4); // row1 col2 → index 6
    assert_eq!(m.cursor, 6);
    m.move_down_visual(4); // row2 len2, col clamped to 2 → index 10 (end)
    assert_eq!(m.cursor, 10);
}
```

- [ ] **Step 2: Run — expect FAIL** `cargo test -p qoo-tui multiline_input::tests::visual_move` → FAIL (methods absent).

- [ ] **Step 3: Implement** in `multiline_input.rs` (add `use crate::view::args_form::wrap_value_cursor;` at top):

```rust
impl MultilineInput {
    /// Char index of the caret after moving `delta` visual rows at `width`,
    /// preserving the visual column (clamped to the target row's length).
    /// Inert (returns the current cursor) past the first/last visual row.
    fn visual_target(&self, width: usize, delta: isize) -> usize {
        let w = width.max(1);
        let (rows, cur_row, cur_col) = wrap_value_cursor(&self.text, self.cursor, w);
        let target = cur_row as isize + delta;
        if target < 0 || target as usize >= rows.len() {
            return self.cursor;
        }
        let target = target as usize;
        // Char index of the start of visual row `target` = sum of prior rows'
        // char lengths, minus a `\n` that was consumed at a hard-line boundary.
        // Simpler + robust: recompute by walking the wrap rows and counting the
        // consumed source characters (each visual row consumes its own chars;
        // a hard newline consumes one extra `\n` not present in any row string).
        // Use `wrap_row_char_starts` to get exact source indices.
        let starts = wrap_row_char_starts(&self.text, w);
        let base = starts[target];
        let row_len = rows[target].chars().count();
        base + cur_col.min(row_len)
    }

    pub fn move_up_visual(&mut self, width: usize) {
        self.cursor = self.visual_target(width, -1);
    }
    pub fn move_down_visual(&mut self, width: usize) {
        self.cursor = self.visual_target(width, 1);
    }
}

/// Source char index at the start of each visual row for `text` wrapped to
/// `width` — the inverse mapping `wrap_value_cursor` implies. A hard `\n`
/// terminates a row and is itself consumed (not part of any row string).
fn wrap_row_char_starts(text: &str, width: usize) -> Vec<usize> {
    let w = width.max(1);
    let mut starts = vec![0usize];
    let mut col = 0usize;
    for (idx, ch) in text.chars().enumerate() {
        if ch == '\n' {
            starts.push(idx + 1); // next row starts AFTER the newline
            col = 0;
        } else {
            if col == w {
                starts.push(idx); // soft-wrap: next row starts AT this char
                col = 0;
            }
            col += 1;
        }
    }
    starts
}
```

Note: keep the existing logical-line `move_up`/`move_down` (still used elsewhere/tests). The new methods are additive.

- [ ] **Step 4: Run — expect PASS** `cargo test -p qoo-tui multiline_input` (new + existing pass). Add a test asserting `wrap_row_char_starts` agrees with `wrap_value_cursor` on the caret row for several widths, to pin the inverse-mapping invariant:

```rust
#[test]
fn wrap_row_char_starts_matches_wrap_value_cursor() {
    let text = "abc\ndefghij\nk";
    for w in [1usize, 2, 3, 4, 7] {
        let starts = wrap_row_char_starts(text, w);
        for cur in 0..=text.chars().count() {
            let (_rows, row, _col) = wrap_value_cursor(text, cur, w);
            assert!(starts[row] <= cur, "w={w} cur={cur} row={row}");
            if row + 1 < starts.len() {
                assert!(cur <= starts[row + 1], "w={w} cur={cur}");
            }
        }
    }
}
```

- [ ] **Step 5: Commit**

```bash
command git add crates/qoo-tui/src/view/multiline_input.rs
command git commit -m "feat(tui): visual-line caret navigation in MultilineInput"
```

### Task 2: `FormState` caches content width + width-aware vertical nav

**Files:**
- Modify: `crates/qoo-tui/src/view/form.rs` (`FormState` struct + `move_up`/`move_down`), `crates/qoo-tui/src/app/form.rs` (pass cached width)
- Test: `form.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `MultilineInput::move_up_visual/move_down_visual` (Task 1).
- Produces: `FormState.content_width: usize` (last-rendered inner text width, default 40); `FormState::set_content_width(usize)`; `move_up`/`move_down` use visual navigation at `content_width` for a focused Textarea.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn textarea_vertical_nav_is_visual_at_cached_width() {
    let mut f = FormState::new("t", "OK", vec![Field::textarea("p", "abcdefghij", true)]);
    f.focus = 0;
    f.set_content_width(4); // rows: abcd/efgh/ij
    f.caret = 9;            // 'j', visual row 2 col 1
    f.move_up(); // → visual row1 col1 → index 5
    assert_eq!(f.caret, 5);
}
```

- [ ] **Step 2: Run — expect FAIL** (`set_content_width` absent; `move_up` still logical).

- [ ] **Step 3: Implement**: add `pub content_width: usize` to `FormState` (init to `40` in `new`). Add:

```rust
pub fn set_content_width(&mut self, w: usize) {
    self.content_width = w.max(1);
}
```

Change `move_up`/`move_down` to use the visual variants at `content_width`:

```rust
pub fn move_up(&mut self) {
    if self.is_textarea_focused() {
        let w = self.content_width;
        self.edit(|mi| mi.move_up_visual(w));
    }
}
pub fn move_down(&mut self) {
    if self.is_textarea_focused() {
        let w = self.content_width;
        self.edit(|mi| mi.move_down_visual(w));
    }
}
```

- [ ] **Step 4: Run — expect PASS** `cargo test -p qoo-tui form::tests`.

- [ ] **Step 5: Wire the cached width at render time.** In `render_form` (and later `render_fields`, Task 5) the text field's content width is `content.width.saturating_sub(1)` (the caret reserve). Because render takes `&FormState` (immutable), store the width from the key-handler side instead: in `app/form.rs`, before handling `Up`/`Down`, the last render already set it — but render is immutable. Simplest robust approach: compute the width in `render_form` and stash via a `Cell<usize>`? No — keep it pure. Instead, have `app/form.rs` set the width from the known modal geometry is brittle. **Chosen approach:** make `render_fields` return the per-textarea content width and have the caller (the frame builder in `view/mod.rs`) call `state.set_content_width(...)` on the NEXT frame is also awkward.

  Final decision (implement this): give `render_form`/`render_fields` a `&mut FormState` so they can call `set_content_width(text_content_width)` during layout. Rendering already borrows the state immutably; widen it to `&mut` at the two call sites (`view/mod.rs` dispatch for `Mode::Form` and the new `Mode::DefArgs`). This makes the cached width exact and one-frame-fresh. Update the render fn signature:

```rust
pub fn render_form(frame: &mut Frame, hit: &mut HitMap, state: &mut FormState) { /* ... */ }
```

Inside, when laying out the focused text field, call `state.set_content_width(wrap_w)` where `wrap_w = content.width.saturating_sub(1).max(1)`. (Borrow care: compute `wrap_w` before the immutable-borrow loop, or set it in a dedicated pre-pass that finds the focused textarea's box width.)

- [ ] **Step 6: Update the two render call sites** in `crates/qoo-tui/src/view/mod.rs` to pass `&mut` state (grep `render_form(`). Adjust `Mode::Form { state, .. }` destructuring to `&mut`.

- [ ] **Step 7: Run — expect PASS** `cargo test -p qoo-tui` and `cargo build`.

- [ ] **Step 8: Commit**

```bash
command git add crates/qoo-tui/src/view/form.rs crates/qoo-tui/src/app/form.rs crates/qoo-tui/src/view/mod.rs
command git commit -m "feat(tui): FormState caches content width; textarea nav is visual"
```

### Task 3: Auto-grow textarea height

**Files:**
- Modify: `crates/qoo-tui/src/view/form.rs` (`Field::box_content_height`, `render_form` height math)
- Test: `form.rs` snapshot + a height unit test

**Interfaces:**
- Produces: a Textarea's content height grows from 3 up to an available cap given its value's wrapped row count, then internal scroll (existing windowing).

- [ ] **Step 1: Write the failing test** — a Textarea with 6 wrapped rows at a known width reports height > 3:

```rust
#[test]
fn textarea_autogrows_with_content() {
    // helper: content rows for a value at width w
    assert_eq!(textarea_rows("a\nb\nc\nd\ne\nf", 40), 6); // 6 logical lines
    assert_eq!(textarea_rows("", 40), 3);                 // floor at 3
    assert_eq!(textarea_rows("x", 40), 3);
}
```

Define `pub(crate) fn textarea_rows(value: &str, width: usize) -> u16` in `form.rs`: `wrap_value_cursor(value, 0, width.max(1)).0.len()` clamped to `3..=AUTOGROW_CAP` where `const AUTOGROW_CAP: u16 = 12;`.

- [ ] **Step 2: Run — expect FAIL** (`textarea_rows` absent).

- [ ] **Step 3: Implement** `textarea_rows` and use it. Replace `Field::box_content_height` for the Textarea branch so the render computes height from the value. Because `box_content_height` has no width/value context beyond `&self`, compute the height in `render_form` where `content.width` is known: for a Textarea field, `let content_h = textarea_rows(&f.value, wrap_w).min(available_rows);` and use `content_h + 2` as the box height (border top/bottom). The final field beyond `available_rows` scrolls internally (the existing `start = cur_row.saturating_sub(rows-1)` windowing already handles a caret past the visible cap). Cap `available_rows` to the modal's remaining interior so the modal never overflows the screen.

- [ ] **Step 4: Update the height accumulation** (`fields_h`, `field_h`) to use the value/width-aware height for Textarea fields (thread `wrap_w` in, or precompute per-field heights into a `Vec<u16>` before the draw loop and reuse it for both sizing and drawing).

- [ ] **Step 5: Run — expect PASS** unit test; update/refresh the `form_snapshot` (`form_create_worktree`) snapshot: the empty prompt still renders 3 rows, so the snapshot may be unchanged; if it changed, review the diff and `command git add` the `.snap`.

- [ ] **Step 6: Add a snapshot** `form_autogrow` with a multi-line prompt value to pin the grown height; write with `insta::assert_snapshot!`.

- [ ] **Step 7: Run — expect PASS** `cargo test -p qoo-tui` (accept new snapshots: review `.snap.new` → rename to `.snap`).

- [ ] **Step 8: Commit**

```bash
command git add crates/qoo-tui/src/view/form.rs crates/qoo-tui/src/view/snapshots/
command git commit -m "feat(tui): auto-grow textarea height in the shared form"
```

---

## Phase 2 — One field engine, two shells (spec Part 1)

### Task 4: Read-only Field flavor

**Files:**
- Modify: `crates/qoo-tui/src/view/form.rs`
- Test: `form.rs` tests

**Interfaces:**
- Produces: `Field.readonly: bool`; `Field::readonly(label, value)` constructor; focus movement skips read-only fields; edits/validation ignore them; render dims them.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn readonly_fields_are_focus_skipped_and_not_edited() {
    let mut f = FormState::new("t", "OK", vec![
        Field::readonly("target", "JUS-1"),
        Field::input("name", "", true),
    ]);
    assert_eq!(f.focus_kind(), FocusKind::Field(1)); // starts past the readonly
    f.insert_char('x'); // edits field 1, not the readonly
    assert_eq!(f.fields[1].value, "x");
    f.focus_next(); // → Primary (skips back over readonly on wrap too)
    assert_eq!(f.focus_kind(), FocusKind::Primary);
    f.focus_next(); // → Cancel
    f.focus_next(); // → wraps to field 1 (skips readonly field 0)
    assert_eq!(f.focus_kind(), FocusKind::Field(1));
}
```

- [ ] **Step 2: Run — expect FAIL** (`Field::readonly` absent; focus lands on 0).

- [ ] **Step 3: Implement**: add `pub readonly: bool` to `Field` (default `false` in every constructor). Add:

```rust
pub fn readonly(label: &str, value: &str) -> Self {
    Field { label: label.into(), kind: FieldKind::Input, value: value.into(), required: false, readonly: true }
}
```

`FormState::new` initial focus: scan for the first non-readonly field. `focus_next`/`focus_prev`: loop until a non-readonly field (or a button) is reached. `land_caret`/`focused_text_field`: treat a readonly field as non-editable (return `None`). `validate`: skip readonly fields. Render: a readonly field uses `p.dim_style()` for both border and value and never shows the accent/focus border.

- [ ] **Step 4: Run — expect PASS** `cargo test -p qoo-tui form`.

- [ ] **Step 5: Commit**

```bash
command git add crates/qoo-tui/src/view/form.rs
command git commit -m "feat(tui): read-only Field flavor (focus-skipped, non-editable)"
```

### Task 5: Extract `render_fields` + `render_open_dropdown`

**Files:**
- Modify: `crates/qoo-tui/src/view/form.rs`
- Test: existing `form.rs` render tests (no behavior change)

**Interfaces:**
- Produces:
  - `pub(crate) fn render_fields(frame, hit, state: &mut FormState, inner: Rect) -> Option<(Rect, Vec<String>)>` — draws every field box into `inner` (top-down, reserving no button row), registers `FormField(i)` hit targets + the caret, caches content width, and returns the open dropdown/combobox anchor box + its option list (else `None`).
  - `pub(crate) fn render_open_dropdown(frame, hit, state, area, anchor, options)` — the bordered option popup, `DropdownItem(i)` targets.
- Consumes: called by `render_form` (Task 5) and the two-panel `render_def_args` (Task 8).

- [ ] **Step 1:** Refactor `render_form` so its per-field draw loop becomes `render_fields`, and its open-dropdown block becomes `render_open_dropdown`. `render_form` keeps: modal chrome (`Block` + `Clear` + `Modal` hit), computes the inner rect, calls `render_fields(frame, hit, state, inner_without_button_row)`, draws the button row (`render_button_row`), then calls `render_open_dropdown` with the returned anchor. This is a pure extraction — the rendered bytes must not change.

- [ ] **Step 2: Run — expect PASS** `cargo test -p qoo-tui form` including the `form_snapshot` (bytes unchanged). If the snapshot changed, the extraction altered layout — fix until identical.

- [ ] **Step 3: Commit**

```bash
command git add crates/qoo-tui/src/view/form.rs
command git commit -m "refactor(tui): extract render_fields/render_open_dropdown from render_form"
```

### Task 6: `Mode::DefArgs` carries `FormState` + launch context

**Files:**
- Modify: `crates/qoo-tui/src/app/mode.rs`, `crates/qoo-tui/src/app/def_args.rs` (`open_def_args`), `crates/qoo-tui/src/app/actions.rs` (open paths)
- Test: `crates/qoo-tui/src/app/menu_flow_tests.rs` / `def_pick_tests.rs`

**Interfaces:**
- Produces:
```rust
DefArgs {
    state: crate::view::form::FormState,
    repo: String,
    def_name: String,
    args: Vec<crate::ipc::types::ArgSpec>, // declaration order == state.fields order
    initial_worktree: Option<String>,      // launch worktree, if any
    preview_scroll: usize,
}
```
- Produces: `App::form_from_args(repo, args, fixed, initial, worktree) -> FormState` — builds one field per arg in order: read-only when the arg name is in `fixed`; Dropdown when `options` present; Combobox when `is_worktree()` (Task 12); else Input (single-token args) or Textarea (free-text). Default value precedence mirrors the old `initial_value` (fixed → initial → default → first option → empty).

- [ ] **Step 1: Write the failing test** in `menu_flow_tests.rs`: opening def-args for a def with a fixed arg + an enum arg produces a `FormState` whose fields match (read-only fixed, dropdown enum) and whose focus starts on the first editable field.

```rust
#[test]
fn open_def_args_builds_formstate_fields() {
    let mut app = /* app seeded with the def */;
    app.open_def_args("platform".into(), "pr-ready".into(),
        vec![arg_enum("review", &["full-review","bypass-review"]), arg("pr")],
        map(&[]), map(&[]), None);
    let Mode::DefArgs { state, .. } = &app.mode else { panic!() };
    assert!(matches!(state.fields[0].kind, FieldKind::Dropdown{..}));
    assert_eq!(state.focus, 0);
}
```

- [ ] **Step 2: Run — expect FAIL** (compile error: `Mode::DefArgs { form }` shape).

- [ ] **Step 3: Implement** the new `Mode::DefArgs` variant (update `mode.rs` + its doc comment). Implement `form_from_args`. Rewrite `open_def_args` to build the `FormState` and set `Mode::DefArgs { state, repo, def_name, args, initial_worktree: worktree, preview_scroll: 0 }`. Keep the `ensure_full_def` prefetch return.

- [ ] **Step 4:** Fix the two open call sites (`actions.rs::run_selected_task_def`, `def_args.rs::def_pick_activate`) — they already pass `args/fixed/initial/worktree` into `open_def_args`; only the mode shape changes.

- [ ] **Step 5: Run — expect PASS** compile + the new test. Other DefArgs tests will fail to compile until Tasks 7-9; if using strict per-task commits, gate this task's compile by temporarily `#[allow(dead_code)]`-ing the not-yet-updated handlers, OR fold Tasks 6-9 into one commit (recommended — they are one refactor and cannot compile independently). Prefer: implement Tasks 6-9 together, commit once at the end of Task 9.

- [ ] **Step 6:** (No commit yet — continue to Task 7.)

### Task 7: DefArgs key handling via the shared engine

**Files:**
- Modify: `crates/qoo-tui/src/app/def_args.rs` (`def_args_key`, `submit_def_args`)
- Test: `def_pick_tests.rs` / `menu_flow_tests.rs`

**Interfaces:**
- Consumes: `FormState` methods (`focus_next/prev`, `move_up/down/left/right`, `insert_char`, `backspace`, `open_dropdown`, `dropdown_move/pick`, `insert_newline`, `is_dropdown_focused`, `validate`).
- Produces: `def_args_key` mirrors `form_key` semantics exactly (Tab focus; arrows inner-nav; Shift+Enter newline; Enter opens a focused dropdown/combobox or, on the Primary button, submits; Esc cancels). The two-panel shell has no Primary/Cancel buttons drawn inside `render_fields`, but the picker draws them (Task 8), so `FocusKind::Primary`/`Cancel` remain the submit/cancel stops.

- [ ] **Step 1:** Replace the body of `def_args_key` to route through `self.mode`'s `FormState` using the same match arms as `form_key` (copy the structure; the only differences are the mode pattern and that submit calls `submit_def_args`). Update `submit_def_args` to read `Mode::DefArgs { state, repo, def_name, args, initial_worktree, .. }`, call `state.validate()`, and on Ok build the run command (Task 15 adds the `ref` resolution; for now pass positional values + `initial_worktree` as before via `run_definition_cmd(repo, def_name, &values, initial_worktree.as_deref(), None)`).

- [ ] **Step 2:** Keep the preview-scroll wheel handling out of keys (it is mouse-only).

### Task 8: Two-panel picker render over `FormState`

**Files:**
- Create: `crates/qoo-tui/src/view/def_args.rs` (`render_def_args`)
- Modify: `crates/qoo-tui/src/view/mod.rs` (dispatch `Mode::DefArgs` → `render_def_args`), `crates/qoo-tui/src/view/args_form.rs` (stop using `render_run_form` from the dispatch)
- Test: snapshot in `view/def_args.rs`

**Interfaces:**
- Consumes: `picker_layout`, `render_preview_markup`/`render_preview` (from `view::menu`), `render_fields`/`render_open_dropdown` (Task 5), `render_button_row` (`view::modal`).
- Produces: `pub fn render_def_args(frame, hit, p, state: &mut FormState, def_name, prompt: Option<&str>, preview_scroll) -> PreviewMetrics` — left panel = bordered fields (via `render_fields`) + button row; right panel = prompt preview; open dropdown/combobox popup last.

- [ ] **Step 1:** Implement `render_def_args` mirroring the OLD `render_run_form` two-panel shell but drawing the LEFT interior with `render_fields` (bordered boxes) + `render_button_row` on the left's last line, and the open popup via `render_open_dropdown`. Reserve the left button row before calling `render_fields` (pass `left_inner` shrunk by one row).

- [ ] **Step 2:** In `view/mod.rs`, dispatch `Mode::DefArgs { state, def_name, preview_scroll, .. }` (mut) to `render_def_args`, sourcing the prompt from the cached full def (same lookup the old code used via `full: Option<&TaskDefinition>`).

- [ ] **Step 3: Snapshot** `def_args_two_panel` — a def with a Dropdown + an Input, prompt text on the right. `insta::assert_snapshot!`.

- [ ] **Step 4: Run — expect PASS** `cargo test -p qoo-tui` (whole crate now compiles: Tasks 6-9 together). Review + accept new snapshots.

### Task 9: Mouse routing + preview scroll for the new DefArgs

**Files:**
- Modify: `crates/qoo-tui/src/app/mouse.rs` (DefArgs click + wheel), `crates/qoo-tui/src/app/menus.rs` (preview scroll accessors), `crates/qoo-tui/src/app/def_args.rs` (`def_args_click`)
- Test: `app/tests.rs` / `mouse`-related tests

**Interfaces:**
- Consumes: `HitTarget::{FormField, DropdownItem, Button, MenuPreview, Modal}`.
- Produces: `def_args_click` routes clicks onto the `FormState` (focus a field → open its dropdown/combobox; Confirm submits; Cancel/outside dismiss); preview wheel scroll reads/writes `Mode::DefArgs.preview_scroll`.

- [ ] **Step 1:** Update `menus.rs` preview-scroll accessors (`Mode::DefArgs { form }` → `Mode::DefArgs { preview_scroll, .. }`). Update `mouse.rs` arms that match `Mode::DefArgs { .. }` (wheel over preview scrolls; click routing calls `def_args_click`). Rewrite `def_args_click` to drive `FormState` (mirror `form_click`).

- [ ] **Step 2: Run — expect PASS** `cargo test -p qoo-tui`.

- [ ] **Step 3: Commit Tasks 6-9 together**

```bash
command git add crates/qoo-tui/src/app/mode.rs crates/qoo-tui/src/app/def_args.rs crates/qoo-tui/src/app/actions.rs crates/qoo-tui/src/app/mouse.rs crates/qoo-tui/src/app/menus.rs crates/qoo-tui/src/view/def_args.rs crates/qoo-tui/src/view/mod.rs crates/qoo-tui/src/app/menu_flow_tests.rs crates/qoo-tui/src/app/def_pick_tests.rs crates/qoo-tui/src/view/snapshots/
command git commit -m "refactor(tui): def-args form runs on the shared FormState engine"
```

### Task 10: Retire `ArgsForm`

**Files:**
- Modify: `crates/qoo-tui/src/view/args_form.rs` (delete `ArgsForm` + `render_run_form` + inline `render_fields`/`render_buttons`/`render_dropdown`; KEEP `wrap_value_cursor` + `caret_line` and their tests)
- Modify: `crates/qoo-tui/src/view/mod.rs` (drop the `args_form::render_run_form` import if now unused), any `mod`/`use` referencing the deleted items
- Test: full crate build + tests

**Interfaces:**
- Produces: `wrap_value_cursor`/`caret_line` remain `pub(crate)` in `args_form.rs` (or move both, plus their tests, into `multiline_input.rs` and delete `args_form.rs` entirely — pick whichever leaves no dead module).

- [ ] **Step 1:** Delete the retired items. Resolve every resulting unused-import / missing-symbol error. If `args_form.rs` becomes only the two helpers + tests, that is fine; if it would be empty, move the helpers to `multiline_input.rs` and remove the module declaration in `view/mod.rs`.

- [ ] **Step 2: Run — expect PASS** `cargo build && cargo test -p qoo-tui`. Grep for stragglers: `grep -rn "ArgsForm\|render_run_form" crates/qoo-tui/src` → only comments/none.

- [ ] **Step 3: Commit**

```bash
command git add crates/qoo-tui/src/view/args_form.rs crates/qoo-tui/src/view/mod.rs crates/qoo-tui/src/view/multiline_input.rs
command git commit -m "refactor(tui): retire ArgsForm; keep wrap/caret helpers"
```

---

## Phase 3 — Worktree/target combobox (spec Part 2)

### Task 11: TUI ref classifier

**Files:**
- Create: `crates/qoo-tui/src/ref_classify.rs` (+ `mod ref_classify;` in `crates/qoo-tui/src/main.rs` or `lib.rs`)
- Test: same file

**Interfaces:**
- Produces: `pub fn classify_ref(raw: &str) -> Option<String>` — the canonical ref for a typed target, or `None` when the input is not a recognizable ref (caller then treats it as a literal worktree name). Mirrors `packages/core/src/ref.ts::parseRef` for the human-typed cases: bare digits → `pr:N`; `#N` → `pr:N`; a GitHub PR URL → `pr:N`; a full ticket id (`[A-Z][A-Z0-9]*-\d+`) → `ticket:ID`; a Linear issue URL → `ticket:<extracted>`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn classify_ref_maps_typed_targets() {
    assert_eq!(classify_ref("45").as_deref(), Some("pr:45"));
    assert_eq!(classify_ref("#45").as_deref(), Some("pr:45"));
    assert_eq!(
        classify_ref("https://github.com/o/r/pull/45").as_deref(),
        Some("pr:45"),
    );
    assert_eq!(classify_ref("JUS-1756").as_deref(), Some("ticket:JUS-1756"));
    assert_eq!(classify_ref("feature-x").as_deref(), None); // literal worktree name
    assert_eq!(classify_ref("").as_deref(), None);
}
```

- [ ] **Step 2: Run — expect FAIL** (module absent).

- [ ] **Step 3: Implement** (hand-rolled scans, no regex dep — the crate avoids regex; reuse the `worktree_context::extract_ticket` style):

```rust
/// Canonical ref for a human-typed target, else None (treat as a worktree name).
pub fn classify_ref(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() { return None; }
    // #N or bare N → PR
    let digits = t.strip_prefix('#').unwrap_or(t);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        return Some(format!("pr:{digits}"));
    }
    // GitHub PR URL → pr:N   (…/pull/<N>)
    if let Some(n) = github_pr_number(t) {
        return Some(format!("pr:{n}"));
    }
    // Linear issue URL → ticket:<id> (first LETTERS-DIGITS token in the slug)
    if t.contains("linear.app/") {
        if let Some(id) = crate::worktree_context::extract_ticket(t) {
            return Some(format!("ticket:{id}"));
        }
    }
    // Whole-string ticket id → ticket
    if is_full_ticket(t) {
        return Some(format!("ticket:{}", t.to_ascii_uppercase()));
    }
    None
}
```

Implement `github_pr_number` (find `/pull/` then read trailing digits) and `is_full_ticket` (`^[A-Za-z][A-Za-z0-9]*-\d+$`) as small helpers. Reuse `worktree_context::extract_ticket` for the Linear slug.

- [ ] **Step 4: Run — expect PASS** `cargo test -p qoo-tui ref_classify`.

- [ ] **Step 5: Commit**

```bash
command git add crates/qoo-tui/src/ref_classify.rs crates/qoo-tui/src/main.rs
command git commit -m "feat(tui): ref classifier for typed worktree/PR/ticket targets"
```

### Task 12: `FieldKind::Combobox` state

**Files:**
- Modify: `crates/qoo-tui/src/view/form.rs` (FieldKind + FormState combobox ops), `crates/qoo-tui/src/app/form.rs` + `def_args.rs` (key handling)
- Test: `form.rs` tests

**Interfaces:**
- Produces: `FieldKind::Combobox { options: Vec<String> }` and `Field::combobox(label, options, value)`. FormState treats a Combobox as: an editable text value (the typed filter/value) with an openable, filterable option list. New ops: `combobox_filtered() -> Vec<(usize,String)>` (options containing the typed text, case-insensitive) plus a synthetic "use <ref>" row when `classify_ref(value)` is `Some` and matches no option; `open` reuses `dropdown_open`; `dropdown_move`/`dropdown_pick` operate over the FILTERED view; picking an option sets `value` to the option (or to the classified ref for the synthetic row).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn combobox_filters_and_accepts_typed_ref() {
    let mut f = FormState::new("t","OK", vec![Field::combobox(
        "target", vec!["JUS-1756".into(),"acme".into()], "")]);
    f.focus = 0;
    for c in "ac".chars() { f.insert_char(c); }
    let view = f.combobox_filtered();
    assert!(view.iter().any(|(_, s)| s == "acme"));
    // typing a bare number offers a pr ref row even with no matching worktree
    f.set_field_value(0, "45");
    let view = f.combobox_filtered();
    assert!(view.iter().any(|(_, s)| s == "pr:45"));
}
```

(Add a small `set_field_value(i, &str)` test helper on FormState or set `f.fields[0].value` directly + reset caret.)

- [ ] **Step 2: Run — expect FAIL**.

- [ ] **Step 3: Implement**: add the variant + constructor; make `is_text`/editing treat Combobox as text (typeable); add `combobox_filtered` using `crate::ref_classify::classify_ref`; make `open_dropdown`/`dropdown_move`/`dropdown_pick`/`focused_options` aware of Combobox (options = filtered view; pick writes the chosen string, closing the list). `is_dropdown_focused` stays for pure Dropdown; add `is_combobox_focused`.

- [ ] **Step 4:** Key handling: in `form_key` and `def_args_key`, a focused Combobox: printable/Backspace edit the value AND (re)open the list; Up/Down open or move the filtered highlight; Enter picks the highlight (or accepts the typed ref if the synthetic row is highlighted); Esc closes the list only. Left/Right move the caret in the value (it is text).

- [ ] **Step 5: Run — expect PASS** `cargo test -p qoo-tui form`.

- [ ] **Step 6: Commit**

```bash
command git add crates/qoo-tui/src/view/form.rs crates/qoo-tui/src/app/form.rs crates/qoo-tui/src/app/def_args.rs
command git commit -m "feat(tui): Combobox field kind (type-or-pick, accepts typed ref)"
```

### Task 13: Combobox render

**Files:**
- Modify: `crates/qoo-tui/src/view/form.rs` (`render_fields` + `render_open_dropdown` handle Combobox)
- Test: snapshot in `form.rs`

**Interfaces:**
- Produces: a Combobox renders like an Input (typed value + caret) with a right-aligned `▾`; when open, its popup lists the FILTERED options plus the synthetic `use <ref>` row.

- [ ] **Step 1:** In `render_fields`, draw a Combobox as the text-field path (value + caret) with the chevron suffix (reuse the Dropdown chevron code). Return it as the open anchor when focused + open. In `render_open_dropdown`, when the focused field is a Combobox, list `combobox_filtered()` (label the synthetic ref row, e.g. `pr:45   ← use PR #45`).

- [ ] **Step 2: Snapshot** `combobox_open_typed_ref` (value `45`, list showing a worktree + `pr:45`). Accept the new snapshot.

- [ ] **Step 3: Run — expect PASS** `cargo test -p qoo-tui`.

- [ ] **Step 4: Commit**

```bash
command git add crates/qoo-tui/src/view/form.rs crates/qoo-tui/src/view/snapshots/
command git commit -m "feat(tui): render Combobox value + filtered option popup"
```

### Task 14: Seed the combobox + lock on launch-from-worktree

**Files:**
- Modify: `crates/qoo-tui/src/app/def_args.rs` (`form_from_args`), `crates/qoo-tui/src/app/actions.rs` (`run_selected_task_def`), `crates/qoo-tui/src/app/def_args.rs` (`def_pick_activate`)
- Test: `menu_flow_tests.rs`

**Interfaces:**
- Consumes: `App::active_worktree_rows()` (repo worktree names), `is_worktree()` (Task 0).
- Produces: `form_from_args` renders a worktree-typed arg as a Combobox seeded with the repo's worktree names; when `initial_worktree` is `Some` (launched from a worktree row), that arg becomes a READ-ONLY field pre-filled with the worktree name instead.

- [ ] **Step 1: Write the failing tests**: (a) task-pane open of a def with a worktree arg → that field is a `Combobox` whose options include the repo's worktrees; (b) worktree-row open → that field is `readonly` with the launch worktree as its value.

- [ ] **Step 2: Run — expect FAIL**.

- [ ] **Step 3: Implement**: give `form_from_args` (or `open_def_args`) access to the repo's worktree names (pass them in from the call sites, which already have `active_worktree_rows()`); for a `is_worktree()` arg, build `Field::combobox("...", worktrees, default)` normally, or `Field::readonly(name, worktree)` when `initial_worktree` is `Some`. Non-worktree args keep their Input/Textarea/Dropdown/readonly mapping from Task 6.

- [ ] **Step 4: Run — expect PASS**.

- [ ] **Step 5: Commit**

```bash
command git add crates/qoo-tui/src/app/def_args.rs crates/qoo-tui/src/app/actions.rs crates/qoo-tui/src/app/menu_flow_tests.rs
command git commit -m "feat(tui): worktree arg renders as seeded combobox; locked from a worktree launch"
```

### Task 15: Submit sends the resolved `ref`

**Files:**
- Modify: `crates/qoo-tui/src/app/actions.rs` (`run_definition_cmd` signature), `crates/qoo-tui/src/app/def_args.rs` (`submit_def_args`)
- Test: `menu_flow_tests.rs` (assert the emitted `Cmd` params)

**Interfaces:**
- Produces: `run_definition_cmd(repo, name, values, worktree: Option<&str>, target_ref: Option<&str>) -> Cmd` — adds `params["ref"] = target_ref` when `Some` (and does NOT also send `worktree` in that case, so the daemon honors the ref). `submit_def_args` finds the worktree-typed arg's field value, resolves it to a canonical ref (`classify_ref(value)` → its result; an exact existing-worktree name → `worktree:<name>`), and passes it as `target_ref`.

- [ ] **Step 1: Write the failing test**: submitting a def whose worktree field holds `45` emits a `runDefinition` Cmd with `params.ref == "pr:45"`; whose field holds an existing worktree `JUS-1756` emits `params.ref == "worktree:JUS-1756"`.

- [ ] **Step 2: Run — expect FAIL** (signature + resolution absent).

- [ ] **Step 3: Implement**: extend `run_definition_cmd` with `target_ref`; when `Some`, set `params["ref"]` and skip `params["worktree"]`. In `submit_def_args`, after `validate()`, locate the worktree arg index (`args.iter().position(ArgSpec::is_worktree)`); resolve its field value: if it equals an existing worktree name → `worktree:<name>`, else `classify_ref(value)`, else fall back to `worktree:<value>`. Pass through. When there is no worktree arg, keep the old behavior (positional values + `initial_worktree`).

- [ ] **Step 4:** Update every other `run_definition_cmd(...)` call site to pass `None` for the new arg (grep `run_definition_cmd(`).

- [ ] **Step 5: Run — expect PASS** `cargo test -p qoo-tui`.

- [ ] **Step 6: Commit**

```bash
command git add crates/qoo-tui/src/app/actions.rs crates/qoo-tui/src/app/def_args.rs crates/qoo-tui/src/app/menu_flow_tests.rs
command git commit -m "feat(tui): worktree combobox submits a resolved ref (create-or-reuse)"
```

---

## Phase 4 — Definitions repo (`~/workspace/queohoh`, SEPARATE repo/commit)

### Task 16: pr-ready + pr-review definitions

**Files (in `/Users/noootown/workspace/queohoh`):**
- Modify: `platform/tasks/pr-ready/config.yaml`
- Modify: `platform/tasks/pr-review/config.yaml`
- Modify: `platform/tasks/pr-review/prompt.md`

- [ ] **Step 1:** `pr-ready/config.yaml` — mark the `pr` arg worktree-typed:

```yaml
args:
  - name: pr
    type: worktree
    description: worktree/PR to target; inferred when launched from a worktree
  - name: review
    default: full-review
    options: [full-review, bypass-review]
    description: run the self-review round, or skip it
```

- [ ] **Step 2:** `pr-review/config.yaml` — ADD a worktree-typed target arg (it currently has none). Keep `discovery`, `worktree: "pr:{{number}}"`, `dedup`, etc. unchanged:

```yaml
args:
  - name: target
    type: worktree
    description: worktree/PR to review; inferred when launched from a worktree
```

- [ ] **Step 3:** `pr-review/prompt.md` — replace up-front `{{number}}` interpolation with a runtime detect. At the top, before the first `{{number}}` use, add a detect step:

```markdown
Detect the PR for the current branch:

    PR=$(gh pr view --json number -q .number)

Use `$PR` everywhere below (API paths, the review invocation, the feedback file name). If `gh pr view` reports no PR for the branch, stop and say so.
```

Then change the body's `{{number}}` occurrences (lines ~1, 39, 40, 55, 81 in the current file) to `$PR` / `${PR}`. The `discovery` cron path still provides `{{number}}` for scheduled runs, but the runtime detect is authoritative and works for both paths (a discovery-spawned worktree is on the PR branch too).

- [ ] **Step 4: Verify the YAML loads** — from the TUI repo, run the core loader against the definitions dir if a script exists; otherwise a quick node check:

```bash
node -e "const {loadDefinition}=require('/Users/noootown/Downloads/agent247/queohoh.improvement/packages/core/dist/definition.js'); console.log(loadDefinition('/Users/noootown/workspace/queohoh/platform','platform','pr-ready').args)"
```

(Only if `dist` exists after `pnpm -r build`; otherwise rely on the schema unit test from Task 0 and a manual YAML lint.)

- [ ] **Step 5: Commit IN THE DEFINITIONS REPO**

```bash
command git -C /Users/noootown/workspace/queohoh add platform/tasks/pr-ready/config.yaml platform/tasks/pr-review/config.yaml platform/tasks/pr-review/prompt.md
command git -C /Users/noootown/workspace/queohoh commit -m "feat: worktree-typed target arg for pr-ready/pr-review; pr-review detects PR at runtime"
```

---

## Phase 5 — Final verification

### Task 17: Full green gate

- [ ] **Step 1:** `pnpm -r build` — expect PASS (tsc across core/daemon/tui packages).
- [ ] **Step 2:** `pnpm -r test` — expect PASS (core + daemon vitest).
- [ ] **Step 3:** `cargo test` — expect PASS (TUI unit + snapshot).
- [ ] **Step 4:** `command git status --porcelain` — expect ONLY `menu.rs` (the concurrent writer's file) still dirty; every file this plan touched is committed. If anything of ours is uncommitted, commit it by explicit path.

---

## Self-Review (author checklist — completed)

- **Spec coverage:** Part 1 (one engine/two shells) → Tasks 4-10; Part 2 (worktree type + combobox + ref submit) → Tasks 0, 11-15; Part 3 (auto-grow + visual nav) → Tasks 1-3; cross-repo edits → Task 16; verify → Task 17.
- **Placeholder scan:** no TBD/TODO; each code step carries real code or an exact edit target. The one soft spot (Task 2 cached-width mechanism) is resolved explicitly to "`render_*` takes `&mut FormState` and sets the width during layout."
- **Type consistency:** `classify_ref` (Task 11) is consumed by `combobox_filtered` (Task 12) and `submit_def_args` (Task 15); `render_fields`/`render_open_dropdown` (Task 5) are consumed by `render_form` and `render_def_args` (Task 8); `Field::readonly`/`combobox` (Tasks 4/12) are consumed by `form_from_args` (Tasks 6/14); `run_definition_cmd`'s new `target_ref` (Task 15) matches its sole new caller.
- **Ordering note:** Tasks 6-9 are one indivisible refactor (the crate cannot compile between them) and commit together; every other task compiles + tests green on its own.
