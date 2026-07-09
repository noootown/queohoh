# TUI Full-Screen Rework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the inline TUI with a full-screen (alt-screen) cockpit: project tabs, three left list panes (QUEUE / TASKS / WORKTREES), a contextual right detail pane with sub-tabs, and tmux-style `ctrl+s` prefix navigation.

**Architecture:** Daemon snapshot grows two additive fields (`projects`, `worktrees`) plus a `runDefinition` worktree override and a `definition` RPC. The TUI is rebuilt around pure, unit-testable modules — `keymap.ts` (prefix/focus state machine), `selectors.ts` (snapshot → view models), height-aware `run-files.ts` — composed by presentational Ink components under a terminal-sized root.

**Tech Stack:** TypeScript (ESM, strict), Ink 6 + React 19, vitest + ink-testing-library, zod, Biome (tabs, double quotes).

**Spec:** `docs/superpowers/specs/2026-07-08-tui-fullscreen-rework-design.md` — read it first.

## Global Constraints

- Monorepo: `packages/core` (pure logic), `packages/daemon` (engine + socket API), `packages/tui` (Ink app). Build order matters: core → daemon → tui.
- Run tests per package: `pnpm --filter @queohoh/<pkg> test` (vitest). Typecheck: `pnpm -r typecheck`. Build: `pnpm -r build`. Lint: `mise run check` (Biome) if available, else `pnpm exec biome check --write packages/`.
- Biome style: tab indentation, double quotes. Match existing file style exactly.
- All imports use `.js` extension (ESM).
- Never delete a test without preserving its coverage intent somewhere.
- Commit after every task (conventional commits, no Co-Authored-By trailer).
- Keys: `↑/↓` and `j/k` are equivalent in every list; `ctrl+s` is the global prefix.
- Alt-screen escapes: enter `\x1b[?1049h`, leave `\x1b[?1049l`.
- Tiny-terminal guard threshold: columns < 60 or rows < 15.

---

### Task 1: Daemon snapshot exposes projects + worktrees

**Files:**
- Modify: `packages/daemon/src/engine.ts` (add accessor)
- Modify: `packages/daemon/src/api.ts` (StateSnapshot + snapshot())
- Test: `packages/daemon/src/__tests__/engine.test.ts`, `packages/daemon/src/__tests__/api.test.ts`

**Interfaces:**
- Produces: `Engine.worktreesByRepo(): Record<string, WorktreeInfo[]>`
- Produces: `StateSnapshot` gains `projects: { name: string }[]` and `worktrees: Record<string, WorktreeInfo[]>` (project name → worktrees; `WorktreeInfo = { name, path, branch }` from `@queohoh/core`).

- [ ] **Step 1: Read the existing test setups** in `engine.test.ts` and `api.test.ts` to reuse their fixture helpers (fake `resolverIO`, temp state dirs).

- [ ] **Step 2: Write failing tests.**
  - engine test: after `tick()`, `engine.worktreesByRepo()` returns `{ [projectName]: [...worktrees from the fake resolverIO] }`.
  - api test: `snapshot()` contains `projects: [{ name: "<project>" }, ...]` in config order, and `worktrees` mirroring `engine.worktreesByRepo()`.

- [ ] **Step 3: Run tests to verify they fail** (`pnpm --filter @queohoh/daemon test`).

- [ ] **Step 4: Implement.**

In `engine.ts` (next to `runningTaskIds`):

```ts
worktreesByRepo(): Record<string, WorktreeInfo[]> {
	return Object.fromEntries(this.worktreeCache);
}
```

In `api.ts`:

```ts
export interface StateSnapshot {
	tasks: TaskInstance[];
	archivedRecent: TaskInstance[];
	sessions: SessionEntry[];
	running: string[];
	projects: { name: string }[];
	worktrees: Record<string, WorktreeInfo[]>;
}
```

and in `snapshot()`:

```ts
projects: this.deps.config.projects.map((p) => ({ name: p.name })),
worktrees: this.deps.engine.worktreesByRepo(),
```

(import `WorktreeInfo` type from `@queohoh/core`).

- [ ] **Step 5: Run tests, typecheck** — all green.
- [ ] **Step 6: Commit** `feat(daemon): expose projects and worktrees in state snapshot`

---

### Task 2: Core — instantiateDefinition ref override

**Files:**
- Modify: `packages/core/src/instantiate.ts`
- Test: `packages/core/src/__tests__/instantiate.test.ts`

**Interfaces:**
- Produces: `InstantiateDeps` gains optional `refOverride?: string`. When set, every created task's `ref` is exactly `refOverride` instead of the rendered `def.worktree` template.

- [ ] **Step 1: Write failing test** in `instantiate.test.ts`: instantiate a definition (args mode) with `refOverride: "worktree:wt-plan-a"`; assert the created task has `target.ref === "worktree:wt-plan-a"` even though the definition's `worktree` config says something else (e.g. `"temp"`).

