# queohoh — architecture guide

A task-queue orchestrator for coding agents. A TypeScript **daemon** owns all state — it schedules queued Claude tasks across git worktrees, shells out to `git`/`gh` for enrichment, and serves JSON-RPC over a unix socket. A Rust ratatui **TUI** (`qoo`) is a pure client: it renders the daemon's `StateSnapshot` and sends RPCs. An **MCP server** (inside the daemon) is the programmatic enqueue surface used by the `/qoo` skill.

## System data flow

```
/qoo skill ──MCP──▶ ┌────────────────────┐          ┌─────────────────────┐
                    │  daemon (TS)       │◀─JSON-RPC│  qoo-tui (Rust)     │
git / gh ◀──exec────│  queue engine,     │  unix    │  Elm-style client,  │
claude ◀───spawn────│  worktree+PR       │  socket─▶│  renders snapshots  │
                    │  enrichment, runs  │          │  + run files        │
                    └────────────────────┘          └─────────────────────┘
                       runs dir (report/transcript files) ─────▶ read by TUI
```

## Hierarchy

```
packages/core/          Shared TS domain layer (no I/O policy of its own)
  src/resolver.ts         WorktreeInfo — THE daemon→TUI worktree contract;
                          ref resolution (worktree:/pr:/branch refs)
  src/definition.ts       Task-definition model (frontmatter SKILL-style files)
  src/config.ts           Global/workspace config
  src/dedup.ts            Queue dedup keys

packages/daemon/        The single writer of all state
  src/engine.ts           Queue engine + git/PR enrichment (TTL-cached, one
                          `gh pr list` per repo per sweep; failure = null,
                          never throw)
  src/api.ts              JSON-RPC methods + StateSnapshot payload shape
  src/mcp.ts, mcp-tools.ts  MCP server (enqueue_task/chain, list, run defs)
  src/reload.ts           Hot self-restart on rebuild
  src/cli.ts, daemon.ts   Entry points

crates/qoo-tui/         The TUI — strictly layered, one-way data flow
  src/ipc/                Wire layer: types.rs mirrors TS types
                          (serde camelCase, `default` for old daemons),
                          client.rs the socket client
  src/selectors/          PURE derivation layer: snapshot → rows, column
                          layouts (*ColLayout), labels. Split via include!
                          (rows/labels/cols + tests_a/b) so private helpers
                          stay private. No rendering, no I/O.
  src/markup/             Transcript/report styling (sanitize, wrap, fence
                          + markdown paint); tests co-located.
  src/app/                State + update: update.rs consumes Events and
                          returns Cmds; mouse.rs routes clicks via the HitMap;
                          actions.rs/menus.rs the action-menu flows
  src/view/               Pure render functions of App+Computed: panes.rs
                          (list panes), detail/, menu/modal/args_form,
                          theme.rs (Palette)
  src/event.rs            The event loop + Cmd executor (all side effects,
                          spawned off the UI thread)
  src/hit.rs              HitMap: per-frame (Rect, HitTarget) registry;
                          reverse scan = topmost wins

examples/               Example task definitions + reference skill (NOT read by
                        the daemon — copy into a workspace's tasks / skills dir)
scripts/                daemon-ensure.sh: build + (re)start the daemon
```

Dependency direction: `view → selectors → ipc/types`, `app/update → selectors`, side effects only in `event.rs`. TS side: `daemon → core`; the TUI depends on the wire shape only, never on TS code.

## Architectural invariants

- **Daemon is the single writer.** The TUI never touches git, task state, or the filesystem beyond layout/run-file reads. Anything needing `git`/`gh` belongs in `engine.ts`.
- **Runs are detached via a per-run shim.** The daemon spawns a detached `dist/shim.js` (`shim-host.ts`) that owns the `claude -p` child, so a daemon reload/crash never kills a run. A returning daemon re-adopts via the adoption sweep in `engine.ts` (`adoptionDecision`): `result.json` present → finalize; shim pid alive & argv is a shim → adopt; else → `worker died`. `runTask` splits into `startRun` (→`SpawnSpec`) and `finalizeRun`; the shim writes only run-dir files, never task state.
- **Run-dir contract.** `spawn.json` (0600, daemon→shim, unlinked after read), `result.json` (shim→daemon, atomic tmp+rename), `worker.json` (shim pid), `cancelled` (Stop marker, persisted before the signal so a stop surviving a daemon death still settles `cancelled`).
- **Wire compat is one-directional.** Every field a newer daemon adds must be `Option` in `ipc/types.rs` so an old daemon keeps working (container-level `serde(default)`). Never remove/rename wire fields.
- **TUI is Elm-style and single-threaded.** State changes only in `App::update`; side effects only via `Cmd` variants executed in `event.rs::execute` (fire-and-forget, off the UI thread). Views are pure.
- **Mouse = HitMap.** Views register hit rects painter's-order each frame; `hit()` scans in reverse so the topmost wins. Sub-rects (chips, links) must be registered AFTER the row/pane beneath them; overlays own every click while open via early returns in `app/mouse.rs`.
- **Fixed-width column model.** Pane columns (`selectors/`) are fixed widths, pane-gated on data availability, degraded in a documented drop order; one FILL column absorbs slack. Never size a column from row data (rows shifting as data changed was explicit user feedback). A renderer needing a cell's x-offset shares the layout's arithmetic (see `WtColLayout::pr_col_x`), never re-derives it.

## Commands

- `mise run check` — full gate (build, test, typecheck, lint). Individual: `cargo test -p qoo-tui`, `pnpm -r test`, `pnpm -r typecheck`, `pnpm lint:ci` (biome; `pnpm lint` auto-fixes).
- `mise run daemon` / `mise run tui` / `mise run status` — run locally.
- Rust snapshot tests use insta (`crates/qoo-tui/src/snapshots/`); review diffs deliberately (`cargo insta accept`), never blind-accept.

## Conventions

- Dense doc comments carrying the WHY (often user feedback) — match the voice of neighboring code; a field/variant without a rationale comment is incomplete.
- `selectors/` holds pure, unit-testable derivations; keep view files free of business logic.
- No new dependencies without strong cause (clipboard is hand-rolled OSC 52, URL-opening shells to `open`/`xdg-open`, rather than crates).
