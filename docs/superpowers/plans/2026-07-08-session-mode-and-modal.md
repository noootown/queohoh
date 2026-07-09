# Main-Session Tasks + Floating Action Modal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Per-lane "main" Claude sessions (`f`/`m` task enqueueing from the worktrees pane) and a centered floating modal that hosts every TUI action.

**Architecture:** Core gains a `session` field on tasks, a `MainSessionStore` (lane → session id, atomic JSON file), `--resume` support in the runner, and spawn-time pointer resolution + post-run advance in the worker. The daemon wires the store, exposes `mainSessions` in the snapshot, and extends `enqueue`. The TUI adds a `Modal` component (Ink absolute positioning, spike-verified) that all action flows render through, and `f`/`m` keys on the worktrees pane.

**Tech Stack:** TypeScript ESM strict, Ink 6.8 + React 19, vitest + ink-testing-library, Biome (tabs, double quotes).

**Spec:** `docs/superpowers/specs/2026-07-08-session-mode-and-modal-design.md` — read it first; its sections are binding.

## Global Constraints

- Build order: core → daemon → tui (`pnpm --filter @queohoh/core build && pnpm --filter @queohoh/daemon build` before TUI tests).
- Per-package gates: `pnpm --filter @queohoh/<pkg> test`, `pnpm -r typecheck`. Lint: `mise x node@22 -- pnpm lint` (biome OOMs under Node 25).
- ESM `.js` import extensions; Biome tabs + double quotes; match existing file style.
- TDD per task: failing test → verify → implement → green.
- Commit per task with explicit file paths (never `git add .`); `--no-verify` if the pre-commit hook interferes; conventional messages; no Co-Authored-By trailer.
- ESC always closes any modal; `q` closes only non-text-input modals.
- Marker glyphs: `⛓` (main-session queue row), `◆` (lane has main pointer).
- Main-pointer file: `<state>/main-sessions.json`, shape `{ "sessions": { "<repo:worktree>": "<sessionId>" } }`.

---

### Task 1: Core — `session` field on tasks

**Files:**
- Modify: `packages/core/src/task.ts`, `packages/core/src/store.ts`
- Test: `packages/core/src/__tests__/task.test.ts` (or wherever parse/serialize tests live — find them), store tests

**Interfaces:**
- Produces: `TaskInstance.session: "fresh" | "main"`; frontmatter key `session` with zod default `"fresh"`; `serializeTaskFile` writes it. `NewTaskInput.session?: "fresh" | "main"`; `QueueStore.create` defaults `"fresh"`.

- [ ] **Step 1:** Failing tests: parse task file without `session` → `"fresh"`; round-trip with `session: "main"`; `create({ ..., session: "main" })` persists it.
- [ ] **Step 2:** Verify failures.
- [ ] **Step 3:** Implement — `TaskMetaSchema` gains `session: z.enum(["fresh", "main"]).default("fresh")`; thread through parse/serialize/create (schema is `.strict()`, so serialize must always emit the key).
- [ ] **Step 4:** Core suite + typecheck green.
- [ ] **Step 5:** Commit `feat(core): session field on tasks (fresh | main)`.

---

### Task 2: Core — MainSessionStore

**Files:**
- Create: `packages/core/src/main-sessions.ts`
- Modify: `packages/core/src/index.ts` (export)
- Test: `packages/core/src/__tests__/main-sessions.test.ts`

**Interfaces:**
- Produces:

```ts
export class MainSessionStore {
	constructor(readonly filePath: string); // loads eagerly; corrupt/missing → empty
	get(lane: string): string | null;
	set(lane: string, sessionId: string): void; // persists atomically (tmp+rename)
	all(): Record<string, string>; // copy
}
```

File shape: `{ "sessions": { "<lane>": "<id>" } }`. Follow `SessionRegistry`'s load/persist pattern (`sessions.ts`) exactly — same defensive JSON.parse, same tmp+rename.

