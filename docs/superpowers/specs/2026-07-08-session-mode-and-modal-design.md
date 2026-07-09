# queohoh — Main-Session Tasks + Floating Action Modal Design

Date: 2026-07-08
Status: approved for planning

## Goal

Two features that make the TUI the primary "dogfood loop" surface:

1. **Main-session tasks.** Each lane (repo:worktree) can have a *main* Claude
   session — a persistent conversation the user keeps feeding findings into.
   From the WORKTREES pane, `f` queues a task in a fresh session, `m` queues a
   task that continues the lane's main session, so consecutive fixes share
   context instead of starting over.
2. **Floating action modal.** All TUI actions (text inputs, the definition
   picker, future CTAs) render in a centered floating modal — nvim/telescope
   style — with a title stating the action and ESC to close. Nothing docks to
   the bottom of the screen anymore.

## Non-goals

- No `M`/reset key for the main pointer (user decision — keep it simple; a
  rotten chain is cured by queueing a context-reset prompt into it or by
  hand-editing the state file, since files are truth).
- No retry-with-failed-session-context (falls out of this machinery later).
- No re-add of the old `a` ad-hoc-on-temp-worktree flow; `a` is deleted. If
  missed, it returns later as `f` from the queue pane.
- No cross-lane session sharing (a session is anchored to its worktree cwd).

## 1. Session mode — data model

- `TaskInstance` gains `session: "fresh" | "main"` (default `"fresh"`;
  serialized in task frontmatter as `session`). Backward compatible: missing
  field parses as `fresh`.
- `NewTaskInput` gains optional `session`.
- **Main-pointer store**: daemon-owned JSON file `<state>/main-sessions.json`
  mapping lane key (`repo:worktree`) → Claude session id. Single writer (the
  daemon), read/written through a small `MainSessionStore` class in core
  (same atomic tmp+rename pattern as `SessionRegistry`). Corrupt/missing file
  → empty map.
- Exposed in `StateSnapshot` as `mainSessions: Record<string, string>` so the
  TUI can badge lanes and rows.

## 2. Session mode — execution semantics

- **Resolution at spawn time, not enqueue time.** When the worker starts a
  task with `session: "main"`, it reads the lane's current pointer:
  - pointer exists → pass `--resume <sessionId>` to the claude invocation
    (`ExecuteClaudeOptions` gains `resumeSessionId?: string` → args
    `["--resume", id]` before `claudeArgs`).
  - no pointer → run fresh; this run *establishes* the chain.
- **Pointer advance:** after a `main`-run finishes (done OR failed), if the
  run captured a `sessionId`, the daemon sets the lane pointer to it.
  Headless `--resume` forks a new session id per run, so the pointer must
  chase the latest id. Fresh runs never touch the pointer. Runs that captured
  no session id (spawn failure) leave the pointer unchanged.
- Lane serialization (existing scheduler invariant: one task per lane) makes
  the read→run→advance sequence race-free.
- If `--resume` fails because the session is gone (pruned by Claude), the run
  fails with claude's error like any other run failure; the pointer is then
  overwritten by the next successful establishment (a failed run with a new
  captured session id advances the pointer, which self-heals the chain).

## 3. Session mode — API & TUI

- `enqueue` RPC accepts optional `session` and optional `worktree`; when
  `worktree` is present the task is created with ref `worktree:<name>`
  (mirrors runDefinition's override; the old `a`-flow default ref `temp`
  remains for enqueues without a worktree, e.g. MCP).
- WORKTREES pane keys (rows of kind `"worktree"` only):
  - `f` → modal prompt input titled `New task — fresh session — <lane>` →
    enqueue `{ prompt, repo, worktree, session: "fresh" }`.
  - `m` → modal prompt input titled `New task — main session — <lane>` →
    enqueue `{ ..., session: "main" }`.
- QUEUE pane: `a` is removed (from keymap, footer, and App). Queue rows for
  `session: "main"` tasks show a `⛓` marker after the glyph.
