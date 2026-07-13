# Unified Dialogs, Launcher & Form Kit — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify every TUI dialog on the confirm dialog's shape (rounded border, `MODAL_PADDING`, bottom `[ Primary ] [ Cancel ]` row with a focused button), then rebuild the session picker into a launcher that also creates worktrees and feeds a reusable bordered form kit, with a project-configurable default model (`opus`).

**Architecture:** Extract the confirm dialog's button row into a shared `render_button_row` primitive parameterized by base color (Phase 1). Redesign `render_session_pick` into the launcher (Phase 2). Add `default_model` to the TS daemon and thread it to the TUI (Phase 3). Build a single-panel typed form kit generalizing `ArgsForm` (Phase 4). Wire the launcher entries to the form and retire the old create-worktree modal + `[c]` chip (Phase 5).

**Tech Stack:** Rust (`crates/qoo-tui`, ratatui 0.29, insta snapshots), TypeScript (`packages/core` + `packages/daemon`, vitest, zod).

## Global Constraints

- No inline glyph/color literals in components — all glyphs live in `theme.rs`, colors come from `&Palette` (`view/theme.rs:3`, `:84`).
- Yellow (`p.warn`) border is reserved for destructive confirms; all other dialogs use `p.accent`.
- `MODAL_PADDING = { left: 2, right: 2, top: 1, bottom: 1 }` (`view/modal.rs:16`) is the shared interior padding.
- Model dropdown order is fixed: `fable, opus, sonnet, haiku` (most→least powerful). Default `opus`.
- No Back button anywhere; `Esc` cancels/restarts. Explicit button commit — Enter fires the focused button only.
- Markdown docs: one logical line per paragraph/bullet (no hard-wrap at 80 cols).
- Reuse existing `ButtonKind` (`Confirm` = primary, `Cancel`) and existing hit targets (`FormField`/`DropdownItem`/`Button`/`Modal`/`MenuItem`).

---

## Task 0: Commit the approved spec

- [ ] **Step 1: Commit the untracked spec doc + this plan.**

```bash
git add docs/superpowers/specs/2026-07-12-unified-dialogs-and-launcher-design.md docs/superpowers/plans/2026-07-12-unified-dialogs-and-launcher.md
git commit -m "docs: unified dialogs + launcher spec and implementation plan"
```

---

## Phase 1 — Shared button row + border convention

**Files:**
- Modify: `crates/qoo-tui/src/view/modal.rs` (extract `render_button_row`; refactor `render_confirm` to call it).
- Test: `crates/qoo-tui/src/view/modal.rs` `#[cfg(test)] mod confirm_view_tests` (existing) + a new `button_row_view_tests` module.

**Interfaces:**
- Produces: `pub(crate) fn render_button_row(frame: &mut ratatui::Frame, hit: &mut HitMap, row: Rect, primary_label: &str, focus: ButtonKind, base: ratatui::style::Color)` — draws `[ {primary_label} ]  [ Cancel ]` left-aligned on the 1-high `row`, focused button `REVERSED|BOLD`, unfocused primary in `base`, unfocused Cancel dim; registers `Button(Confirm)`/`Button(Cancel)`.
- Consumes: `Palette`, `ButtonKind`, `HitTarget::Button`.

### Task 1.1: Extract `render_button_row`

- [ ] **Step 1: Write the failing test** in a new `mod button_row_view_tests` in `modal.rs`:

```rust
#[cfg(test)]
mod button_row_view_tests {
    use super::*;
    use crate::hit::{ButtonKind, HitMap, HitTarget};
    use ratatui::{backend::TestBackend, layout::Rect, style::Modifier, Terminal};

    fn draw(primary: &str, focus: ButtonKind, base: ratatui::style::Color) -> (String, HitMap, ratatui::buffer::Buffer) {
        let mut term = Terminal::new(TestBackend::new(40, 3)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| {
            let row = Rect { x: 1, y: 1, width: 38, height: 1 };
            render_button_row(f, &mut hit, row, primary, focus, base);
        }).unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..3 { for x in 0..40 { s.push_str(buf[(x, y)].symbol()); } s.push('\n'); }
        (s, hit, buf)
    }

    #[test]
    fn draws_both_buttons_and_registers_targets() {
        let (s, hit, _buf) = draw("Next", ButtonKind::Confirm, Palette::default().accent);
        assert!(s.contains("[ Next ]"));
        assert!(s.contains("[ Cancel ]"));
        let (mut c, mut x) = (false, false);
        for y in 0..3 { for xx in 0..40 { match hit.hit(xx, y) {
            Some(HitTarget::Button(ButtonKind::Confirm)) => c = true,
            Some(HitTarget::Button(ButtonKind::Cancel)) => x = true, _ => {} } } }
        assert!(c && x);
    }

    #[test]
    fn focus_reverses_the_focused_button() {
        let reversed = |focus| {
            let (_s, _h, buf) = draw("Next", focus, Palette::default().accent);
            let mut out = String::new();
            for y in 0..3 { for x in 0..40 {
                if buf[(x, y)].modifier.contains(Modifier::REVERSED) { out.push_str(buf[(x, y)].symbol()); } } }
            out
        };
        assert!(reversed(ButtonKind::Confirm).contains("Next"));
        assert!(!reversed(ButtonKind::Confirm).contains("Cancel"));
        assert!(reversed(ButtonKind::Cancel).contains("Cancel"));
    }
}
```

- [ ] **Step 2: Run to verify it fails.** `cargo test -p qoo-tui button_row_view_tests` → FAIL (`render_button_row` not found).

- [ ] **Step 3: Implement `render_button_row`** in `modal.rs` (lift the body from `render_confirm:169-198`, generalize the color):

```rust
/// Shared bottom button row: `[ {primary_label} ]  [ Cancel ]`, left-aligned on
/// the 1-high `row`. Focused button reversed+bold; unfocused primary in `base`,
/// unfocused Cancel dim. Registers `Button(Confirm)` (primary) / `Button(Cancel)`.
pub(crate) fn render_button_row(
    frame: &mut ratatui::Frame,
    hit: &mut HitMap,
    row: Rect,
    primary_label: &str,
    focus: ButtonKind,
    base: ratatui::style::Color,
) {
    let p = Palette::default();
    let primary_btn = format!("[ {primary_label} ]");
    let cancel_btn = "[ Cancel ]";
    let primary_w = primary_btn.chars().count() as u16;
    let cancel_w = cancel_btn.chars().count() as u16;
    let focused = Style::default().fg(base).add_modifier(Modifier::REVERSED | Modifier::BOLD);
    let primary_style = if matches!(focus, ButtonKind::Confirm) { focused } else { Style::default().fg(base) };
    let cancel_style = if matches!(focus, ButtonKind::Cancel) { focused } else { p.dim_style() };
    let primary_rect = Rect { x: row.x, y: row.y, width: primary_w, height: 1 };
    let cancel_rect = Rect { x: row.x + primary_w + 2, y: row.y, width: cancel_w, height: 1 };
    frame.render_widget(Paragraph::new(Line::from(Span::styled(primary_btn, primary_style))), primary_rect);
    frame.render_widget(Paragraph::new(Line::from(Span::styled(cancel_btn, cancel_style))), cancel_rect);
    hit.push(primary_rect, HitTarget::Button(ButtonKind::Confirm));
    hit.push(cancel_rect, HitTarget::Button(ButtonKind::Cancel));
}
```

- [ ] **Step 4: Refactor `render_confirm`** to call it: replace the button-styling/rect/render/push block (`modal.rs:178-198`) with:

```rust
    let btn_y = inner.y + inner.height.saturating_sub(1);
    render_button_row(frame, hit, Rect { x: inner.x, y: btn_y, width: inner.width, height: 1 },
        confirm_label, focus, p.warn);
```