- [ ] **Step 1:** Failing tests: get on empty → null; set/get round-trip; persistence across a second instance on the same path; corrupt file → empty (no throw); `all()` returns a copy (mutating it doesn't affect the store).
- [ ] **Step 2:** Verify failures.
- [ ] **Step 3:** Implement + export from `index.ts`.
- [ ] **Step 4:** Green.
- [ ] **Step 5:** Commit `feat(core): MainSessionStore for per-lane main session pointers`.

---

### Task 3: Core — runner `--resume` + worker pointer resolve/advance

**Files:**
- Modify: `packages/core/src/runner.ts`, `packages/core/src/worker.ts`
- Test: `packages/core/src/__tests__/runner.test.ts`, `packages/core/src/__tests__/worker.test.ts`

**Interfaces:**
- Consumes: Task 1 `session` field, Task 2 `MainSessionStore`.
- Produces:
  - `ExecuteClaudeOptions.resumeSessionId?: string` → when set, args include `"--resume", id` (insert after `--model <model>`, before `claudeArgs`).
  - `WorkerDeps.mainSessions?: MainSessionStore` (optional — absent behaves as before). In `runTask`: if `task.session === "main"` and store present, `resumeSessionId = store.get(laneKey(task)) ?? undefined`. After the run (any outcome), if `task.session === "main"` and `result.sessionId` is non-null, `store.set(laneKey(task), result.sessionId)`.

- [ ] **Step 1:** Failing tests:
  - runner: `resumeSessionId: "abc"` → spawn args contain `--resume abc`; unset → no `--resume` (assert via existing fake-spawn pattern in runner tests — read them first).
  - worker: main task with pointer set → executor receives `resumeSessionId` = pointer; main task without pointer → no resume, and after a run capturing sessionId "s1" the store holds "s1"; fresh task never reads/writes the store; failed main run with captured sessionId still advances; main run with null sessionId leaves pointer unchanged.
- [ ] **Step 2:** Verify failures.
- [ ] **Step 3:** Implement (read worker.ts fully first — hook the advance where RunResult is available for both done and failed outcomes).
- [ ] **Step 4:** Green.
- [ ] **Step 5:** Commit `feat(core): main-session resume — runner --resume, worker pointer resolve/advance`.

---

### Task 4: Daemon — wire store, snapshot `mainSessions`, enqueue params

**Files:**
- Modify: `packages/daemon/src/daemon.ts`, `packages/daemon/src/engine.ts`, `packages/daemon/src/api.ts`, `packages/daemon/src/paths.ts`
- Test: `packages/daemon/src/__tests__/api.test.ts`, `packages/daemon/src/__tests__/engine.test.ts`

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces:
  - `paths.ts`: `mainSessionsPath = (state) => join(state, "daemon/main-sessions.json")`.
  - `daemon.ts` constructs `MainSessionStore` and passes it into `Engine` deps; `engine.ts` threads it into `runTask` deps (`mainSessions`).
  - `StateSnapshot.mainSessions: Record<string, string>` from `store.all()` (ApiServer deps gain the store).
  - `enqueue` RPC: optional `worktree` param → ref `worktree:<name>` (else existing `params.ref ?? "temp"`); optional `session` param (validated to `"fresh" | "main"`, default fresh) → threaded into `store.create`.
  - TUI compat: `normalizeSnapshot` (tui `use-daemon.ts`) gains `mainSessions: {}` default — do it here so the workspace typechecks (tui fixtures too: `makeSnapshot`).

- [ ] **Step 1:** Failing tests: snapshot includes `mainSessions` (empty by default; non-empty after `store.set`); enqueue with `worktree: "wt-a"` → `target.ref === "worktree:wt-a"`; enqueue with `session: "main"` → task field set; engine test: a completed main run advances the store (reuse the fake-executor fixture that yields a sessionId — read engine.test.ts fixtures first).
- [ ] **Step 2:** Verify failures.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Daemon suite + `pnpm -r typecheck` green (tui fixture updates included).
- [ ] **Step 5:** Commit `feat(daemon): main-session store wiring, snapshot mainSessions, enqueue session/worktree`.

---

### Task 5: TUI — Modal component (spike-first)

**Files:**
- Create: `packages/tui/src/components/Modal.tsx`
- Modify: `packages/tui/src/components/TextInput.tsx` (compose inside Modal)
- Test: `packages/tui/src/__tests__/modal.test.tsx`

**Interfaces:**
- Produces:

```tsx
export function modalGeometry(columns: number, rows: number, contentHeight: number): {
	width: number; // min(72, columns - 8), floor 20
	marginLeft: number;
	marginTop: number;
}; // pure, unit-tested

export function Modal(props: {
	title: string;
	columns: number;
	rows: number;
	hint: string; // dim bottom line, e.g. "esc close"
	children: React.ReactNode;
}): JSX.Element;
```

Rendered by App as the last child of a `position="relative"` root; Modal itself renders an absolute Box offset by `modalGeometry`. Interior lines padded to full inner width for opacity (helper `padLine(text, width)` exported for reuse by picker rows).

- [ ] **Step 1 — SPIKE (timeboxed):** in a scratch test, render a root with body text and a last-child absolute Box with border over it; assert via `lastFrame()` that the modal's border/content cells overwrite the body text beneath and body text remains visible outside the modal. If compositing fails (body bleeds through the modal's padded interior or absolute offset doesn't apply), STOP and implement the documented fallback instead: App hides the body while a modal is open and centers the Modal in the empty body — same `Modal` public API, note the fallback in code and in your report.
- [ ] **Step 2:** Failing tests: `modalGeometry` math (clamps, centering, floor 20 width); Modal renders bold title, hint line, padded interior (assert a short content line is padded to inner width); children render.
- [ ] **Step 3:** Verify failures; implement; green.
- [ ] **Step 4:** Commit `feat(tui): centered floating Modal component` (mention spike outcome in the message body).

---

### Task 6: TUI — keymap f/m + selectors markers

**Files:**
- Modify: `packages/tui/src/keymap.ts`, `packages/tui/src/format.ts` (or selectors — wherever queue rows are built), `packages/tui/src/selectors.ts`, `packages/tui/src/components/Footer.tsx`
- Test: `packages/tui/src/__tests__/keymap.test.ts`, `packages/tui/src/__tests__/selectors.test.ts`, components test for Footer

**Interfaces:**
- Consumes: `StateSnapshot.mainSessions` (Task 4).
- Produces:
  - Keymap: worktrees-pane `f` → `{ type: "worktree-add"; session: "fresh" }`, `m` → `{ type: "worktree-add"; session: "main" }`; queue-pane `a` mapping REMOVED (`a` on queue → null).
  - `QueueRow` gains `sessionMarker: string` (`"⛓ "` for main tasks, `""` otherwise) rendered after the glyph in QueuePane.
  - `WorktreeRow` gains `hasMainSession: boolean` (lane in `snapshot.mainSessions`); WorktreesPane renders `◆ ` before state when true.
  - Footer: worktrees hint becomes `[C-s] prefix · [↑↓/jk] select · [f]resh task · [m]ain task · [enter] run def · [q]uit`; queue hint drops `[a]dd`.

- [ ] **Step 1:** Failing tests: keymap f/m emit worktree-add with session (worktrees pane only — on queue/tasks/detail they emit nothing); `a` on queue → null; queue row marker for main tasks; worktree row `hasMainSession`; footer strings.
- [ ] **Step 2:** Verify failures; implement; green (adapt any existing tests pinning the old `a` behavior/footer copy — preserve intent, update expectation).
- [ ] **Step 3:** Commit `feat(tui): f/m main-session keys, chain markers, footer updates`.

---

### Task 7: TUI — App integration (modal-hosted flows + enqueue wiring)

**Files:**
- Modify: `packages/tui/src/App.tsx`, `packages/tui/src/actions.ts`
- Test: `packages/tui/src/__tests__/app.test.tsx`

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `actions.enqueue(prompt, repo, opts?: { worktree?: string; session?: "fresh" | "main" })` threading params into the RPC.
  - App: ALL modal modes render through `Modal` (add-task input, worktree-assign input, def-pick picker, def-args input) — nothing docks at the bottom. Titles: `New task — fresh session — <lane>` / `New task — main session — <lane>` / `Assign worktree — task <last-6-of-id>` / `Run task definition — <lane>` / `<def name> args (<args>)`.
  - `worktree-add` action (from Task 6 keymap) opens the add-task modal for the selected worktree row (kind `"worktree"` only); submit calls `enqueue(prompt, activeProject, { worktree: row.name, session })`.
  - Old `add-prompt`/`queue-add` mode removed.
  - Close semantics: ESC closes every modal; `q` also closes the def-pick picker (add `q` to its handled keys) but NOT text-input modals.

- [ ] **Step 1:** Failing App tests: `f` on a worktree row opens modal titled `New task — fresh session — …` and submit enqueues with `{ worktree, session: "fresh" }` (fake actions assert args); `m` likewise with `"main"`; `a` on queue does nothing; def-pick modal closes on `q` AND esc; text modal: typing `q` inserts a q (assert input echo), esc cancels; modal renders centered (assert title present and body content still rendered outside modal per spike outcome).
- [ ] **Step 2:** Verify failures; implement; green. Full TUI suite + `pnpm -r typecheck && pnpm -r build`.
- [ ] **Step 3:** Commit `feat(tui): modal-hosted action flows + main-session enqueue`.

---

### Task 8: Final sweep

- [ ] **Step 1:** `pnpm -r build && pnpm -r typecheck && pnpm -r test`; `mise x node@22 -- pnpm lint`.
- [ ] **Step 2:** Spec cross-check §1–§7; fix small gaps with tests, report larger ones.
- [ ] **Step 3:** Live smoke: `mise run daemon:restart`, then `node packages/daemon/dist/cli.js status` shows `mainSessions: {}`. Do not launch the interactive TUI.
- [ ] **Step 4:** Commit fixes if any: `fix: post-sweep fixes (session mode + modal)`.