- [ ] **Step 2: Run to verify it fails.**

- [ ] **Step 3: Implement** — in `instantiate.ts`, add `refOverride?: string` to `InstantiateDeps` and change the create call:

```ts
ref: deps.refOverride ?? render(def.worktree, globalVars, repoVars, item),
```

- [ ] **Step 4: Tests + typecheck green.**
- [ ] **Step 5: Commit** `feat(core): instantiateDefinition accepts refOverride`

---

### Task 3: Daemon — runDefinition worktree param + definition RPC

**Files:**
- Modify: `packages/daemon/src/api.ts` (dispatch)
- Test: `packages/daemon/src/__tests__/api.test.ts`

**Interfaces:**
- Consumes: `refOverride` from Task 2.
- Produces: RPC `runDefinition` accepts optional `worktree: string` param → tasks created with ref `worktree:<name>`.
- Produces: RPC `definition { repo, name }` → returns the full `TaskDefinition` (loaded via `loadDefinition`); throws `unknown repo: <repo>` for unregistered repos and propagates load errors for missing definitions.

- [ ] **Step 1: Write failing tests** (reuse api.test.ts fixtures that already create a workspace with task definitions):
  - `runDefinition` with `worktree: "wt-x"` → created task `target.ref === "worktree:wt-x"`.
  - `definition` with valid repo/name → result has `prompt`, `args`, `worktree`, `model` fields matching the fixture definition.
  - `definition` with unknown repo → error reply.

- [ ] **Step 2: Verify failures.**

- [ ] **Step 3: Implement** in `api.ts` dispatch:
  - In `runDefinition` case, before `instantiateDefinition`: `const worktree = typeof params.worktree === "string" && params.worktree.length > 0 ? params.worktree : undefined;` and pass `refOverride: worktree ? \`worktree:${worktree}\` : undefined` in the deps object.
  - New case:

```ts
case "definition": {
	const repo = String(params.repo ?? "");
	const name = String(params.name ?? "");
	if (!deps.config.projects.some((p) => p.name === repo)) {
		throw new Error(`unknown repo: ${repo}`);
	}
	return loadDefinition(projectWorkspaceDir(deps.config, repo), repo, name);
}
```

- [ ] **Step 4: Tests + typecheck green.**
- [ ] **Step 5: Commit** `feat(daemon): runDefinition worktree override + definition RPC`

---

### Task 4: TUI actions — definition fetch + worktree override

**Files:**
- Modify: `packages/tui/src/actions.ts`

**Interfaces:**
- Consumes: Task 3 RPCs.
- Produces (used by App in Task 11):
  - `runDefinition(repo, name, args, worktree?): Promise<string | null>` (extra optional param, threaded into RPC params only when defined)
  - `definition(repo, name): Promise<TaskDefinition | null>` (null on any error)
  - `DefinitionSummary` unchanged. Import `TaskDefinition` type from `@queohoh/core`.

- [ ] **Step 1: Implement** (no dedicated test — this module is a thin RPC wrapper with no existing tests; it is exercised through App tests in Task 11 and daemon api tests in Task 3):

```ts
runDefinition: async (repo, name, args, worktree) => {
	const result = await mutate("runDefinition", {
		repo,
		name,
		args,
		source: "tui",
		...(worktree ? { worktree } : {}),
	});
	if (result?.includes("timed out")) return null;
	return result;
},
definition: async (repo, name) => {
	try {
		return await withClient(
			sockPath,
			(c) => c.call("definition", { repo, name }) as Promise<TaskDefinition>,
		);
	} catch {
		return null;
	}
},
```

Update the `Actions` interface accordingly.

- [ ] **Step 2: `pnpm --filter @queohoh/tui typecheck` green** (tests still pass — nothing consumed the new members yet).
- [ ] **Step 3: Commit** `feat(tui): actions gain definition fetch and worktree override`

---

### Task 5: TUI — alt-screen module + useTerminalSize hook

**Files:**
- Create: `packages/tui/src/alt-screen.ts`
- Create: `packages/tui/src/use-terminal-size.ts`
- Modify: `packages/tui/src/cli.tsx`
- Test: `packages/tui/src/__tests__/alt-screen.test.ts`, `packages/tui/src/__tests__/use-terminal-size.test.tsx`

**Interfaces:**
- Produces: `enterAltScreen(out?)` / `leaveAltScreen(out?)` — write `\x1b[?1049h` / `\x1b[?1049l` to `out` (default `process.stdout`); `installAltScreenGuards(out?)` registers `exit`/`SIGINT`/`SIGTERM` handlers that leave alt screen (idempotent leave — writes only once).
- Produces: `useTerminalSize(stream?): { columns: number; rows: number }` — defaults to `process.stdout`, falls back to 80×24 when undefined, subscribes/unsubscribes to the stream's `"resize"` event.

