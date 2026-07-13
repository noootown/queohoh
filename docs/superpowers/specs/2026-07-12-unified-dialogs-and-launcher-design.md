# Unified Dialogs, Launcher & Form Kit — Design

## Goal

Establish one dialog aesthetic across the whole TUI, using the remove-worktree confirm dialog (commit `1e6715c`) as the standard: rounded border, uniform interior padding, and a bottom `[ Primary ] [ Cancel ]` button row with a clearly focused button. Nothing submits on a stray Enter — the user always fires an explicit button.

On that foundation, redesign the session picker into a **launcher** that also creates worktrees, and introduce a reusable **form kit** (bordered, typed fields) that the launcher's "New session" and "Create Worktree" flows share. Add a project-configurable default model (default `opus`).

This spec captures the full design. Implementation is phased (see the last section); it will land as more than one PR. The form kit is the reusable primitive that all other dialogs (def-pick, action menu, text-input modals) will gradually adopt — but only the surfaces below are in scope here.

## Design principles

- **One dialog shape.** Every dialog is a `Clear`ed, rounded, centered popup with the shared interior padding (`MODAL_PADDING`, h2/v1) and — where it takes an action — a bottom button row. Dim backdrop as today.
- **Border color is a signal, not decoration.** `p.warn` (yellow) border is reserved exclusively for destructive confirmations (`render_confirm`). Every other dialog uses the `p.accent` border.
- **Explicit commit.** A dialog that performs or advances an action shows `[ Primary ] [ Cancel ]`. Enter fires the *focused* button. There is no Back button anywhere — `Esc` cancels and restarts the flow from the beginning.
- **Selection vs focus are different things.** In a list, the accent-barred row is the *selection* (where you are); the solid-highlighted button is the *focus* (what Enter fires). They render differently and can coexist.

## Component 1 — Shared button row

Extract the button-row rendering currently inline in `render_confirm` (`crates/qoo-tui/src/view/modal.rs:169-198`) into a shared helper in `modal.rs`:

```
render_button_row(frame, hit, row: Rect, primary_label: &str, focus: ButtonKind, base: Color)
```

- Draws `[ {primary_label} ]  [ Cancel ]` left-aligned on the 1-high `row`, two-space gap.
- Focused button → `REVERSED | BOLD`; unfocused primary → `base`; unfocused Cancel → dim.
- Registers `Button(ButtonKind::Confirm)` (the primary) and `Button(ButtonKind::Cancel)` hit targets. `ButtonKind` (`crates/qoo-tui/src/hit.rs:6`) is reused unchanged — `Confirm` = "the primary button" regardless of its label ("Remove" / "Next" / "Run" / "Create").
- `base` is the only per-dialog variable: `p.warn` for `render_confirm`, `p.accent` everywhere else.

`render_confirm` is refactored to call this helper; its rendered output and snapshots are unchanged.

## Component 2 — The form kit

A reusable multi-field form rendered inside a single accent-bordered, padded popup. Generalizes the existing `ArgsForm` (`crates/qoo-tui/src/view/args_form.rs`), which already has enum/text/fixed rows, an inline dropdown, and a `[ Run ]`/`[ Cancel ]` button row — but renders fields as flat rows (the clarity complaint). The kit gives each field its own bordered box.

### Field types

