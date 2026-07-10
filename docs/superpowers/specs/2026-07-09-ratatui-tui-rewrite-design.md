# qoo TUI rewrite in Rust/ratatui — Design

**Date:** 2026-07-09
**Status:** Approved design, pre-implementation

## Motivation

The current TUI (`packages/tui`, Ink/React, ~3.8k lines) has three problems:

1. Poor mouse support — no clicking, no scrollbars; only wheel scrolling on the focused pane.
2. Render latency — even after the "1 render per keypress" optimization, Ink's React reconciliation adds visible lag.
3. Dated look and widget vocabulary — no real dropdowns, styled inputs, or dialogs.

## Scope decisions (settled with user)

- **TUI-only rewrite.** The daemon, core, and worker stay TypeScript. The Unix-socket NDJSON JSON-RPC protocol is the boundary and does not change.
- **Cargo crate in this repo** at `crates/qoo-tui/`, binary `qoo-tui`. Built locally (`cargo build --release`); a mise task wires launching. No npm packaging, no cross-compilation.
- **Parallel until parity.** The Ink TUI stays the daily driver; `packages/tui` is deleted only after the Rust binary covers everything and the user has lived on it.
- **Full redesign following ratatui community conventions**, reusing ecosystem widgets rather than hand-rolling. Design references: herdr (compute→hit-test pipeline), gitui (popup stack, key-hint footer), taskwarrior-tui (task-manager conventions), atuin (list + preview).
- **Fully clickable UI**: rows, tabs, buttons, menu items, sub-tabs, form fields, draggable scrollbars, wheel everywhere. Mouse capture always-on (Shift/Option for native text selection — same tradeoff as today and Claude Code).
- **All current features reach parity** (full inventory below).
- **Direct keys replace the ctrl+s prefix** — vim-style navigation, single-key actions, `?` help overlay.

## Architecture (Approach A — approved)

Skeleton follows the official `ratatui/async-template` shape: single-threaded UI state, async I/O at the edges, everything funneled through one event channel.

### Crate layout

```
crates/qoo-tui/
  src/
    main.rs          # terminal setup, alt-screen + mouse capture, panic/exit guards
    event.rs         # Event enum + tokio event loop
    app.rs           # App state, Mode enum, update() dispatch
    ipc/
      client.rs      # NDJSON JSON-RPC over UnixStream (call + subscribe)
      types.rs       # serde mirrors of StateSnapshot/TaskInstance/ArgSpec/…
    view/
      mod.rs         # compute pass → ViewState (layout rects + hit map), render pass
      theme.rs       # Palette struct (colors/glyphs in one place)
      <component>.rs # tabbar, queue, tasks, worktrees, detail, footer, modals…
    hit.rs           # HitTarget enum + hit-test registry
    selectors.rs     # view-model derivation (port of selectors.ts)
    keymap.rs        # key → AppAction mapping (port of keymap.ts, new bindings)
    markup.rs        # line markup → styled spans (port of markup.ts)
    runfiles.rs      # transcript/report tailing
    heal.rs          # stale-daemon detect + restart
```

### Dependencies (deliberately lean)

`ratatui` · `crossterm` (feature `event-stream`) · `tokio` (rt-multi-thread, net, io-util, sync, time, macros, process) · `serde` + `serde_json` · `tui-input` (form text fields) · `tui-popup` + `tui-confirm-dialog` (dialogs) · `throbber-widgets-tui` (running-task spinner) · `insta` (dev, snapshot tests).

Action menus and pickers are plain `List`-in-popup — no menu crate. `tui-scrollview`, `rat-widget`, and `ratatui-interact` deliberately omitted; each can be adopted later without restructuring. herdr is AGPL — patterns only, never code.

### Event & state model

**`Event` enum** — everything enters through one `tokio::mpsc` channel:
`Key(KeyEvent)` / `Mouse(MouseEvent)` / `Resize` / `Snapshot(StateSnapshot)` / `Disconnected` / `Tick` / `RunFiles { task_id, files }` / `ActionResult { label, error }`.