- [ ] **Step 1: Write failing tests.**

```ts
// alt-screen.test.ts
import { describe, expect, it } from "vitest";
import { createAltScreen } from "../alt-screen.js";

function fakeOut(): { writes: string[]; stream: { write: (s: string) => boolean } } {
	const writes: string[] = [];
	return { writes, stream: { write: (s: string) => (writes.push(s), true) } };
}

describe("alt screen", () => {
	it("enter writes 1049h, leave writes 1049l once", () => {
		const { writes, stream } = fakeOut();
		const alt = createAltScreen(stream as unknown as NodeJS.WriteStream);
		alt.enter();
		alt.leave();
		alt.leave(); // idempotent
		expect(writes).toEqual(["\x1b[?1049h", "\x1b[?1049l"]);
	});
});
```

```ts
// use-terminal-size.test.tsx — render a probe component via ink-testing-library
// with a fake EventEmitter stream {columns: 100, rows: 40}; assert rendered
// "100x40"; emit "resize" after mutating columns/rows; assert re-render.
```

- [ ] **Step 2: Verify failures.**

- [ ] **Step 3: Implement.**

```ts
// alt-screen.ts
export interface AltScreen {
	enter(): void;
	leave(): void;
	installGuards(): void;
}

export function createAltScreen(
	out: NodeJS.WriteStream = process.stdout,
): AltScreen {
	let entered = false;
	const enter = () => {
		if (entered) return;
		entered = true;
		out.write("\x1b[?1049h");
	};
	const leave = () => {
		if (!entered) return;
		entered = false;
		out.write("\x1b[?1049l");
	};
	const installGuards = () => {
		process.on("exit", leave);
		process.on("SIGINT", () => {
			leave();
			process.exit(130);
		});
		process.on("SIGTERM", () => {
			leave();
			process.exit(143);
		});
	};
	return { enter, leave, installGuards };
}
```

```ts
// use-terminal-size.ts
import { useEffect, useState } from "react";

export interface TerminalSize {
	columns: number;
	rows: number;
}

type SizeStream = Pick<NodeJS.WriteStream, "columns" | "rows"> &
	Pick<NodeJS.EventEmitter, "on" | "off">;

export function useTerminalSize(
	stream: SizeStream = process.stdout,
): TerminalSize {
	const read = () => ({
		columns: stream.columns ?? 80,
		rows: stream.rows ?? 24,
	});
	const [size, setSize] = useState<TerminalSize>(read);
	useEffect(() => {
		const onResize = () => setSize(read());
		stream.on("resize", onResize);
		return () => {
			stream.off("resize", onResize);
		};
		// biome-ignore lint/correctness/useExhaustiveDependencies: read closes over stream
	}, [stream]);
	return size;
}
```

`cli.tsx` becomes:

```tsx
#!/usr/bin/env node
import { runsPath, socketPath, statePath } from "@queohoh/daemon";
import { render } from "ink";
import { createAltScreen } from "./alt-screen.js";
import { App } from "./App.js";
import { createActions } from "./actions.js";

const sock = socketPath(statePath());
const alt = createAltScreen();
alt.installGuards();
alt.enter();
const instance = render(
	<App
		sockPath={sock}
		runsDir={runsPath(statePath())}
		actions={createActions(sock)}
	/>,
);
void instance.waitUntilExit().then(() => alt.leave());
```

- [ ] **Step 4: Tests + typecheck green.**
- [ ] **Step 5: Commit** `feat(tui): alt-screen lifecycle and terminal-size hook`

---

### Task 6: TUI — keymap state machine (pure)

**Files:**
- Create: `packages/tui/src/keymap.ts`
- Test: `packages/tui/src/__tests__/keymap.test.ts`

**Interfaces:**
- Produces (consumed by App in Task 11):

```ts
export type PaneId = "queue" | "tasks" | "worktrees" | "detail";
export type ListPaneId = Exclude<PaneId, "detail">;
export type Direction = "up" | "down" | "left" | "right";

export interface KeyInput {
	input: string; // the char from ink useInput
	ctrl: boolean;
	upArrow: boolean;
	downArrow: boolean;
	leftArrow: boolean;
	rightArrow: boolean;
	return: boolean;
}

export type KeymapAction =
	| { type: "quit" }
	| { type: "move-selection"; delta: 1 | -1 }
	| { type: "activate" } // enter on tasks/worktrees; enter on queue = focus detail
	| { type: "focus"; pane: PaneId }
	| { type: "move-focus"; dir: Direction }
	| { type: "switch-tab"; index: number } // 0-based
	| { type: "cycle-tab"; delta: 1 | -1 }
	| { type: "switch-subtab"; index: number } // 0-based
	| { type: "queue-add" }
	| { type: "queue-retry" }
	| { type: "queue-skip" }
	| { type: "queue-worktree" }
	| { type: "scroll"; delta: 1 | -1 }
	| { type: "scroll-edge"; edge: "top" | "bottom" };

export interface KeymapResult {
	prefixArmed: boolean; // new armed state
	action: KeymapAction | null;
}

export function handleKey(
	prefixArmed: boolean,
	focus: PaneId,
	key: KeyInput,
): KeymapResult;

export function moveFocus(
	current: PaneId,
	dir: Direction,
	lastListPane: ListPaneId,
): PaneId;
```

