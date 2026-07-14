# Task-run input consolidation — one form engine, worktree target type, Claude-Code textarea

## Problem

Launching a task has two divergent input surfaces that should be one:

- The **run dialog** (`Mode::Form` / `FormState` / `view::form::render_form`) — a centered modal used by New session and Create worktree. Bordered field boxes, model dropdown, prompt textarea. Looks good.
- The **def-args form** (`Mode::DefArgs` / `ArgsForm` / `view::args_form::render_run_form`) — a two-panel picker (arg inputs left, `prompt.md` preview right) with its own hand-rolled multiline editor. Looks worse and duplicates the editor, focus model, and dropdown logic.

Three problems fall out of this split:

1. **Duplicated, inconsistent components.** `ArgsForm` reimplements what `FormState` already does. The def-args rows render less cleanly than the run dialog's field boxes.
2. **PR/worktree args are free text.** `pr-ready` and `pr-review` take a `pr`/`number` string, but a PR is ~1:1 with a worktree. Launching from a worktree should infer the target; launching from the task pane should let you pick a worktree. There is no arg type that tells the TUI to render a worktree picker.
3. **The textarea is not Claude-Code-like.** It is fixed at 3 rows (no auto-grow), and up/down move by *logical* (`\n`-split) line — so a single long line that soft-wraps across visual rows cannot be traversed with the arrow keys.

## Non-goals

- No change to the daemon's ref → worktree pipeline. It already does everything needed (verified below).
- No new target type on `autofix` — it keeps `worktree: auto` (derives the ref from its `situation` text). Only `pr-ready`/`pr-review` gain the worktree type.
- No change to cron/discovery-driven runs of `pr-review` (the non-TUI path keeps passing `number` and `worktree: pr:{{number}}`).

## Background — the targeting chain already supports the hard case

The genuinely tricky scenario is **reviewing someone else's PR whose worktree does not exist locally yet**: the run must create a worktree first. This needs no backend work.

- `runDefinition` (`packages/daemon/src/api.ts:465`) accepts two distinct params: `params.worktree` (an existing worktree *name* → `worktree:<name>`) and **`params.ref`** (a full ref string used verbatim as `refOverride`). `params.ref` beats the definition's own `worktree:` template.
- `refOverride` flows through `instantiateDefinition` → `resolveRef` (`packages/core/src/instantiate.ts:117`, override always wins) → `resolveTarget` (`packages/core/src/resolver.ts:57`).
- For a `pr:N` ref, `resolveTarget` calls `prBranch` (`resolver-io.ts:59` → `gh pr view N --json headRefName`), then reuses a worktree on that branch or **spawns one on the PR's head branch** (`resolver.ts:81-110`, `spawnWorktree` at `resolver-io.ts:74` fetches `origin/<branch>` and `wt switch`es to it).

So a freshly-spawned `pr:N` worktree lands on the PR's head branch, which makes runtime PR-detection (below) valid whether the worktree pre-existed or was just created.

## Design

Three coupled parts over one shared field engine.

### Part 1 — One form engine, two render shells

Extract the field kit so its fields render into any `Rect`, and drive both surfaces from it.

- **Shared engine:** `FormState` + `Field` + `FieldKind` + the `form_key` handler become the single source of truth for field state, focus, editing, dropdown, and validation.
- **Shell A — centered modal:** `render_form` (run dialog: New session, Create worktree). Look unchanged.
- **Shell B — two-panel picker:** def-args. Left panel renders the shared fields into its rect; right panel keeps the `prompt.md` preview and its wheel-scroll. A small `render_fields(frame, hit, state, rect)` primitive is factored out of `render_form` so both shells call it; `render_form` becomes "draw the modal chrome, then `render_fields` into the inner rect," and the picker's left panel calls `render_fields` into the left rect.
- **Mode change:** `Mode::DefArgs` carries a `FormState` plus the preview scroll, instead of `ArgsForm`. `ArgsForm`, `view::args_form::render_run_form`, and the bespoke editor retire. The shared `wrap_value_cursor` / `caret_line` helpers stay (move to a neutral module if `args_form` is deleted wholesale; otherwise keep the file for just those helpers).
- **Key handling** is the existing app-wide standard in `form_key`: Tab/Shift-Tab move between fields and the button row; arrow keys are inner-navigation only and never step focus; Shift+Enter inserts a newline; only the Primary button submits. `def_args_key` collapses into this (the picker adds only the preview wheel-scroll, which is mouse, not keys).

