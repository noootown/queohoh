# qoo-tui — agent guide

A ratatui terminal UI for the queohoh daemon. This doc records the load-bearing DECISIONS, the module hierarchy, and the CONVENTIONS you must follow. It does not restate what the code shows — read it before a first change, then trust the code. Verify any claim here against the source if you're about to depend on it.

## Architecture (Elm-ish, one-way data flow)

The event loop lives in `event::run_event_loop`: **draw → block on the next `Event` (or a `Tick` when `App::wants_tick()`) → stamp `App::now_ms` → `App::update(event) -> Update { dirty, cmds } → execute(cmd)` for each cmd → draw again.**

- **State + update:** `app::App` holds ALL state; `App::update` is the single, synchronous, pure-ish transition (no I/O). It never performs side effects — it returns `Cmd`s.
- **Cmd/Event boundary:** side effects (RPCs, file reads, tmux, clipboard) are `Cmd`s run by `event::execute` on tokio tasks; results come back as `Event`s through a channel. Keep `update` free of I/O so it stays unit-testable — this is why "read a file / call the daemon" is a `Cmd`, never an inline call in a handler.
- **Selectors → view-model:** `selectors.rs` turns the raw `StateSnapshot` into ready-to-render row structs (`QueueRow`/`WorktreeRow`/def rows) — sorting, filtering, glyphs, labels. Pure functions; the view does no data logic.
- **View renders + registers a HitMap:** `view::render(app) -> HitMap`. The view holds only `&App`. It draws widgets AND records clickable regions into a `HitMap`; the next mouse `Event` is routed against `app.hit` (stored after each draw). Rendering owns geometry; mouse routing reads geometry back.
- **Render-feedback Cells (interior mutability):** the view needs post-layout facts the model can't know pre-render (true scroll ceiling, wrapped-line count, detail geometry). It writes them into `App`'s `Cell`/`RefCell` fields (`detail_max_scroll`, `detail_wrapped_len`, `menu_preview_max_scroll`, `detail_geom`) through `&App`. **Freshness argument (relied upon everywhere):** every state change redraws BEFORE the next event is read, so these cells always match what is on screen — scroll/drag math reads them instead of recomputing the wrap. Do not read them expecting pre-first-paint validity.

## Module hierarchy (who owns which concern)

```
src/
  main.rs            terminal setup/teardown (raw mode, alt screen, panic hook)
  event.rs           Event + Cmd enums; run_event_loop; execute (all I/O); seq_summary
  app/               THE App model, split by concern — all add methods to one `App`:
    mod.rs             App struct + all state fields; construction, layout persistence,
                       self-heal wiring, dispatch_rpc, definition reconciliation, core
                       state accessors (ui/set_cursor/active_repo/…); WHEEL_STEP, consts
    mode.rs            Mode enum + pure state/UI types (PaneId, ListPane, TabUiState,
                       DetailKind, Selection, DragKind, …). Re-exported via `pub use`.
    update.rs          update / update_event — the top-level Event dispatch
    actions.rs         apply_action (list-mode AppAction) + `r`/`x` queue verbs
                       (requeue_selected/cancel_selected) + Cmd builders
    menus.rs           action-menu & bulk-menu: open, key nav/filter, wheel, execute
    def_args.rs        def-picker, run-form, create-worktree input handling
    mouse.rs           on_mouse routing, drags (scrollbar/divider), DETAIL text
                       selection + scroll, current_detail_context
    *_tests.rs / tests.rs   test modules (children of `app`, super = app)
  selectors.rs       StateSnapshot → view-model rows; status glyphs; sort/filter;
                     column layout math; pad_clip; time/age labels
  markup.rs          LineCtx pipeline for the DETAIL pane (see below)
  view/
    mod.rs             compute() (App → Computed view-model) + top render + overlay dispatch
    panes.rs           the three list panes; title-bar chips; build_header degradation
    detail.rs          DETAIL pane; content_for (per-line text + LineCtx)
    menu.rs            lazyvim-style picker widget (two-panel, preview, filter)
    modal.rs           confirm + text-input modals (ConfirmRemove/BulkRemove/Cancel, …)
    help/settings/footer/tabbar.rs   overlays + chrome
    theme.rs           Palette (colors), GLYPH_* consts, glyph_style, chip labels, titles
  hit.rs             HitMap, HitTarget, PaneButton, pane_buttons() (chip source of truth)
  ipc/               types.rs (wire structs, serde), client.rs (socket RPC)
  runfiles.rs        reads the daemon run-store files directly (transcript/report/data.json)
  keymap.rs          pure KeyEvent → AppAction for Mode::List
  layout.rs          per-project pane layout persistence model
  heal.rs / worktree_context.rs   self-heal daemon restart; branch-name validation
```

## Decisions (why, in one line each)

- **App split into `app/` submodules, one `App` type:** each file adds an `impl App` block; cross-module methods are `pub(super)` (= `pub(in app)`), never widened to `pub`. Private struct fields stay accessible because submodules are descendants of `app`.
- **Pure `update`, effects as `Cmd`:** enables the whole update/action layer to be tested with plain `App::update(event)` and no daemon.
- **Render-feedback Cells over recomputation:** the view already computed the wrap; recomputing it in the model would drift. See the freshness argument above.
- **`#[serde(other)] Unknown` on every wire enum:** old-daemon tolerance is a HARD rule — a daemon that predates a field/variant must not break the TUI (see IPC).

## Single sources of truth (touch the source, not the copies)