- [ ] **Step 1: Write exhaustive failing tests.** Cover:
  - `ctrl+s` arms (`prefixArmed: true`, no action); any second key disarms.
  - Armed + arrows/hjkl → `move-focus` with the right dir; armed + `1`..`9` → `switch-tab` index 0..8; armed + `n`/`p` → `cycle-tab` ±1; armed + other → no action, disarmed.
  - Unprefixed `q` → quit from every pane.
  - Unprefixed digits → `switch-subtab` (index 0-based).
  - Queue focus: `j`/`downArrow` → move-selection +1; `k`/`upArrow` → −1; `a`/`r`/`s`/`w` → queue-add/retry/skip/worktree; `return` → `{ type: "focus", pane: "detail" }`.
  - Tasks focus: selection keys; `return` → activate.
  - Worktrees focus: selection keys; `return` and `t` → activate.
  - Detail focus: `j`/`k`/arrows → scroll ±1; `g` → scroll-edge top; `G` → scroll-edge bottom.
  - `moveFocus` geometry: queue↓→tasks↓→worktrees (clamped at ends); any left pane + right → detail; detail + left → `lastListPane`; queue + up stays queue; detail + up/down stays detail.

- [ ] **Step 2: Verify failures.**

- [ ] **Step 3: Implement** `handleKey` as a pure function:

```ts
const DIR_KEYS: Record<string, Direction> = {
	h: "left",
	j: "down",
	k: "up",
	l: "right",
};

function arrowDir(key: KeyInput): Direction | null {
	if (key.upArrow) return "up";
	if (key.downArrow) return "down";
	if (key.leftArrow) return "left";
	if (key.rightArrow) return "right";
	return null;
}

export function handleKey(
	prefixArmed: boolean,
	focus: PaneId,
	key: KeyInput,
): KeymapResult {
	if (key.ctrl && key.input === "s") {
		return { prefixArmed: true, action: null };
	}
	if (prefixArmed) {
		const dir = arrowDir(key) ?? DIR_KEYS[key.input] ?? null;
		if (dir) return { prefixArmed: false, action: { type: "move-focus", dir } };
		if (/^[1-9]$/.test(key.input)) {
			return {
				prefixArmed: false,
				action: { type: "switch-tab", index: Number(key.input) - 1 },
			};
		}
		if (key.input === "n")
			return { prefixArmed: false, action: { type: "cycle-tab", delta: 1 } };
		if (key.input === "p")
			return { prefixArmed: false, action: { type: "cycle-tab", delta: -1 } };
		return { prefixArmed: false, action: null };
	}
	if (key.input === "q") return { prefixArmed: false, action: { type: "quit" } };
	if (/^[1-9]$/.test(key.input)) {
		return {
			prefixArmed: false,
			action: { type: "switch-subtab", index: Number(key.input) - 1 },
		};
	}
	const dir = arrowDir(key) ?? DIR_KEYS[key.input] ?? null;
	if (focus === "detail") {
		if (dir === "down") return act({ type: "scroll", delta: 1 });
		if (dir === "up") return act({ type: "scroll", delta: -1 });
		if (key.input === "g") return act({ type: "scroll-edge", edge: "top" });
		if (key.input === "G") return act({ type: "scroll-edge", edge: "bottom" });
		return { prefixArmed: false, action: null };
	}
	if (dir === "down") return act({ type: "move-selection", delta: 1 });
	if (dir === "up") return act({ type: "move-selection", delta: -1 });
	if (focus === "queue") {
		if (key.return) return act({ type: "focus", pane: "detail" });
		if (key.input === "a") return act({ type: "queue-add" });
		if (key.input === "r") return act({ type: "queue-retry" });
		if (key.input === "s") return act({ type: "queue-skip" });
		if (key.input === "w") return act({ type: "queue-worktree" });
	}
	if (focus === "tasks" && key.return) return act({ type: "activate" });
	if (focus === "worktrees" && (key.return || key.input === "t")) {
		return act({ type: "activate" });
	}
	return { prefixArmed: false, action: null };
}

function act(action: KeymapAction): KeymapResult {
	return { prefixArmed: false, action };
}

const COLUMN_ORDER: ListPaneId[] = ["queue", "tasks", "worktrees"];

export function moveFocus(
	current: PaneId,
	dir: Direction,
	lastListPane: ListPaneId,
): PaneId {
	if (current === "detail") {
		return dir === "left" ? lastListPane : "detail";
	}
	if (dir === "right") return "detail";
	if (dir === "left") return current;
	const idx = COLUMN_ORDER.indexOf(current);
	const next = dir === "down" ? idx + 1 : idx - 1;
	return COLUMN_ORDER[Math.min(COLUMN_ORDER.length - 1, Math.max(0, next))];
}
```

