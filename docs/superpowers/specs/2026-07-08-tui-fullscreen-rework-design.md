# queohoh TUI — Full-Screen Rework Design

Date: 2026-07-08
Status: approved for planning

## Goal

Replace the inline, content-sized TUI with a full-screen terminal cockpit
(htop/k9s school): alternate screen buffer, project tabs, tmux-style pane
navigation under a `ctrl+s` prefix, and a contextual detail viewer modeled on
agent247's run viewer. The driving use cases:

- **Read a task definition's prompt without leaving the TUI** (no tmux tab +
  nvim round-trip).
- **Run a task against a chosen worktree directly from the TUI** — e.g. select
  `wt-plan-a`, run `autotest`; or select `pr-ready`, type a PR number, and let
  the resolver find/spawn the right worktree.
- **One glanceable screen per project** on a large (4K) terminal.

## Non-goals / follow-ups

- **Worktree setup/teardown script convention** (per-project
  `<workspace>/<project>/scripts/setup-worktree.sh` / `cleanup-worktree.sh`
  overriding the default `wt switch -c`): separate follow-up spec. The default
  `wt`-based `spawnWorktree` already works and this rework does not touch it.
- **Cron scheduling** (slice 2). The CRON panel is deleted; cron-ness surfaces
  as a badge on task definitions (derived from `hasDiscovery` until a
  `schedule` field exists).
- Pane zoom, mouse support, combined "all projects" tab (KISS — add later if
  needed).

## 1. Screen & rendering

- **Alternate screen buffer.** `cli.tsx` writes `\x1b[?1049h` to stdout before
  `render()` and `\x1b[?1049l` after unmount — on normal quit, SIGINT, and
  SIGTERM — so quitting restores the shell exactly (htop behavior). The escape
  handling lives outside the React tree.
- **Full-terminal root.** A `useTerminalSize()` hook returns
  `{columns, rows}` from `process.stdout`, subscribing to `resize` events. The
  root `<Box>` is pinned to exactly that size; children flex to fill. Resize
  re-lays-out.
- **Tiny-terminal guard.** Below 60×15 the app renders a one-line "terminal
  too small" message instead of the layout.

## 2. Layout

```
 1:platform  2:queohoh                              daemon ● · running 1/3
┌ QUEUE ─────────────────────────┐┌ 1:transcript  2:report  3:prompt ────┐
│ > ▶ autotest wt-plan-a   2m14s ││                                       │
│   ○ pr-ready #1832             ││   content of the selected sub-tab,    │
│   ✗ kb-extract           4h ago││   driven by the focused list's        │
├ TASKS ─────────────────────────┤│   selection; fills all space;         │
│   autotest                     ││   scrolls with ↑↓/j/k when focused    │
│   pr-ready                     ││                                       │
│   pr-review          ⏰        ││                                       │
├ WORKTREES ─────────────────────┤│                                       │
│   wt-plan-a     ▶ busy         ││                                       │
│   wt-jus-1234   free           ││                                       │
│   queohoh       YOU            ││                                       │
└────────────────────────────────┘└───────────────────────────────────────┘
 [C-s] prefix · [↑↓/jk] select · [enter] run · [a]dd · [r]etry · [q]uit
```

- **Tab bar (row 1):** one tab per project from daemon config, rendered
  `1:platform 2:queohoh`, active tab highlighted (inverse), agent247-style.
  Right side: daemon connection dot, running count vs `max_concurrent_tasks`.
  If any task's `target.repo` matches no configured project, a synthetic tab
  appears for that repo so nothing is invisible. When the daemon is
  unreachable, the header shows a yellow `daemon unreachable — retrying…`
  banner (panes keep rendering the last snapshot).
- **Left column (~1/3 width): three stacked list panes**, all filtered to the
  active project:
  - **QUEUE** — task executions (live + recent archived), same row format as
    today (glyph, lane, summary, elapsed/status detail). Grows to fill.
  - **TASKS** — task definitions of the active project (from the `definitions`
    RPC). `⏰` badge when the definition has discovery. Content-sized, capped
    at 25% of the column height (scrolls selection into view beyond that).
  - **WORKTREES** — real git worktrees of the project (new snapshot data), one
    row each: name, state (`busy` = running task on its lane, `failed` = most
    recent lane task failed, `free` otherwise). Interactive sessions whose cwd
    is inside the project render a `YOU` row. Content-sized, capped at 25% of
    the column height; QUEUE always keeps at least half the column.
- **Right (~2/3 width): DETAIL pane**, contextual on the most recently focused
  list's selection:
  - Queue run selected → sub-tabs **1:transcript 2:report 3:prompt**.
    Transcript is a height-aware tail (no more hardcoded 25 lines); report is
    the run's `report.md`; prompt is the task's stored prompt.
  - Task definition selected → sub-tabs **1:prompt 2:config** (definition
    `prompt.md` and rendered config fields: args, worktree template, dedup,
    model, timeout, priority, discovery command).
  - Worktree selected → single view: path, branch, state, and the tasks
    (live + archived) targeting its lane.
  - Sub-tab strip renders at the top of the pane; the active sub-tab is
    highlighted. Sub-tab index is remembered per context kind.
- **Footer (last row):** keybindings for the focused pane; action errors
  render in red in the footer slot and clear on the next keypress. While the
  prefix is armed, the footer shows a `PREFIX` indicator.