- [ ] **Step 5: Run tests.** `cargo test -p qoo-tui modal` → PASS (existing `confirm_view_tests` still green, including `focused_button_is_reversed_and_bold` and the `confirm_bulk_remove` snapshot — unchanged output). If the snapshot shifts, review the diff; it must be identical.

- [ ] **Step 6: Commit.**

```bash
git add crates/qoo-tui/src/view/modal.rs
git commit -m "refactor(tui): extract shared render_button_row from confirm dialog"
```

---

## Phase 2 — Launcher redesign

**Files:**
- Modify: `crates/qoo-tui/src/view/menu.rs` (`render_session_pick`: padding, icons, Create Worktree row, always-on age, button row, drop `MENU_HINT`).
- Modify: `crates/qoo-tui/src/app/mode.rs:243` (`Mode::SessionPick` gains `focus: ButtonKind`).
- Modify: `crates/qoo-tui/src/app/actions.rs:599` (construct `SessionPick` with `focus: ButtonKind::Confirm`).
- Modify: `crates/qoo-tui/src/app/menus.rs` (`session_pick_key`: Tab/Shift+Tab focus cycle across filter→list→Next→Cancel; Enter fires focused button; keep ↑/↓ selection + typing filter; route Button clicks).
- Add glyphs to `crates/qoo-tui/src/view/theme.rs`: `GLYPH_NEW_SESSION`, `GLYPH_CREATE_WORKTREE`.
- Test: `menu.rs` session-pick tests (extend) + `menus.rs`/`app` key tests.

**Interfaces:**
- Consumes: `render_button_row` (Phase 1).
- Produces: `Mode::SessionPick { repo, worktree, items, loading, index, query, focus: ButtonKind }`; launcher rows `0 = New session`, `1 = Create Worktree`, `2.. = filtered sessions` (view indices shift by ONE vs today — every consumer of the session-pick index math updates in lockstep).

> **Note — index remap:** today view row 0 = New session, 1.. = sessions. After Phase 2, row 0 = New session, row 1 = Create Worktree, 2.. = sessions. `session_pick_key`'s `chosen` resolution, `reset_session_index`, `session_pick_move` total, `route_session_pick_click`, and `session_pick_wheel` all key off this and must be updated together. Create Worktree stays inert (selecting + Next is a no-op status line) until Phase 5.

### Task 2.1: Add focus to the mode + glyphs

- [ ] **Step 1** Add to `theme.rs` (near other glyphs, ~line 33):

```rust
/// Launcher entry markers (single-width ASCII-ish so alignment holds across terminals).
pub const GLYPH_NEW_SESSION: char = '✦';
pub const GLYPH_CREATE_WORKTREE: char = '＋';
```

- [ ] **Step 2** `mode.rs:243` add field `focus: crate::hit::ButtonKind,` to `Mode::SessionPick` and update the doc comment (mention the button row + Create Worktree row). `actions.rs` `new_task_on_worktree` (~599): add `focus: crate::hit::ButtonKind::Confirm,` to the struct literal.

- [ ] **Step 3** `cargo build -p qoo-tui` → expect compile errors at every `Mode::SessionPick { .. }` match that binds all fields; fix by adding `focus` / `..` as needed. Re-run to green.

- [ ] **Step 4: Commit.** `git commit -am "feat(tui): SessionPick carries button focus + launcher glyphs"`

### Task 2.2: Always-on age + launcher rows + padding + button row

- [ ] **Step 1: Write failing tests** (extend `menu.rs` session-pick tests):