Note: `g`/`G` require case-sensitive `input` matching — ink passes the shifted char as `input`.

- [ ] **Step 4: Tests + typecheck green.**
- [ ] **Step 5: Commit** `feat(tui): pure keymap state machine with ctrl+s prefix`

---

### Task 7: TUI — height-aware run-file tails

**Files:**
- Modify: `packages/tui/src/run-files.ts`
- Test: `packages/tui/src/__tests__/run-files.test.ts`

**Interfaces:**
- Produces: `readRunFiles(runsDir, taskId, opts?: { tailLines?: number })` — same return shape `{ report, transcriptTail }`; `tailLines` defaults to 25 (current behavior); byte window becomes `max(65536, tailLines * 512)` capped at 262144.

- [ ] **Step 1: Write failing test:** create a transcript with 200 lines; `readRunFiles(dir, id, { tailLines: 100 }).transcriptTail` has length 100 ending with the last line; default call still returns 25.

- [ ] **Step 2: Verify failure.**

- [ ] **Step 3: Implement:** thread `tailLines` through `readTranscriptTail(path, tailLines)`; compute `const window = Math.min(262144, Math.max(65536, tailLines * 512));` replacing the constant; `.slice(-tailLines)` at the end. Keep exported signature backward-compatible.

- [ ] **Step 4: Tests green** (existing run-files tests must still pass unchanged).
- [ ] **Step 5: Commit** `feat(tui): height-aware transcript tail`

---

### Task 8: TUI — selectors (snapshot → view models)