- **`hit::pane_buttons(pane)`** — the chip set per pane. The renderer draws exactly these chips AND the keymap gates a pane-action key on the pane showing that chip. Change this array and both the chips and the key-gating retune together; there is no separate list to keep in sync.
- **`theme::Palette` + `MOCHA`/`THEME`** — the only place colors are named. Components take `&Palette`; NEVER write a raw `Color` in a component. Semantic rule: `info` (teal) is TIMESTAMPS ONLY; all other metadata is `meta` (lavender).
- **`theme::GLYPH_*` consts** — no inline glyph literals in components. `glyph_style` colors a row by matching the glyph CHAR, so two statuses must use DISTINCT glyphs to get distinct colors. `selectors::status_glyph` keeps literal copies that MUST stay char-for-char in sync with the consts (a test asserts it).
- **`app::WHEEL_STEP`** — lines per wheel tick, shared by the DETAIL pane and every picker/run-form preview so they can't drift.

## Layout invariants

- **Fixed-width metadata columns are never sized from row data** — a column's width is a constant or derived from the pane width, never from the longest value, so a new/changed row can't shift columns. Each pane has ONE flexible FILL column (queue: prompt; worktrees: last-task) that absorbs slack.
- **Cell-width discipline:** measure with `Span::width()` / unicode-width (`pad_clip`), never assume 1 char == 1 cell. A mis-sized glyph breaks the border fill, right-alignment, and hit rects.
- **Double-width emoji only in the title row** (pane titles), where a wide glyph can't break column alignment. Status/marker glyphs must be single-width — verify via the snapshot multi-width annotation (below).
- **Chip strip degradation ladder** (`build_header`): drop chips from the RIGHT (collapse first) as width shrinks; the `·` scope divider shows only while both scope groups survive. `*_ROW_SCOPED` consts mark the row-scoped/pane-scoped split and MUST match the `pane_buttons` ordering.

## The LineCtx markup pipeline (DETAIL pane)

`content_for` returns `(lines, ctxs, placeholder)` — one `LineCtx` per logical line. Flow: `fence_states` (or per-line ctx) → `wrap_lines` (reflow to width, carrying each segment's ctx) → `style_transcript_line` (dispatch by ctx). Rules:
- Markdown/transcript text flows through `fence_states`.
- `LineCtx::Config { key_col }` styles aligned `key   value` rows (key column vs value). **A wrapped Config continuation carries `key_col: 0`** ("value starts at column 0") so its start is never re-colored as a key — regression-critical.
- `LineCtx::LaneTask { glyph, is_def, age, selected }` renders queue-style rows in the worktree info tab (self-truncating; right-pins the age).

## IPC contract

- `ipc/types.rs` structs are `#[serde(rename_all = "camelCase", default)]` with container `default`; enums carry `#[serde(other)] Unknown`. Additions are optional and old-daemon-safe — **never** make a field required or remove the `Unknown` fallback. New status/field arrives only as a rendering concern.
- The TUI reads the daemon's run-store files DIRECTLY via `runfiles.rs` (`<runs_dir>/<task_id>/{transcript.md, report.md, data.json}`) — `data.json` yields `session_id` + `resolved_worktree`. Missing/blank → `None` (files appear lazily). This is deliberately not an RPC.
- **Adding a `TaskStatus` variant is a SWEEP, not a one-liner.** Each predicate below decides a behavior independently; a new variant that isn't handled at each site silently does the wrong thing (a `cancelled` row once landed in the ACTIVE section because only `queue_row_finished` was missed). Handle every one deliberately — don't lean on a `_`/`Unknown` fallthrough: `selectors.rs`: `status_glyph` (+ keep `theme::GLYPH_*` in sync), `status_active_rank` (active vs finished ordering), `queue_row_finished` (the ACTIVE/FINISHED SECTION split), `queue_rows` detail text, `worktree_state`/`last_finished_on_lane` (lane last-task) and the lane counts (`queued_on_lane`/`running_elapsed_on_lane`/ `queue_pane_summary` — all Queued/Running-specific). The `x`-cancel eligibility in `app/actions.rs` (`status_kebab`, `cancel_method`) is a further site.

## Interaction conventions

- **Keys mirror chips per pane** (via `pane_buttons`): a verb key is inert unless the focused pane shows its chip. A title-bar chip CLICK behaves exactly like pressing its key with that pane focused (route through the same `AppAction`).
- **Arrows drive the LEFT pane cursor** (shift extends); **`j`/`k`/`h`/`l` drive the DETAIL pane** (row cursor or scroll / sub-tab). Don't rebind arrows to detail or vim keys to the list.
- **Destructive / irreversible actions confirm first** via a `Confirm*` Mode + centered modal (ConfirmRemove/ConfirmBulkRemove/ConfirmCancel). The frozen targets/calls are captured at open time so a mid-dialog snapshot can't retarget. Default focus = confirm (Enter/y fire; n/q/Esc dismiss).
- **HitMap painter's order:** overlays/modals and chips are registered LAST so they win the reverse hit scan over the pane bodies/dividers beneath them. Register a new clickable region after whatever it must sit on top of.

## Testing & snapshots

- Unit-test the model via `App::update(event)` (no daemon). Keymap logic is pure (`keymap::list_mode_action`) — test it directly.
- View tests use `insta` snapshots of a `TestBackend` buffer. **The multi-width annotation (`Hidden by multi-width symbols: [...]`) is the alignment canary** — a glyph that renders wider than assumed shows up there; treat its appearance on a data column as a bug, not a snapshot to accept.
- **Review every snapshot diff before accepting.** Accept only after confirming the change is intended (`cargo insta review`, or `INSTA_UPDATE`); a full-frame diff that touches more than the line you changed means a real regression.
- Baseline gate: `cargo test -p qoo-tui` green (count must not shrink) and `cargo clippy -p qoo-tui` clean.