An explicit field-type model (replacing today's shape-inference from `ArgSpec`):

- **Input** — single-line text. Rendered as a 1-row bordered box with the value and a caret when focused.
- **Textarea** — multi-line text. Rendered as a **3-row** bordered box (its height alone distinguishes it from an input; no label/tag needed). `Shift+Enter` inserts a newline. Reuses the wrapping/caret logic (`wrap_value_cursor`, `caret_line`) and consolidates `MultilineInput` (`crates/qoo-tui/src/view/multiline_input.rs`) with `ArgsForm`'s duplicate inline editor into one shared single-line/textarea editor.
- **Dropdown** — a closed 1-row bordered box showing the current value with a right-aligned `▾` chevron (the chevron alone signals "dropdown"; no tag). Focusing it and pressing `↑/↓` opens it inline as a bordered option list with an accent-barred highlight; `Enter` picks and collapses; `Esc` collapses without leaving the form.

No type tags and no per-field hint lines — shape carries the meaning (1 row = input, 3 rows = textarea, chevron = dropdown).

### Rendering

- Each field: a small dim label line above a bordered box. **Focused field** → accent border + inset accent ring + **bold accent label**. **Unfocused field** → dim (`p.border`) box + dim label.
- The bottom row is the shared button row (Component 1), `base = p.accent`.
- Sizing: centered popup; width content-driven and clamped like the existing modals. The form does **not** require the two-panel picker shell — it is a single panel (unlike `render_run_form`, which keeps its prompt-preview panel; that surface is out of scope here).

### Focus & navigation

- **Tab / Shift+Tab** cycle focus across every component in order, wrapping: fields top-to-bottom, then `[ Primary ]`, then `[ Cancel ]`.
- **↑/↓** move the selection *within* the focused component: dropdown open → move the highlighted option; textarea → move the caret line, falling through to the next/prev field at the top/bottom edge (as `ArgsForm` does today).
- **Enter** fires the focused button (Primary/Cancel); on a focused-but-closed dropdown, `Enter` (or `↑/↓`) opens it, and `Enter` inside the open dropdown picks.
- **Typing** edits the focused input/textarea.
- **Esc** cancels the whole flow → `Mode::List` (no Back).
- Mouse: field boxes register `FormField(i)`; open-dropdown options register `DropdownItem(i)`; buttons register `Button(..)`. Same hit-target vocabulary that exists today (`crates/qoo-tui/src/hit.rs:50-58`).

### Validation

A field flagged required renders its box border in `p.error` and its label with a red "required" marker when the user tries to fire the Primary button with it empty; focus jumps to the first offending field; the form stays open (mirrors `ArgsForm::validate`, `args_form.rs:308`).

## Component 3 — The launcher (session picker + Create Worktree)

Redesign `render_session_pick` (`crates/qoo-tui/src/view/menu.rs:537`) into the launcher. Opened by **`r` on a worktree row** (unchanged trigger, `keymap.rs:119`, `actions.rs:587`), scoped to that worktree. Layout, top to bottom, inside an accent border with `MODAL_PADDING`:

1. Title border: `{project} · {worktree}` (accent border — **not** yellow).
2. Filter row: `> {query}` with a right-aligned dim `{filtered}/{total}` count (kept — filtering sessions stays useful).
3. **🌱 New session** — the fresh-session entry, icon-prefixed.
4. **＋ Create Worktree…** — new entry, icon-prefixed. Always present (so a new worktree can be spawned from any existing one).
5. A thin rule separating the two icon entries from the resumable sessions.
6. Resumable session rows, each `{clipped label} · {relative_age}`. **The age is always shown** — the label is clipped with `clip()` (`selectors.rs:1002`) to the width left after reserving the ` · {age}` suffix, fixing today's bug where a long label truncates the age away.
7. A thin rule, then the highlighted row's description (fresh-session hint, or the session id + absolute time — as today).
8. Button row: `[ Next ]  [ Cancel ]`, focus defaults to `Next`. ("Next" because the picker advances to the form, it does not itself run anything.)

Remove the `MENU_HINT` bottom-border key legend (`menu.rs:27`).

**Focus model (chosen: "Tab cycles everything"):** Tab/Shift+Tab cycle focus filter → list → `Next` → `Cancel` (wrapping). `↑/↓` move the session selection at any time (a convenience that works regardless of which component holds focus). Typing filters. `Enter` fires the focused button. `Esc` closes. Mouse: row click selects + advances; `[ Next ]`/`[ Cancel ]` clicks route through the button targets.

State: add `focus: ButtonKind` to `Mode::SessionPick` (`crates/qoo-tui/src/app/mode.rs:243`), defaulting to `Confirm` (Next).

## Flows

All three launcher outcomes advance to the form kit (Component 2); the form's Primary button performs the action.

- **🌱 New session** → form with two fields: **model** (dropdown, preselected to the project default) and **prompt** (textarea). Primary button `[ Run ]`. On fire → `enqueue { repo, worktree, prompt, model }` (the existing enqueue path, `crates/qoo-tui/src/app/update.rs:158`, extended with `model`). This replaces the current bare `Mode::AddTask` prompt-only modal for the New-session case.
- **Resume a session** (clicking/selecting an existing session row, then Next) → same two-field form, but carrying `resume_session_id`; enqueue includes `resume_session_id` as today.
- **＋ Create Worktree…** → form with three fields: **branch / worktree name** (input), **model** (dropdown, project default), **prompt** (textarea). Primary button `[ Create ]`. On fire → create the worktree, then enqueue a task in it with the chosen model + prompt. The branch name is validated via `worktree_context::validate_branch` (`worktree_context.rs:57`) before firing, surfacing errors inline in the form (no separate modal).

For the worktree launcher path, these form flows replace the prompt-only `Mode::AddTask` modal (`render_prompt_modal`) and the standalone create-worktree modal (`Mode::CreateWorktree` / `render_create_worktree`); the latter is retired (see Removals). The **Queue pane's** ad-hoc `Mode::AddTask` (opened by `c` on Queue, `actions.rs:187`) is a separate path and is left untouched here — migrating it onto the form kit is a follow-up.

### Create-worktree + enqueue mechanism (implementation decision, resolve in planning)

The daemon already: accepts `worktree`, `ref`, and `model` on `enqueue` (`packages/daemon/src/api.ts:231-278`), and exposes a create-worktree command that returns the new path (`crates/qoo-tui/src/app/actions.rs:542`, `Cmd::CreateWorktree`). Two viable wirings — pick after a quick daemon capability check during planning:

- **(A) Client-sequenced:** TUI dispatches `createWorktree`, and on its reply enqueues into the new worktree. Minimal daemon change; two round-trips + error handling in the TUI.
- **(B) Daemon-combined:** `enqueue` grows the ability to create the worktree when given a new `worktree` + branch `ref`. One round-trip; small daemon change. Recommended if the branch/`ref` params already flow to worktree creation.

## Component 4 — Configurable default model

- **Order (fixed, display):** `fable, opus, sonnet, haiku` — most to least powerful. The model dropdown always lists them in this order. Aliases resolve through the existing table (`packages/core/src/models.ts:11`).
- **Default:** `opus`, overridable per project. Add a tolerant `default_model` scalar to `vars.yaml`, loaded beside `loadProjectModels` / `loadProjectGithubId` and reserved in `loadProjectVars` (`packages/core/src/config.ts:117-160`). Built-in fallback becomes `opus`; change the ad-hoc/enqueue daemon default from `"sonnet"` to the resolved project default (`packages/daemon/src/engine.ts:644`).
- **Preselection:** the launcher form preselects the dropdown to the effective project default. The TUI obtains it from the models settings IPC (`crates/qoo-tui/src/ipc/types.rs:238` `SettingsModels`); extend that payload with the resolved `default_model` if it is not already derivable.
- Task-definition model defaults (`packages/core/src/definition.ts:55`) are out of scope — they govern defined-task runs, not the ad-hoc launcher.

## Removals

- Drop `Create` from the **Worktrees** `pane_buttons` arm (`crates/qoo-tui/src/hit.rs:44`). This removes the `[c]reate` title-bar chip and makes `c` inert on that pane via the shared gate (`keymap.rs:135`). The Queue pane's `Create` (ad-hoc task) is untouched.
- Retire `Mode::CreateWorktree` and `render_create_worktree` (`mode.rs:234`, `modal.rs:204`), and the `A::Create` Worktrees branch (`actions.rs:195`), once the launcher's Create Worktree flow lands.
- Remove `MENU_HINT` from the launcher popup (`menu.rs:27` usage in `render_session_pick`).

## Interaction reference

| Surface | Tab / Shift+Tab | ↑/↓ | Enter | Esc | Type |
| --- | --- | --- | --- | --- | --- |
| Launcher | cycle filter → list → Next → Cancel | move session selection (any time) | fire focused button | close | filter |
| Form | cycle fields → Primary → Cancel | move within focused field (dropdown option / textarea caret) | fire focused button; open/pick dropdown | cancel flow | edit focused input |
| Confirm (unchanged) | toggle Confirm ⇄ Cancel | — | fire focused | dismiss | — |

## Data-model / state changes

- `Mode::SessionPick` gains `focus: ButtonKind`.
- New `Mode` for the form (or generalize `Mode::DefArgs`'s `ArgsForm`): carries the ordered typed fields, per-field focus, dropdown-open state, validation error, and the frozen action context (repo, worktree, resume id, or pending branch name) needed to build the enqueue/create on Primary.
- `ButtonKind` and the `Button`/`FormField`/`DropdownItem`/`Modal`/`MenuItem` hit targets are reused as-is.
- Daemon: `default_model` in `vars.yaml`; resolved default threaded into the enqueue fallback and exposed to the TUI.

## Testing

- Snapshot tests (insta, `TestBackend`) for: the redesigned launcher (icons, rule, always-on age, button row, focus), each form field type and its focused state, the open dropdown, and validation error state. Follow the existing snapshot patterns in `menu.rs`/`args_form.rs` tests.
- Unit tests: label-clipping keeps the age suffix; Tab focus cycling order + wrap; dropdown open/move/pick; button-row hit-target registration and base-color selection; `render_confirm` output unchanged after the button-row extraction.
- Daemon unit test: `default_model` loads from `vars.yaml`, falls back to `opus`, and resolves through the alias table; ad-hoc enqueue picks up the project default.
- Behavioral checks via `/run` (drive the launcher → form → enqueue with a model, and → create worktree + enqueue) once implemented.

## Phasing (likely PR boundaries)

1. Shared `render_button_row` + border-color convention; refactor `render_confirm` (no behavior change).
2. Launcher redesign: padding, always-on ages, icons + Create Worktree entry (entry inert until phase 5), button row + `focus`, Tab model, remove `MENU_HINT`.
3. Daemon: `default_model` in `vars.yaml` + opus fallback + expose to TUI; extend launcher/enqueue with `model`.
4. Form kit: typed bordered fields (input/dropdown/textarea), focus/Tab nav, dropdown open/pick, validation, button row; consolidate `MultilineInput`.
5. Wire flows: New session + resume → form → enqueue(model); Create Worktree → form → create + enqueue; retire `Mode::CreateWorktree`/`render_create_worktree` + `[c]` chip on Worktrees.

## Out of scope

- Migrating def-pick (`render_def_pick`), the action menu (`render_menu`), and other text-input modals onto the form kit / button-row shape — a follow-up once the kit is proven here.
- Task-definition model defaults (`definition.ts`).
- Any change to the two-panel picker preview shell.