**Files:**
- Create: `packages/tui/src/selectors.ts`
- Test: `packages/tui/src/__tests__/selectors.test.ts`
- Reference: `packages/tui/src/format.ts` (reuse `buildQueueRows`), `packages/tui/src/__tests__/helpers.ts` (snapshot fixture helpers — extend, don't fork)

**Interfaces:**
- Consumes: `StateSnapshot` (with Task 1 fields), `buildQueueRows` from `format.js`.
- Produces:

```ts
export interface ProjectTab {
	name: string;
	synthetic: boolean; // repo seen in tasks but absent from config projects
}
export function buildProjectTabs(snapshot: StateSnapshot): ProjectTab[];
// config projects in order, then synthetic repos (from tasks + archivedRecent) sorted alphabetically

export function queueRowsForProject(
	snapshot: StateSnapshot,
	project: string,
	now: number,
	width: number,
): QueueRow[];
// filters tasks/archivedRecent by target.repo === project, then delegates to buildQueueRows

export type WorktreeState = "busy" | "failed" | "free";
export interface WorktreeRow {
	kind: "worktree" | "session";
	name: string; // worktree name, or session label (cwd basename)
	path: string;
	branch: string | null;
	state: WorktreeState | "you";
}
export function buildWorktreeRows(
	snapshot: StateSnapshot,
	project: string,
): WorktreeRow[];
// one row per snapshot.worktrees[project] entry; state:
//   busy   — some snapshot.tasks entry with status "running" whose laneKey === `${project}:${name}`
//   failed — no running, and the newest (max id) task on that lane has status "failed"
//   free   — otherwise
// then one "session" row per interactive session whose cwd is inside any of the
// project's worktree paths (cwd === path or cwd.startsWith(path + "/")), state "you".

export function windowRows<T>(
	rows: T[],
	selected: number,
	capacity: number,
): { rows: T[]; offset: number };
// slice of rows sized ≤ capacity that keeps `selected` visible (scroll window);
// offset is the index of the first returned row.
```

- [ ] **Step 1: Write failing tests** covering: tab order + synthetic tab appears only when an orphan repo exists; project filtering (tasks of other projects excluded); worktree states busy/failed/free; session row matched by cwd prefix; `windowRows` at top/middle/bottom edges and capacity ≥ rows length.

- [ ] **Step 2: Verify failures.**

- [ ] **Step 3: Implement.** Notes: derive "newest task" by `id` (ULIDs sort chronologically). `windowRows`:

```ts
export function windowRows<T>(
	rows: T[],
	selected: number,
	capacity: number,
): { rows: T[]; offset: number } {
	if (capacity <= 0) return { rows: [], offset: 0 };
	if (rows.length <= capacity) return { rows, offset: 0 };
	const clamped = Math.min(Math.max(selected, 0), rows.length - 1);
	let offset = clamped - Math.floor(capacity / 2);
	offset = Math.max(0, Math.min(offset, rows.length - capacity));
	return { rows: rows.slice(offset, offset + capacity), offset };
}
```

- [ ] **Step 4: Tests + typecheck green.**
- [ ] **Step 5: Commit** `feat(tui): snapshot selectors for tabs, project filtering, worktree rows`

---

### Task 9: TUI — presentational components (TabBar, Pane, list panes, Footer)

**Files:**
- Create: `packages/tui/src/components/Pane.tsx`
- Create: `packages/tui/src/components/TabBar.tsx`
- Create: `packages/tui/src/components/TasksPane.tsx`
- Create: `packages/tui/src/components/WorktreesPane.tsx`
- Modify: `packages/tui/src/components/QueuePanel.tsx` (rename export to `QueuePane`, add focus/height props)
- Modify: `packages/tui/src/components/Footer.tsx` (per-pane keys + prefix indicator + status line)
- Test: `packages/tui/src/__tests__/components.test.tsx` (extend)

**Interfaces:**
- Consumes: `ProjectTab`, `WorktreeRow`, `windowRows` (Task 8), `QueueRow` (format), `DefinitionSummary` (actions).
- Produces:

```tsx
export function Pane(props: {
	title: string;
	focused: boolean;
	children: React.ReactNode;
	flexGrow?: number;
	height?: number;
}): JSX.Element;
// round border; borderColor "cyan" when focused, "gray" otherwise; bold title line

export function TabBar(props: {
	tabs: ProjectTab[];
	activeIndex: number;
	connected: boolean;
	runningCount: number;
	maxConcurrent: number | null; // null → omit "/N"
}): JSX.Element;
// " 1:platform  2:queohoh " — active tab inverse+bold; right side:
// connected ? green "●" : yellow "daemon unreachable — retrying…";
// `running {runningCount}/{maxConcurrent}`

export function QueuePane(props: {
	rows: QueueRow[];
	selectedIndex: number;
	focused: boolean;
	capacity: number; // max visible rows; use windowRows
}): JSX.Element;

export function TasksPane(props: {
	defs: DefinitionSummary[];
	selectedIndex: number;
	focused: boolean;
	capacity: number;
}): JSX.Element;
// row: name + (args) when present + " ⏰" when hasDiscovery

export function WorktreesPane(props: {
	rows: WorktreeRow[];
	selectedIndex: number;
	focused: boolean;
	capacity: number;
}): JSX.Element;
// row: name + dim state; "you" renders yellow "YOU"

export function Footer(props: {
	focus: PaneId;
	prefixArmed: boolean;
	statusLine: string | null;
}): JSX.Element;
// statusLine (red) wins; else prefixArmed → " PREFIX — arrows/hjkl move · 1-9 tab · n/p cycle ";
// else per-pane hints:
//   queue:     "[C-s] prefix · [↑↓/jk] select · [a]dd · [r]etry · [s]kip · [w]orktree · [enter] detail · [q]uit"
//   tasks:     "[C-s] prefix · [↑↓/jk] select · [enter] run · [q]uit"
//   worktrees: "[C-s] prefix · [↑↓/jk] select · [enter] run task here · [q]uit"
//   detail:    "[C-s] prefix · [↑↓/jk] scroll · [g/G] top/bottom · [1-9] sub-tab · [q]uit"
```

- [ ] **Step 1: Write failing render tests** (ink-testing-library `render(...).lastFrame()`): TabBar active highlight + unreachable banner; Pane border color by focus (assert frame contains title; focus color via `lastFrame()` ANSI is brittle — instead pass focused and assert via snapshot of two frames differing); QueuePane empty state ("queue empty — press a to add"), selection inverse, windowing (capacity 2 of 5 rows shows selected); TasksPane badge; WorktreesPane YOU row; Footer variants (per-pane, prefix, statusLine precedence).

- [ ] **Step 2: Verify failures.**

- [ ] **Step 3: Implement.** Keep components dumb — all derivation upstream. QueuePane keeps existing row format (`{glyph} {lane} {summary} {detail}`), dims archived rows.

- [ ] **Step 4: Adapt existing `components.test.tsx` assertions** to renamed `QueuePane`/new `Footer` props — preserve every behavior the old tests pinned (empty queue message, archived dimming, status line rendering now via Footer).

- [ ] **Step 5: Tests + typecheck green.**
- [ ] **Step 6: Commit** `feat(tui): full-screen presentational components`

---

### Task 10: TUI — DetailPane with contextual sub-tabs

**Files:**
- Create: `packages/tui/src/components/DetailPane.tsx`
- Create: `packages/tui/src/detail.ts` (pure context/sub-tab logic)
- Test: `packages/tui/src/__tests__/detail.test.ts`, extend `packages/tui/src/__tests__/components.test.tsx`
- Delete (Task 11 actually removes usage): `packages/tui/src/components/DetailView.tsx`, `packages/tui/src/components/RightColumn.tsx`

**Interfaces:**
- Consumes: `WorktreeRow` (Task 8), `readRunFiles` (Task 7), `TaskDefinition` (core), `TaskInstance` (core).
- Produces:

```ts
// detail.ts
export type DetailContext =
	| { kind: "run"; task: TaskInstance }
	| { kind: "definition"; repo: string; name: string }
	| { kind: "worktree"; row: WorktreeRow; laneTasks: TaskInstance[] }
	| { kind: "empty" };

export function subTabsFor(kind: DetailContext["kind"]): string[];
// run → ["transcript", "report", "prompt"]; definition → ["prompt", "config"];
// worktree → ["info"]; empty → []

export function clampSubTab(index: number, kind: DetailContext["kind"]): number;
```

```tsx
// DetailPane.tsx
export function DetailPane(props: {
	context: DetailContext;
	subTab: number;
	focused: boolean;
	width: number;
	height: number; // content rows available
	scrollOffset: number; // lines from bottom for transcript, from top for others
	runFiles: { report: string | null; transcriptTail: string[] } | null; // for kind "run"
	definition: TaskDefinition | null; // for kind "definition"; null → loading/error placeholder
}): JSX.Element;
```

Rendering rules:
- Sub-tab strip: `1:transcript  2:report  3:prompt` style, active inverse.
- run/transcript: last `height` lines of `transcriptTail` shifted by `scrollOffset`; `(no transcript yet)` placeholder.
- run/report: report text split to lines, windowed by scrollOffset from top; `(no report yet)`.
- run/prompt: `task.prompt` lines, windowed.
- definition/prompt: `definition.prompt`; definition/config: one line per field — args, worktree, dedup, model, timeout (ms → original string not available; render `${timeoutMs}ms`), priority, discovery command or `—`; `(loading definition…)` when null.
- worktree/info: `path`, `branch`, `state`, blank line, `tasks on this lane:` then one row per laneTask (glyph + summary via `promptSummary(task.prompt, width)`), `(none)` if empty.
- empty: `(nothing selected)`.

- [ ] **Step 1: Write failing tests** for `subTabsFor`/`clampSubTab` and DetailPane render of each context/sub-tab incl. placeholders and scroll windowing.
- [ ] **Step 2: Verify failures.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Tests + typecheck green.**
- [ ] **Step 5: Commit** `feat(tui): contextual detail pane with sub-tabs`

---

### Task 11: TUI — App rewrite (full-screen composition)

**Files:**
- Rewrite: `packages/tui/src/App.tsx`
- Delete: `packages/tui/src/components/DetailView.tsx`, `packages/tui/src/components/RightColumn.tsx`
- Modify: `packages/tui/src/__tests__/app.test.tsx`, `packages/tui/src/__tests__/smoke.test.tsx`, `packages/tui/src/__tests__/helpers.ts`
- Reference: spec §2–§3 for exact behavior.

**Interfaces:**
- Consumes: everything from Tasks 4–10.
- Produces: `App({ sockPath, runsDir, actions, stdoutStream? })` — optional `stdoutStream` prop threaded to `useTerminalSize` for tests.

**Behavior checklist (implement all):**

1. **Root layout**: `useTerminalSize()` → root `<Box width={columns} height={rows} flexDirection="column">`; tiny-terminal guard (`columns < 60 || rows < 15` → single `<Text>terminal too small (60x15 minimum)</Text>`).
2. **Rows**: TabBar (1) / body (grows) / modal overlay (when open) / Footer (1). Body: left column `width="34%"` (Queue pane grows; Tasks and Worktrees capped at 25% of body height each via `capacity`), right DetailPane fills the rest.
3. **State**:
   - `activeTab: number` (clamped to tabs length).
   - Per-tab UI state in a `Map<string, TabUiState>` keyed by tab name: `{ focus: PaneId; lastListPane: ListPaneId; selections: { queue: number; tasks: number; worktrees: number }; subTab: Record<DetailContext["kind"], number>; scrollOffset: number }` — preserved across tab switches, defaults on first visit.
   - `prefixArmed: boolean` + 2 s `setTimeout` reset (clear on any key).
   - `mode` modal union (keep current shapes): `list | add-prompt | worktree-input | def-pick | def-args`, plus `def-pick`/`def-args` gain optional `worktree?: string` for the worktree-run flow. `add-repo` mode is **deleted** (repo = active tab).
   - `statusLine: string | null`, `now` 1 s tick.
   - Definitions cache: `Map<projectName, DefinitionSummary[]>` fetched on tab activation (and after def-run submit); full-definition cache `Map<"repo/name", TaskDefinition | null>` fetched lazily when a definition row is selected.
   - Run files for the selected queue row polled at 1 s with `tailLines` = detail content height (reuse existing interval pattern from old DetailView).
4. **Key dispatch** (list mode): build `KeyInput` from ink's `useInput` args, call `handleKey(prefixArmed, focus, key)`, apply `prefixArmed`, then `switch` on action:
   - `quit` → `exit()` (cli leaves alt screen).
   - `move-selection` → bump focused list's selection (clamped to its rows).
   - `move-focus` → `moveFocus(...)`; update `lastListPane` when landing on a list pane.
   - `focus` → set focus directly.
   - `switch-tab`/`cycle-tab` → clamp/wrap `activeTab`; wrap for cycle, clamp for direct (ignore out-of-range index).
   - `switch-subtab` → `clampSubTab(index, currentContext.kind)` stored per context kind.
   - `queue-add` → mode add-prompt (submit → `actions.enqueue(prompt, activeProjectName)`).
   - `queue-retry`/`queue-skip` → act on selected queue row (skip archived rows exactly as before).
   - `queue-worktree` → mode worktree-input for selected row.
   - `activate` on tasks → selected def: args.length > 0 ? mode def-args : `actions.runDefinition(repo, name, [])`.
   - `activate` on worktrees → only for `kind === "worktree"` rows: fetch definitions for active project → mode def-pick with `worktree: row.name`.
   - `scroll`/`scroll-edge` → adjust `scrollOffset` (clamp ≥ 0; edge top = large offset clamped by content, bottom = 0 for transcript / 0 = top for others — keep the simple rule: offset is "lines away from the default view", default 0).
   - def-pick/def-args submit paths pass `mode.worktree` through to `actions.runDefinition(repo, name, args, worktree)`.
5. **Detail context derivation**: from `lastListPane` + its selection (`queue` → run context with the selected row's task; `tasks` → definition context; `worktrees` → worktree context with laneTasks filtered from snapshot). Empty selections → empty context.
6. **Daemon-unreachable**: TabBar shows it (Task 9); no separate banner row.

- [ ] **Step 1: Update `helpers.ts`** fixtures: snapshot builder gains `projects`/`worktrees` fields (default `projects` derived from task repos, `worktrees: {}`) so daemon-shape stays consistent everywhere.
- [ ] **Step 2: Write failing App tests** (extend `app.test.tsx`; write stdin via ink-testing-library as the old tests do):
  - renders tab bar with project names; `ctrl+s`+`2` switches tab (frame shows second project's tasks only).
  - queue filtering: tasks from other project absent.
  - `ctrl+s`+`→` then `↓` scrolls detail (frame changes); `ctrl+s`+`←` returns focus.
  - `a` opens prompt input; submit enqueues via fake actions with repo = active project (assert fake called).
  - tasks pane: `ctrl+s`+`↓` focus tasks, `enter` on a def with args opens args input; on a def without args calls runDefinition.
  - worktrees pane: focus, `enter` opens def-pick; picking def calls runDefinition with `worktree` = selected row name.
  - `q` exits (existing pattern).
  - tiny terminal guard (pass fake stdoutStream 40×10).
- [ ] **Step 3: Verify failures.**
- [ ] **Step 4: Rewrite App.tsx** per checklist; delete `DetailView.tsx`/`RightColumn.tsx`; migrate any behavior their tests pinned (transcript placeholder text, worktree lane display) into the new tests if not already covered by Tasks 8–10.
- [ ] **Step 5: Adapt `smoke.test.tsx`/remaining tests**; whole TUI suite green; `pnpm -r typecheck && pnpm -r build` green.
- [ ] **Step 6: Commit** `feat(tui): full-screen app — project tabs, pane focus, contextual detail`

---

### Task 12: Final sweep

**Files:** none new.

- [ ] **Step 1:** `pnpm -r build && pnpm -r typecheck && pnpm -r test` — all green from repo root.
- [ ] **Step 2:** `pnpm exec biome check packages/` (or `mise run check`) — clean.
- [ ] **Step 3:** Spec cross-check: walk spec §1–§7 and confirm each requirement has an implementation + test; fix any gap found.
- [ ] **Step 4:** Manual smoke: `pnpm -r build && node packages/tui/dist/cli.js` in a real terminal *only if a daemon is running* — otherwise verify alt-screen enter/leave by piping to `cat -v` is not feasible; skip gracefully and note in the report.
- [ ] **Step 5:** Commit any fixes `fix(tui): post-sweep fixes`.