**Loop:** `tokio::select!` over crossterm's `EventStream` and the mpsc receiver. `app.update(event)` returns whether state changed; **draw only on change** — preserves "1 render per keypress, zero idle renders". The 1s `Tick` is armed only while the active project has a running task (ports the `activeHasRunningRef` throttle).

**Mutations are fire-and-forget tokio tasks**: open a short-lived socket connection, call the RPC, send `ActionResult` back into the channel (error → status line). UI never blocks. Preserved semantics: `createWorktree` 10-minute timeout; `runDefinition` treats client timeout as success (discovery can outlive it; the push subscription re-syncs); 5s default RPC timeout.

**`Mode` enum** ports the existing state machine 1:1: `List`, `AddTask`, `DefPick`, `DefArgs`, `ActionMenu`, `ConfirmRemove`, `ConfirmBulkRemove`, `CreateWorktree`, `WorktreeInput`, `Search`. Per-tab UI state ports as `HashMap<String, TabUiState>` (`focus`, `lastListPane`, per-pane `{cursor, anchor}` selections, per-pane search strings, per-detail-kind sub-tab, scrollOffset). `mode`, status line, and input buffers are global.

### Daemon IPC client

One tokio task owns the persistent subscription connection: `UnixStream` + `BufReader::lines()`, each line serde-decoded. Push frames `{event:"state",data}` → `Event::Snapshot`; socket close → `Event::Disconnected` + 2s reconnect loop (connect → `subscribe` → `state`), matching `use-daemon.ts`.

Ported deliberately:

- **Snapshot dedup** — skip byte-identical raw lines before deserializing, except the first snapshot after a (re)connect, which always commits.
- **Lenient decode** (`normalizeSnapshot` equivalent) — every `StateSnapshot` field carries `#[serde(default)]` so an older daemon's snapshot never crashes the client. `buildId: Option<String>`; `None` means stale (drives self-heal).

### Wire protocol (unchanged, for reference)

- NDJSON over Unix domain socket; one JSON object per newline; partial-line buffering.
- Request `{id, method, params?}` (monotonic int id) · response `{id, result}` or `{id, error: "<msg>"}` · push `{event:"state", data}` (only to subscribers).
- Methods used by the TUI: `state`, `subscribe`, `enqueue`, `retry`, `skip`, `setWorktree`, `removeWorktree`, `createWorktree`, `runDefinition`, `definition`, `definitions`, `shutdown`, `ping`.
- Key shapes (mirror in `ipc/types.rs` with lenient defaults):
  - `StateSnapshot { tasks, archivedRecent, sessions, running, maxConcurrent, projects, worktrees: Record<repo, WorktreeInfo[]>, mainSessions: Record<lane, sessionId>, buildId? }`
  - `TaskInstance { id, status: queued|needs-input|running|done|failed, definition, item, itemKey, target{repo,ref,worktree}, priority, created, source, ephemeralWorktree, error, session: fresh|main, resumeSessionId, model, prompt }`
  - `ArgSpec { name, default?, options?, description? }` — no `type` field: `options` present ⇒ dropdown; `default` absent ⇒ required; fixed-ness is a UI-side concept.
  - `DefinitionSummary { repo, name, scope: project|global, args, hasDiscovery }`; `TaskDefinition` adds `discovery`, `dedup`, `worktree`, `model`, `timeoutMs`, `priority`, `prompt`.

## Mouse system