- WORKTREES rows whose lane has a main pointer show `◆` before the state.
- Footer hints updated: worktrees pane shows
  `[f]resh task · [m]ain task · [enter] run def`; queue pane drops `[a]dd`.

## 4. Floating action modal

One `Modal` component replaces every bottom-docked action UI.

- **Positioning:** rendered as the LAST child of the root (relative) Box with
  `position="absolute"`, centered via computed offsets from terminal size:
  width `min(72, columns - 8)`, height = content-driven; offsets
  `floor((columns - w) / 2)` / `floor((rows - h) / 2)` (implemented as
  margins on an absolute full-screen wrapper). Ink paints later children over
  earlier ones, which yields the z-on-top effect.
- **Opacity:** Ink boxes have no background fill; the modal must pad every
  interior content line to its full inner width (spaces) so underlying text
  never bleeds through. A `Modal` helper renders children lines pre-padded;
  TextInput and picker rows inside it are wrapped accordingly.
- **Spike-first:** the implementation plan front-loads a tiny spike proving
  absolute-position painting over background content in ink-testing-library
  and a real terminal. Fallback if compositing misbehaves: hide the body
  while a modal is open and render the modal centered in the empty body
  (title/ESC semantics identical, no background visible). The public
  `Modal` API is the same either way.
- **Chrome:** round border, bold inverse title line, content area, dim hint
  line (`esc close` for inputs; `esc/q close · ↑↓ select · enter confirm`
  for pickers).
- **Close semantics:** ESC always closes any modal. `q` additionally closes
  non-text-input modals (pickers/CTAs). Inside text inputs `q` types a "q".
- **Adopters (all current modals):**
  - add-task prompt input (`f`/`m`) — replaces bottom TextInput
  - worktree-assign input (`w` on queue) — modal titled `Assign worktree — task <id>`
  - definition picker (`enter`/`t` on worktrees; `enter` on tasks with the
    def-pick flow) — modal titled `Run task definition — <lane>`
  - definition args input — modal titled `<def name> args (<arg names>)`
- While any modal is open, list-mode keymap does not run (existing pattern);
  bare digits/q never leak.

## 5. Files touched (orientation)

- core: `task.ts` (schema + field), `store.ts` (NewTaskInput), new
  `main-sessions.ts` (MainSessionStore), `runner.ts` (resumeSessionId),
  `worker.ts` (resolve pointer, advance after run), `index.ts` (exports).
- daemon: `engine.ts` (wire MainSessionStore into worker deps),
  `api.ts` (snapshot `mainSessions`, enqueue `session`/`worktree` params),
  `daemon.ts` (construct store).
- tui: `keymap.ts` (f/m on worktrees, drop a), `actions.ts` (enqueue gains
  worktree/session params), `App.tsx` (modal state, enqueue wiring), new
  `components/Modal.tsx`, `components/TextInput.tsx` (render inside modal),
  `format.ts`/`selectors.ts` (⛓ and ◆ markers), `Footer.tsx` (hints).

## 6. Error handling

- Corrupt `main-sessions.json` → treated as empty; next advance rewrites it.
- `m` on a lane while another `m` task is queued: both store mode only;
  serialization + spawn-time resolution keeps the chain linear.
- Enqueue with `worktree` that disappears before resolution → existing
  resolver `needs-input` path ("worktree not found").
- Modal open + terminal resized below minimum → tiny-terminal guard wins
  (modal state preserved; reappears when size recovers).

## 7. Testing

- core: MainSessionStore round-trip/corrupt-file; task frontmatter
  serialize/parse with `session` (+ default); runner passes `--resume` iff
  resumeSessionId; worker resolves pointer at spawn, advances on done/failed
  with sessionId, leaves untouched on fresh/spawn-failure.
- daemon: snapshot `mainSessions`; enqueue with session+worktree creates
  correct ref/field.
- tui: keymap f/m mapping (worktrees-pane only) and `a` removal; Modal
  render (title, hint, padded opacity, centered offsets math as pure
  function); App flows — f/m open modal with correct title and enqueue with
  correct params; picker inside modal closes on esc AND q; text modal does
  NOT close on q; queue ⛓ and worktree ◆ markers.