Fixed/derived context rows (today `ArgsForm.fixed`, e.g. `source`/`branch`/`ticket` prefilled from the launch worktree) become read-only fields in `FormState` — a `Field` flavor that renders its value but is skipped by focus and excluded from editing. This preserves the existing "launched-from-a-worktree prefills the context args" behavior.

### Part 2 — Worktree/target arg type (combobox)

**Definition schema.** `ArgSpec` gains an optional discriminator:

```ts
export interface ArgSpec {
  name: string;
  type?: "worktree";        // NEW — absent = free-text; a non-empty `options` still means enum
  default?: string;
  options?: string[];
  description?: string;
}
```

`type: "worktree"` and `options` are mutually exclusive (validation error if both). This is the "dedicated type so the TUI knows how to render this."

**New field kind.** `FieldKind::Combobox { options }` — a typeable dropdown:

- Options are seeded with the repo's existing worktree names (the TUI already has these via `active_worktree_rows()` / `worktreesByRepo`).
- Typing filters the option list (same filter helper the def-picker uses).
- Arrow keys move the highlight within the filtered list while it is open; closed, up/down open it (consistent with the existing dropdown behavior). Enter picks the highlight.
- A typed value that matches no existing worktree but **parses as a ref** (`45`, `#45`, a GitHub PR URL, `JUS-123`, a Linear URL) is accepted as a new target and shown as a synthetic "use <canonical-ref>" row (mirrors the approved mockup: typing `45` offers `pr:45`).
- The parse reuses the existing `parseRef` grammar conceptually; the TUI side gets a small Rust ref-classifier (bare number → `pr:N`, `#N` → `pr:N`, PR URL → `pr:N`, ticket-shaped or Linear URL → `ticket:ID`, otherwise treat as a literal worktree name). Canonicalization on the daemon side (`canonicalizeRef`) remains authoritative; the TUI classifier only needs to produce a string the daemon will accept.

**Launch context.**

- **From a worktree row:** the worktree field is pre-filled with that worktree and **locked** (rendered, read-only, focus-skipped) — you see the target, you do not re-pick. This is the "the worktree infers the PR" behavior.
- **From the task pane:** the combobox is interactive (pick-or-type).

**Submit.** The worktree field resolves to a canonical ref: `worktree:<name>` for an existing pick, `pr:<n>` / `ticket:<id>` for a typed target. The TUI sends it as the **`ref`** param on `runDefinition` (the run form's submit path grows a `ref` alongside the existing positional `args`). The worktree arg still occupies its declared positional slot (so `args` indices line up with `def.args`); its positional value is irrelevant to the prompt (see below) and may carry the ref string or empty. `refOverride` from `ref` supersedes the definition's `worktree:` template. No daemon change.

**Prompt/definition edits (in `~/workspace/queohoh`, a separate repo).**

- `pr-ready/config.yaml`: mark the `pr` arg `type: worktree`. Its `prompt.md` already detects-or-creates the PR from the branch and never interpolates `{{pr}}`, so no prompt edit.
- `pr-review/config.yaml`: **add** a `type: worktree` target arg (it has none today — it is discovery/cron-only). This is the arg a human fills when launching from the task pane; the cron/discovery path never reads declared args (discovery items supply the interpolation vars directly), so adding it does not affect scheduled runs. `prompt.md` drops up-front `{{number}}` interpolation in favor of a one-line runtime detect (`gh pr view --json number -q .number` from the branch), then uses the detected number in its API calls. The `discovery` block and `worktree: pr:{{number}}` template are untouched, so the scheduled path still works.

### Part 3 — Claude-Code textarea

Both shells inherit this because it lives in the shared field engine.

- **Auto-grow.** A `Textarea` field's content height is `clamp(lines_needed, 3, available_rows)` where `lines_needed` is the wrapped visual-row count and `available_rows` is bounded by the container (the modal grows downward up to the screen; the picker's left panel is bounded by the pane). Past the cap the field scrolls internally — the render already windows to the caret row (`start = cur_row.saturating_sub(rows-1)` in `render_form`), so this is a height computation change, not a new scroll mechanism.
- **Visual-line navigation.** Up/down move by **visual (wrapped) row**, not logical `\n` line. `MultilineInput::move_up`/`move_down` become width-aware: given the field's content width, they step to the caret's visual row above/below, preserving visual column. The `FormState` caches the last-rendered content width per text field (updated each render) so the key handler can pass it in; before the first render a sensible default width is used (movement self-corrects on the next frame).
- Applies to every textarea: the run-dialog prompt, `autofix`'s `situation`, and any free-text arg.