Ratatui is immediate-mode with no retained widget tree, so hit-testing is ours (herdr's pattern):

**Compute pass → `ViewState`.** Before drawing, compute the frame's geometry once; every clickable element registers `(Rect, HitTarget)`:

```
Tab(index) · PaneBody(pane) · Row(pane, row_index) · SubTab(index)
MenuItem(index) · FormField(index) · Button(kind) · ScrollbarThumb(pane) · ScrollbarTrack(pane)
```

The render pass draws from the same rects — geometry computed once, hit-testing and drawing can never disagree.

**Routing rules:**

- **Click** resolves top-down: modals register last, test first; a click inside a modal never leaks through; a click outside a modal dismisses it (same as `esc`).
- Click tab → switch project. Click row → focus pane + move cursor. **Click the already-selected row → open its action menu.** Click menu item / form field / button / sub-tab → activate or focus. Shift+click → extend selection.
- **Wheel** scrolls the pane **under the cursor** (upgrade from focused-pane-only): lists move the cursor, detail scrolls content.
- **Drag on scrollbar thumb/track** maps pointer row proportionally to scroll offset. Every overflowing pane shows a built-in `Scrollbar`.

Mouse capture always-on; terminal restore (mouse off, alt-screen exit) via a `Drop` guard **and** a panic hook, so no exit path leaves the terminal in mouse-reporting mode.

## Screen design

Layout keeps the proven skeleton — tab bar / left column (QUEUE, TASKS, WORKTREES panes) / detail pane / footer — with modernized chrome:

- Rounded borders; focused pane gets the accent color; a `Palette` theme struct (Catppuccin-style default) centralizes all colors.
- Running tasks get an **animated throbber** replacing static `▶`; the rest of the glyph language survives: `○ ? ✓ ✗`, worktree dots (green free / yellow busy-or-you / red failed), `⛓` main-session task, `◆` main-session worktree, `⏰` discovery, `(g)` global def, `[N]` queued count.
- Visible scrollbars on every overflowing pane.
- Detail sub-tabs are clickable chips; transcript stays bottom-anchored live-tail with `g`/`G` jumps.
- Modals: title bars, dim backdrop, clickable `[ Run ] [ Cancel ]` buttons.
- Args form: bordered `tui-input` fields; enum args render as **dropdowns** (click or Enter opens an option-list popup; ←/→ cycling kept for keyboard speed); fixed fields dimmed/read-only, skipped by focus traversal, still submitted positionally.
- `?` opens a help overlay listing the full keymap.
- Too-small guard (< 60×15), connection-lost banner (`daemon unreachable — retrying…` + last snapshot stays rendered), and `running N/M` header counter all survive.

### Keymap (direct keys; ctrl+s prefix retired)

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | cycle focus between panes (incl. detail) |
| `1`–`9` | switch project tab |
| `[` / `]` | previous / next project tab |
| `j/k` / arrows | move cursor · `J/K` / `shift+↑↓` extend selection |
| `Enter` | action menu for selection (lists) — universal "act on this" |
| `a` | action menu (alias) |
| `c` | create (queue pane → adhoc task; worktrees pane → worktree) |
| `/` | filter focused pane |
| `esc` | clear range → clear filter → close modal (staged, as today) |
| `g` / `G` | top / bottom (detail scroll or list jump) |
| `?` | help overlay |
| `q` | quit |

Inside text inputs plain keys type; `Tab` is next-field inside forms. Globals (`1-9`, `[`/`]`) don't collide with form typing.

## Run files, self-heal, tmux

**Run files** (`runfiles.rs`) — port of the tuned behavior: 120ms selection-settle debounce; read `report.md` fully and `transcript.md` tail-only (seek to a 64–256KB window from EOF, keep last `detail_height × 4` lines); 1s re-poll while the selected task runs; content-identical reads dropped before becoming events; reads tagged with task id and discarded if the selection moved.

**Self-heal** (`heal.rs`) — same decision table as `heal.ts`, ported as a pure function: compare snapshot `buildId` against max mtime of the daemon dist's `.js` files. Stale + idle → `shutdown` RPC (refuses when busy; SIGTERM-via-pidfile fallback for old daemons) → poll `ping` until quiet (~5s bound) → respawn detached `node <daemon-cli> daemon` via `tokio::process::Command`. Stale + busy → defer with status line. One heal attempt per disk build id. Missing `buildId` = stale.

**tmux** — `$TMUX` gates the open-window action (disabled with reason `not inside tmux`); `tmux new-window -c <path>`.

## Parity contract

The full behavioral inventory of the Ink TUI is the contract; the TS test suite (~5,000 lines) is the executable spec. Beyond what's specified above, these behaviors must survive:

- **Layout math:** left column 34% width; `listPaneH = max(4, floor(bodyHeight/4))`, queue pane gets the remainder; row capacity = paneHeight − 3; detail height = bodyHeight − 4; cursor-centered row windowing.
- **Queue rows:** `{glyph} {⛓?}{repo:branch} {prompt summary ≤60} {detail}` where detail = elapsed `⏱ 5m03s` (running), `#N in lane` (queued), error text (failed/needs-input), `done`/`archived`; archived rows dim, last 10 shown after live rows.
- **Tasks rows:** `name (argSummary) ⏰?` with `name=default` for defaulted args. **Worktree rows:** colored dot + name + `◆` + `[N]`, interactive-session rows appended.
- **Pane titles carry state:** `QUEUE · 3 selected`, `QUEUE /foo`, `QUEUE /foo█` while searching.
- **Detail contexts:** queue→run (transcript/report/prompt sub-tabs), tasks→definition (prompt/config), worktrees→worktree (info + lane tasks), empty. Sub-tab and scroll reset on selection change; only run/transcript is bottom-anchored; scroll direction inverts there so `k` = older.
- **Markup:** headings bold, rules dim, `**bold**`, `` `code` `` cyan, URLs blue; one terminal row per line (truncate) so scroll math holds.
- **Action menus:** per-pane single-target menus and bulk menus; disabled rows dimmed with `— reason`; bulk targets frozen at menu-open time; bulk execution sequential with per-item error capture (`reran 3, 2 failed: <first error>`); bulk eligibility rules (rerun: failed/needs-input; skip: +done; bulk-run: zero-arg defs only; bulk-remove: non-busy worktrees, via confirm listing ≤8 names + `…and N more`).
- **Worktree-context auto-fill:** args named `source`/`branch` get the worktree's branch, `ticket` gets the first `[A-Za-z]+-\d+` token uppercased (omitted key when absent so def defaults win). Explicit-target runs pass these as fixed; ambient runs overlay branch candidates as editable dropdown options (excluding main/master/session rows) + prefill from the selected worktree.
- **Args form validation:** first required-and-empty field blocks submit, gets focus, shows ` required`; values submitted positionally.
- **Branch validation** on create-worktree: non-empty, no whitespace/`..`/leading `-` or `/`/trailing `.lock`, printable ASCII.
- **Definitions cache:** fetched lazily per tab, invalidated after any run (discovery may change dedup state); full defs cached by repo/name.
- **Status line:** red in footer, cleared on next list-mode keypress. Footer priority: searching hint > status line > selection count hint > per-pane hints.
- **Enqueue prompt** modal targets the selected worktree lane (or adhoc) and carries the session mode (`fresh`/`main`) chosen from the action menu.

## Testing strategy

- **Pure-logic ports get mirrored unit tests** (same inputs/expected outputs as the TS tests): selectors, keymap decisions, detail windowing/anchoring, format strings, markup spans, heal decisions, branch validation, worktree-context.
- **Rendering:** `ratatui::TestBackend` + `insta` buffer snapshots at fixed sizes with fixture snapshots — panes, modals, args form, too-small guard, disconnected header.
- **Hit-testing:** given a fixture frame's ViewState, assert click coordinates resolve to the right `HitTarget`, including modal-over-pane occlusion.
- **IPC:** fake NDJSON server on a temp socket; assert subscribe/push/dedup/reconnect and lenient decode of old-daemon snapshots.

## Milestones

1. **M1 — Read-only viewer:** IPC client, tabs, three panes, detail with transcript tail, full mouse (click/wheel/scrollbar drag), filters, theme, help overlay.
2. **M2 — Actions:** action menus (single + bulk), retry/skip/enqueue, confirmations, status line.
3. **M3 — Forms & worktrees:** args form with dropdowns, def-picker, worktree create/remove/assign, worktree-context auto-fill, squash-merge flow, tmux open.
4. **M4 — Self-heal + polish + cutover:** heal, edge cases; flip the `qoo` launcher to the Rust binary; delete `packages/tui` after a comfortable bake period.

Risk note: the only genuinely new engineering (not a port) is the hit-testing layer and dropdown popups — small, bounded, landing in M1/M3.

## Out of scope

- Any daemon/protocol change (including new RPC methods).
- npm distribution or cross-platform builds.
- PTY embedding / terminal panes (herdr's genre — not qoo's).
- Themes beyond the single default palette (the `Palette` struct makes this easy later).