## 3. Keys & focus

Four focusable panes: queue, tasks, worktrees, detail. The focused pane has an
accent-colored (cyan) border, tmux-style. Focus starts on queue.

**`ctrl+s` = prefix** (captured cleanly in raw mode; user's tmux prefix is
ctrl+a, so no conflict). Armed state disarms after one key or 2 s:

| Prefix + key | Action |
|---|---|
| `←↑↓→` / `hjkl` | move pane focus directionally (queue↕tasks↕worktrees; left column ↔ detail) |
| `1..9` | jump to project tab N |
| `n` / `p` | next / previous project tab |
| anything else | disarm, swallow |

**Unprefixed keys** go to the focused pane. `↑/↓` and `j/k` are equivalent
everywhere.

| Pane | Keys |
|---|---|
| any | `q` quit · `1..9` switch detail sub-tab · `ctrl+s` prefix |
| QUEUE | `↑↓/jk` select · `a` add ad-hoc task (prompt only; repo = active project, ref = `temp`) · `r` retry · `s` skip · `w` assign worktree (needs-input) · `enter` focus detail |
| TASKS | `↑↓/jk` select · `enter` run: if the definition declares args, open the args input first; otherwise run discovery |
| WORKTREES | `↑↓/jk` select · `enter`/`t` open definition picker → optional args input → run targeting this worktree |
| DETAIL | `↑↓/jk` scroll · `g`/`G` top/bottom |

Text inputs (add-prompt, args, worktree-assign) and the definition picker are
modal overlays as today: they capture all keys, `esc` cancels. Bare number
keys only reach the sub-tab switcher when no modal is open.

Per-tab state (selected rows, focused pane) is remembered when switching
project tabs. Selection clamps when rows disappear.

## 4. Daemon / API changes

Small and additive:

- **`StateSnapshot` gains:**
  - `projects: { name: string }[]` — from config, so the TUI learns tab
    names/order from the daemon.
  - `worktrees: Record<string, WorktreeInfo[]>` — project name → real
    worktrees. Sourced from the engine's existing per-tick `worktreeCache`
    via a new `Engine.worktreesByRepo()` accessor (no extra git calls on the
    snapshot path).
- **`runDefinition` gains optional `worktree` param.** When present, the
  created tasks' ref is overridden with `worktree:<name>` (existing
  `parseRef` kind — resolves only to an existing worktree, deterministic).
  Plumbed through `instantiateDefinition` as an optional ref override.
- **New RPC `definition { repo, name }`** returning the full loaded
  definition (prompt + config fields) for the TASKS detail view. The TUI
  fetches lazily on selection and caches per `(repo, name)`; the `definitions`
  list RPC stays summary-only.

## 5. TUI code structure

`App.tsx` (207 lines today) would bloat; split into focused modules:

- `keymap.ts` — pure prefix/focus state machine:
  `handleKey(state, input) → { state, action }`. No React, unit-tested
  exhaustively. Actions are semantic (`move-focus`, `switch-tab`,
  `switch-subtab`, `queue-retry`, …) and `App` executes them.
- `selectors.ts` — pure snapshot → view-model functions: project tab list
  (incl. synthetic orphan tabs), per-project queue rows (reuses
  `buildQueueRows`), worktree rows with state overlay, lane/session matching
  (logic moved out of `RightColumn`, which is deleted).
- `use-terminal-size.ts` — columns/rows + resize.
- Components: `TabBar`, `QueuePane`, `TasksPane`, `WorktreesPane`,
  `DetailPane` (sub-tabs + content views), `Footer`, plus existing
  `TextInput`; a `Pane` wrapper renders the title + focus border. `DetailView`
  and `RightColumn` are retired.
- `run-files.ts` — tail reading becomes height-aware: caller passes the
  number of lines that fit; byte window grows accordingly (cap ~256 KB).
  Detail scrolling holds an offset-from-end into the tail buffer.
- `cli.tsx` — alt-screen enter/restore + signal handling around `render()`.

Data flow stays: `useDaemon` (snapshot push) + 1 s tick for elapsed times +
1 s poll of run files for the selected run only.

## 6. Error handling

- Daemon unreachable → header banner; last snapshot stays rendered; actions
  fail fast with the error in the footer.
- Action RPC errors (retry on non-failed task, unknown repo, …) → red footer
  message, cleared on next key.
- Missing run files / definition load errors → placeholder text in the detail
  pane (`(no transcript yet)`, `(failed to load definition: …)`).
- Malformed definition on disk → `definitions` RPC already skips it; TASKS
  pane shows what loads.

## 7. Testing

- **keymap**: exhaustive pure tests — prefix arm/disarm/timeout, focus
  movement geometry, tab jump/cycle bounds, per-pane action mapping, modal
  suppression of bare numbers.
- **selectors**: project filtering, orphan-repo synthetic tab, worktree state
  overlay (busy/failed/free/YOU), per-tab selection clamping.
- **components** (ink-testing-library): tab bar active highlight, focused
  pane border, detail sub-tab switching per context, footer per-pane keys,
  tiny-terminal guard.
- **daemon api**: snapshot contains `projects` + `worktrees`; `runDefinition`
  honors `worktree` override; `definition` RPC returns prompt/config and
  errors on unknown names.
- **run-files**: height-aware tail with variable line counts.
- Existing TUI tests adapt to the new layout (helpers updated); no coverage
  intent dropped.