## Affected files (TUI + core/daemon — `queohoh.improvement`)

- `crates/qoo-tui/src/view/form.rs` — `FieldKind::Combobox`, read-only field flavor, `render_fields` extraction, auto-grow height, combobox render + open-list.
- `crates/qoo-tui/src/view/multiline_input.rs` — width-aware `move_up`/`move_down`; keep logical-line helpers where still needed.
- `crates/qoo-tui/src/view/args_form.rs` — retire `ArgsForm`/`render_run_form`; preserve `wrap_value_cursor`/`caret_line` (here or relocated).
- `crates/qoo-tui/src/app/form.rs` — combobox key handling (filter/open/pick/accept-ref), cached content width, submit sends `ref`.
- `crates/qoo-tui/src/app/def_args.rs` — `Mode::DefArgs` backed by `FormState`; `def_args_key` folds into `form_key` semantics; preview wheel-scroll retained.
- `crates/qoo-tui/src/app/mode.rs` — `Mode::DefArgs { form: FormState, preview_scroll }`.
- `crates/qoo-tui/src/app/actions.rs`, `menus.rs`, `mouse.rs` — build the `FormState` for a def (seed combobox options from worktrees, lock the target when launched from a worktree row), route clicks, preview scroll.
- `crates/qoo-tui/src/ipc/types.rs` — `ArgSpec.type` field (deserialize).
- `packages/core/src/definition.ts` — `ArgSpec.type` + mutual-exclusion validation.
- (No change to `api.ts`, `instantiate.ts`, `resolver.ts` — the `ref` path already exists.)

## Affected files (definitions — `~/workspace/queohoh`, separate repo)

- `platform/tasks/pr-ready/config.yaml` — `type: worktree` on `pr`.
- `platform/tasks/pr-review/config.yaml` — add a `type: worktree` target arg (none exists today; discovery/cron path unaffected).
- `platform/tasks/pr-review/prompt.md` — runtime PR-number detect; drop up-front `{{number}}` interpolation.

## Testing

Unit (Rust):

- Combobox: typing filters options; a typed bare number / `#N` / PR URL / ticket produces the right canonical ref; an existing-name pick produces `worktree:<name>`.
- Launch-from-worktree: the target field is present, locked, focus-skipped, and pre-filled with the worktree.
- Visual-line nav: up/down cross wrapped rows of a single long logical line (the case that fails today); still clamp at first/last visual row.
- Auto-grow: height grows 3 → N with content, caps at available rows, then windows to the caret.
- Submit: a worktree field emits `ref` in the `runDefinition` params; positional `args` still align with `def.args`.

Unit (TS): `ArgSpec.type` parses; `type` + `options` together rejects.

Snapshot (Rust): two-panel def-args with shared fields + preview; centered run dialog unchanged; combobox open with a typed-ref synthetic row.

Regression: existing `form.rs`, `def_args`, menu-flow, and form snapshot tests updated to the unified engine; daemon ref pipeline already covered, no new tests there.

## Sequencing

1. **Part 3 first** (textarea auto-grow + visual nav) inside the shared engine — self-contained, no schema or backend touch.
2. **Part 1** — factor `render_fields`, back `Mode::DefArgs` with `FormState`, retire `ArgsForm`.
3. **Part 2** — `Combobox` kind + `ArgSpec.type` + submit-sends-`ref`, then the `~/workspace/queohoh` definition/prompt edits.

Each part builds on the previous; Part 2's new field kind lands in the already-unified engine.