```rust
#[test]
fn session_row_age_survives_long_label() {
    // A very long label must not truncate the "· Ns ago" suffix.
    let items = vec![crate::event::SessionChoice {
        session_id: "s1".into(),
        label: "x".repeat(200), mtime_ms: 0 }];
    let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
    let mut hit = HitMap::default();
    term.draw(|f| render_session_pick(f, &mut hit, "wt", &items, false, 2, "", 5_000)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut s = String::new();
    for y in 0..20 { for x in 0..60 { s.push_str(buf[(x, y)].symbol()); } s.push('\n'); }
    assert!(s.contains("ago"), "age suffix must always render");
    assert!(s.contains('…'), "long label is clipped with an ellipsis");
}

#[test]
fn launcher_has_new_session_and_create_worktree_rows() {
    let (s, hit) = draw_session_pick(80, 20, false, 0, "");
    assert!(s.contains("New session"));
    assert!(s.contains("Create Worktree"));
    assert!(s.contains("[ Next ]"));
    assert!(s.contains("[ Cancel ]"));
    assert!(!s.contains("type to filter"), "MENU_HINT legend removed");
    // Row 0 New session, row 1 Create Worktree, rows 2.. sessions.
    let (mut m0, mut m1, mut m2) = (false, false, false);
    for y in 0..20 { for x in 0..80 { match hit.hit(x, y) {
        Some(HitTarget::MenuItem(0)) => m0 = true,
        Some(HitTarget::MenuItem(1)) => m1 = true,
        Some(HitTarget::MenuItem(2)) => m2 = true, _ => {} } } }
    assert!(m0 && m1 && m2);
}
```

- [ ] **Step 2: Run → FAIL** (`cargo test -p qoo-tui session_row_age_survives_long_label launcher_has_new_session_and_create_worktree_rows`).

- [ ] **Step 3: Rewrite `render_session_pick`** (`menu.rs:537`). Key changes, keeping the single-popup shell but adding `MODAL_PADDING` via a `Block::padding` and the button row on the bottom interior line:
  - Import `render_button_row` and `MODAL_PADDING` from `crate::view::modal`, `GLYPH_NEW_SESSION`/`GLYPH_CREATE_WORKTREE` from theme, `render_button_row`'s `ButtonKind`.
  - Block: keep accent border + title; add `.padding(MODAL_PADDING)`; **remove** `.title_bottom(MENU_HINT)`.
  - Age fix: build each session row as `format!(" {} · {}", clip(&label, avail), age)` where `avail = row_w.saturating_sub(age.chars().count() + 4)` (space + "· " + trailing) — reserve the suffix, clip the label with `selectors::clip`.
  - Rows: row 0 = `{GLYPH_NEW_SESSION}  New session`, row 1 = `{GLYPH_CREATE_WORKTREE}  Create Worktree…`, then a rule, then sessions. `selectable = 2 + session_rows.len()`. Each of rows 0/1/2.. registers `MenuItem(view_ix)`.
  - Reserve the last two interior lines: a blank + the button row. Call `render_button_row(frame, hit, Rect{ x: inner.x, y: inner.y + inner.height - 1, width: inner.width, height: 1 }, "Next", focus, p.accent)`.
  - Height budget: `borders(2) + padding(2) + search(1) + rows + desc + 1 (button row)`.
  - Thread `focus: ButtonKind` into the signature: `render_session_pick(..., now_ms: u64, focus: ButtonKind)`. Update all call sites/tests.

- [ ] **Step 4: Run tests** `cargo test -p qoo-tui menu` → PASS. Update the existing session-pick tests for the new row indices (sessions now start at `MenuItem(2)`), the added `focus` arg, and refresh the `confirm_bulk_remove`/menu snapshots if any legitimately change (`cargo insta review`).

- [ ] **Step 5: Snapshot** the launcher: add `insta::assert_snapshot!("launcher_open", s)` in a test drawing 2 sessions at index 0; `cargo insta accept`.

- [ ] **Step 6: Commit.** `git commit -am "feat(tui): launcher layout — icons, Create Worktree row, always-on age, button row, padding"`

### Task 2.3: Launcher key + mouse handling

- [ ] **Step 1: Write failing tests** in the app tests (mirror `menu_flow_tests.rs`): Tab from list moves focus to Next then Cancel then wraps to filter/list; ↑/↓ still move selection; Enter on Cancel focus closes; Enter on Next focus with Create Worktree selected (index 1) is currently a no-op status (until Phase 5); clicking `Button(Cancel)` closes.

```rust
#[test]
fn launcher_tab_cycles_focus_and_enter_fires() {
    let mut app = /* App with Mode::SessionPick { index:0, focus: Confirm, .. two sessions, loading:false } */;
    // Tab: Confirm -> Cancel
    app.on_event(key(Tab));
    assert!(matches!(app.mode, Mode::SessionPick { focus: ButtonKind::Cancel, .. }));
    // Enter on Cancel closes.
    app.on_event(key(Enter));
    assert!(matches!(app.mode, Mode::List));
}
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Update `session_pick_key`** (`menus.rs:354`): add `Tab`/`BackTab` cycling `focus` across a 4-stop ring conceptually (filter/list/Next/Cancel) — minimally toggle `focus` between `Confirm`/`Cancel` on Tab like the confirm dialog, since filter+list are always live (typing filters, ↑/↓ select). `Enter` fires the focused button: `Confirm` → the existing chosen-row resolution (New session / Create Worktree=no-op status until Phase 5 / resume); `Cancel` → `Mode::List`. Update `chosen`/`session_pick_move` total/`reset_session_index` for the +1 Create-Worktree row offset (New=0, Create=1, sessions 2..). Update `route_session_pick_click` to handle `Button(Confirm)`→fire New/selected, `Button(Cancel)`→close, and `MenuItem(1)`→select Create Worktree (no-op until Phase 5). Update `session_pick_wheel` total.

- [ ] **Step 4: Run tests** `cargo test -p qoo-tui` → PASS.

- [ ] **Step 5: Commit.** `git commit -am "feat(tui): launcher key/mouse — Tab focus, Enter fires button, row remap"`

---

## Phase 3 — Configurable default model (TS daemon → TUI)

**Files:**
- Modify: `packages/core/src/config.ts` (reserve `default_model` in `loadProjectVars`; add `loadProjectDefaultModel`).
- Modify: `packages/daemon/src/engine.ts:644` (adhoc default `model` = resolved project default; built-in fallback `opus`).
- Modify: `packages/daemon/src/api.ts` (expose resolved default model in the models/settings payload the TUI reads).
- Modify: `crates/qoo-tui/src/ipc/types.rs:238` (`SettingsModels` gains `default: String` if not derivable) — read-only preselect source.
- Test: `packages/core/src/config.test.ts` (or nearest), daemon test for enqueue default.

**Interfaces:**
- Produces: `loadProjectDefaultModel(projectDir: string): string | undefined`; effective default resolved to `opus` when unset; surfaced to the TUI as the launcher-form's preselected model.

### Task 3.1: `default_model` in vars.yaml

- [ ] **Step 1: Failing test** in `packages/core/src/config.test.ts`:

```ts
import { loadProjectDefaultModel } from "./config";
it("reads default_model from vars.yaml, tolerant of absence", () => {
  // write tmp <dir>/vars.yaml: "default_model: opus\n"
  expect(loadProjectDefaultModel(dir)).toBe("opus");
  expect(loadProjectDefaultModel(emptyDir)).toBeUndefined();
});
```

- [ ] **Step 2: Run → FAIL** (`pnpm --filter @queohoh/core test config`).

- [ ] **Step 3: Implement** in `config.ts`: reserve `default_model` in `loadProjectVars` (skip it in the scalar loop like `models`/`github_id`, `config.ts:126-127`); add `loadProjectDefaultModel(projectDir)` mirroring `loadProjectGithubId` (tolerant read of the `default_model:` scalar).

- [ ] **Step 4: Run → PASS.**

- [ ] **Step 5: Thread into the daemon** (`engine.ts:644`): compute `defaults.model = loadProjectDefaultModel(projectDir) ?? "opus"` where the per-project `modelTable` is built (`engine.ts:605`). Keep alias resolution via `resolveModel`.

- [ ] **Step 6: Expose to the TUI.** In `api.ts` where the models/settings payload is built, include the resolved default (`default_model`). Add the mirror field to `SettingsModels` in `crates/qoo-tui/src/ipc/types.rs:238`.

- [ ] **Step 7: Tests** `pnpm --filter @queohoh/core test && pnpm --filter @queohoh/daemon test` → PASS; `cargo build -p qoo-tui` → PASS.

- [ ] **Step 8: Commit.** `git commit -am "feat(core): project-configurable default_model (opus fallback), exposed to TUI"`

---

## Phase 4 — Form kit (bordered typed fields)

**Files:**
- Create: `crates/qoo-tui/src/view/form.rs` (the reusable form widget: `FormField` enum, `FormState`, `render_form`).
- Modify: `crates/qoo-tui/src/view/mod.rs` (add `pub mod form;`).
- Modify: `crates/qoo-tui/src/view/multiline_input.rs` (make it the shared single-line/textarea editor; expose caret + wrap helpers or reuse `args_form::{wrap_value_cursor, caret_line}`).
- Modify: `crates/qoo-tui/src/app/mode.rs` (new `Mode::Form { state: FormState, action: FormAction }`).
- Modify: `crates/qoo-tui/src/app/` (new `form.rs` key/click handler; wire into `update.rs` dispatch like `def_args_key`).
- Test: `crates/qoo-tui/src/view/form.rs` unit + snapshot tests.

**Interfaces:**
- Produces:
  - `pub enum FieldKind { Input, Textarea, Dropdown { options: Vec<String> } }`
  - `pub struct Field { pub label: String, pub kind: FieldKind, pub value: String, pub required: bool }`
  - `pub struct FormState { pub title: String, pub fields: Vec<Field>, pub focus: usize /* 0..fields.len() = a field; then Primary, then Cancel */, pub caret: usize, pub dropdown_open: bool, pub dropdown_index: usize, pub error: Option<usize>, pub primary_label: String }`
  - `pub fn render_form(frame, hit, state: &FormState)` — one accent-bordered padded popup; each field a bordered box (focused = accent border + bold label); textarea = 3 rows; dropdown shows `value ▾` closed / inline option list open; bottom `render_button_row`.
  - Focus helpers: `focus_next`/`focus_prev` (wrap over fields → Primary → Cancel), `validate() -> Result<Vec<(String,String)>, usize>`.
- Consumes: `render_button_row`, `MODAL_PADDING`, `wrap_value_cursor`, `caret_line`, `clip`, `pad_clip`, `Palette`, hit targets `FormField`/`DropdownItem`/`Button`.

### Task 4.1: `FormState` model + focus/validation (pure logic, TDD)

- [ ] **Step 1: Failing tests** (`form.rs` tests): `focus_next` cycles field0→field1→…→Primary→Cancel→field0; `validate` flags the first empty required field and returns `Err(index)`, else `Ok(pairs)`; dropdown open/move/pick sets `value` and closes.

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** `FieldKind`/`Field`/`FormState` + methods in `form.rs`. Consolidate text editing by delegating to `MultilineInput` (moving the caret/wrap logic out of `ArgsForm`'s inline copy where practical; if risky, reuse `args_form::wrap_value_cursor`/`caret_line` directly and leave `ArgsForm` untouched — YAGNI on the full consolidation, note it as follow-up).

- [ ] **Step 4: Run → PASS.**

- [ ] **Step 5: Commit.** `git commit -am "feat(tui): form-kit state, focus cycle, validation"`

### Task 4.2: `render_form` + snapshots

- [ ] **Step 1: Failing snapshot/asserts**: input renders 1-row box; textarea renders a ≥3-row box; dropdown closed shows `▾`; focused field box uses accent border; open dropdown lists options with an accent-barred highlight and registers `DropdownItem(i)`; button row present.

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement `render_form`** (bordered boxes per field via nested `Block`s; focused field `border_style(true)` + bold accent label; unfocused dim; textarea box height 3; dropdown closed = value + right-aligned `▾`; open = option list block below the value with `p.selection()` highlight; bottom line via `render_button_row(.., state.primary_label, focus_as_buttonkind, p.accent)`). Register `FormField(i)`, `DropdownItem(i)`, `Modal`.

- [ ] **Step 4: Run → PASS**; `cargo insta accept` the form snapshots (input/textarea/dropdown-open/focused).

- [ ] **Step 5: Commit.** `git commit -am "feat(tui): render bordered typed form fields + inline dropdown"`

### Task 4.3: Form key/click handler

- [ ] **Step 1: Failing tests**: Tab cycles focus incl. buttons; typing edits focused input/textarea; Shift+Enter newline in textarea; on focused dropdown ↑/↓ open+move, Enter picks; Enter on Primary validates→(Ok fires `action`, Err keeps open with `error`); Esc → `Mode::List`; clicks route via `FormField`/`DropdownItem`/`Button`.

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** `form_key`/`form_click` in `app/form.rs`, dispatched from `update.rs` when `matches!(self.mode, Mode::Form { .. })` (mirror the `def_args_key` arm at `update.rs:215-226`). The Primary action is deferred to Phase 5 via a `FormAction` enum passed opaque here (Phase 4 can stub `FormAction::Noop`).

- [ ] **Step 4: Run → PASS.**

- [ ] **Step 5: Commit.** `git commit -am "feat(tui): form-kit key + mouse handling"`

---

## Phase 5 — Wire flows + retire the old create-worktree modal

**Files:**
- Modify: `crates/qoo-tui/src/app/menus.rs` (`session_pick_key`: New session / resume → open `Mode::Form` (model + prompt); Create Worktree → open `Mode::Form` (name + model + prompt)).
- Modify: `crates/qoo-tui/src/app/mode.rs` (`FormAction::{NewSession{repo,worktree,resume_session_id:Option<String>}, CreateWorktree{repo}}`).
- Modify: `crates/qoo-tui/src/app/form.rs` (Primary fires the enqueue / create+enqueue via `dispatch_rpc` with `model`).
- Modify: `crates/qoo-tui/src/app/update.rs:158` (enqueue params include `model`).
- Modify: `crates/qoo-tui/src/hit.rs:44` (drop `Create` from Worktrees `pane_buttons`).
- Remove: `Mode::CreateWorktree` (`mode.rs:234`), `render_create_worktree` (`modal.rs:204`), `A::Create` Worktrees branch (`actions.rs:195`), `create_worktree_key` dispatch (`update.rs:229`). Keep the `Cmd::CreateWorktree` command builder (reused by the form's Create flow).
- Test: app flow tests + a daemon check for create+enqueue wiring.

**Interfaces:**
- Consumes: `FormState`/`render_form`/`form_key` (Phase 4), `default_model` (Phase 3), `Cmd::CreateWorktree` (`actions.rs:542`).

> **Create-worktree + enqueue mechanism — resolve here (spec §"mechanism"):** check whether `enqueue` already creates a worktree from a new `worktree`+`ref` (`packages/daemon/src/api.ts:231-278`). If yes → single enqueue with `worktree=<name>`, `ref=<branch>`, `model`, `prompt` (Option B). If no → client-sequences: dispatch `Cmd::CreateWorktree{repo,name}`, and on its reply enqueue into the new worktree (Option A). Pick per the daemon capability; implement one, note which in the commit.

### Task 5.1: New-session + resume flow → form → enqueue(model)

- [ ] **Step 1: Failing test**: selecting New session (index 0) + firing Next opens `Mode::Form` with two fields (model dropdown defaulted to project default, prompt textarea); firing the form Primary dispatches `enqueue` with a `model` key and the prompt.

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement**: in `session_pick_key`, replace the direct `Mode::AddTask` transition for New session / resume with a `Mode::Form { state: model+prompt, action: FormAction::NewSession { repo, worktree, resume_session_id } }`. In `form.rs`, `FormAction::NewSession` builds the enqueue params (`prompt`, `repo`, `worktree`, optional `resume_session_id`, `model`) and calls `dispatch_rpc("enqueue task", "enqueue", params, ..)`. Extend the enqueue params object (`update.rs:158`) semantics accordingly (or build in `form.rs`).

- [ ] **Step 4: Run → PASS.**

- [ ] **Step 5: Commit.** `git commit -am "feat(tui): New session + resume flow through the form with model select"`

### Task 5.2: Create Worktree flow → form → create + enqueue

- [ ] **Step 1: Failing test**: selecting Create Worktree (index 1) + Next opens `Mode::Form` with three fields (name/model/prompt); invalid branch name keeps the form open with `error`; valid fires create+enqueue.

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** `FormAction::CreateWorktree { repo }`: validate the name field via `worktree_context::validate_branch`; on valid, dispatch per the chosen mechanism (A or B above) with `model`+`prompt`.

- [ ] **Step 4: Run → PASS.**

- [ ] **Step 5: Commit.** `git commit -am "feat(tui): Create Worktree flow — name+model+prompt, create then enqueue"`

### Task 5.3: Retire the old create-worktree modal + `[c]` chip

- [ ] **Step 1** Remove `Create` from `pane_buttons(PaneId::Worktrees)` (`hit.rs:44`) → keep Queue's. Delete `Mode::CreateWorktree`, `render_create_worktree`, the `A::Create` Worktrees arm, and the `create_worktree_key` dispatch + handler. Keep `Cmd::CreateWorktree`.

- [ ] **Step 2** Fix all resulting compile errors (match arms, tests referencing `CreateWorktree`/`render_create_worktree`, `panes.rs` chip tests referencing `PaneButton::Create` on Worktrees).

- [ ] **Step 3: Run full suite** `cargo test -p qoo-tui` → PASS; `cargo build` (workspace) → PASS.

- [ ] **Step 4: Commit.** `git commit -am "refactor(tui): retire standalone create-worktree modal + [c] chip (folded into launcher)"`

### Task 5.4: End-to-end verification

- [ ] **Step 1** `cargo test` (workspace) + `cargo clippy -p qoo-tui` → clean. `pnpm -r test` for touched TS packages → green.
- [ ] **Step 2** `cargo insta review` — accept only intended snapshot changes.
- [ ] **Step 3** Manual/scripted drive (per `/verify` or `cargo run` against a scratch config): `r` on a worktree → launcher → New session → form (opus preselected) → enqueue carries `model`; Create Worktree → form → worktree created + task enqueued; confirm no `[c]` chip remains.
- [ ] **Step 4: Commit** any snapshot/verify fixups. Ensure the tree is clean.

---

## Self-Review notes

- Spec coverage: Phase 1 = button row + border convention; Phase 2 = launcher (icons/age/buttons/focus/no hint); Phase 3 = default_model; Phase 4 = form kit (typed bordered fields, dropdown, nav, validation); Phase 5 = flows + removals. All spec sections map to a phase.
- Index remap (Create Worktree = row 1) is the highest-risk mechanical change — Task 2.2/2.3 update every consumer in lockstep; the launcher tests pin the new indices.
- The create+enqueue mechanism is a single explicit decision in Task 5.1's note, resolved against the daemon at implementation time.
- The `MultilineInput`/`ArgsForm` consolidation is intentionally optional (Task 4.1) to avoid destabilizing the existing def-args form; full consolidation is a follow-up.
