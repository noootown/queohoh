# Bulk Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Shift+arrow contiguous multi-row selection in the three TUI list panes (queue/tasks/worktrees) with bulk actions (rerun/skip/run/remove) that skip inapplicable rows and show counts.

**Architecture:** Per-pane selection state grows from a bare index to `{ cursor, anchor }` in `TabUiState` (App.tsx). Shift+↑/↓ (and `J`/`K`) emit a new `extend-selection` keymap action; plain movement collapses the range. Pressing `a` over a >1-row range opens a bulk action menu whose targets are frozen (resolved to ids/names) at menu-open time and executed as a sequential client-side loop over the existing per-item daemon actions. No daemon/protocol changes.

**Tech Stack:** TypeScript (strict, ESM with `.js` import suffixes), React 19 + ink 6.8, vitest + ink-testing-library, biome (tab indentation).

**Spec:** `docs/superpowers/specs/2026-07-09-bulk-selection-design.md` — read it first.

## Global Constraints

- All code in `packages/tui/`. Run commands from `packages/tui/` (`pnpm test`, `pnpm typecheck`).
- Imports between local files use `.js` suffix (`from "./keymap.js"`), tabs for indentation (biome).
- Ink 6.8 already parses `ESC[1;2A`-style modified arrows into `key.shift` + `key.upArrow`; ink's `useInput` key object has a `shift` boolean.
- Commit messages: conventional prefix (`feat(tui): …`, `test(tui): …`). Do NOT add Co-Authored-By trailers.
- Never break the existing single-selection behavior: `anchor === null` must behave exactly like today.
- Test escape sequences used by existing tests: `SHIFT_DOWN = "[1;2B"`, `SHIFT_UP = "[1;2A"` (add these constants), existing `CTRL_S = ""`, `DOWN = "[B"`, `ESC = ""`.

---

### Task 1: Keymap — shift+arrow / `J`/`K` emit `extend-selection`

**Files:**
- Modify: `packages/tui/src/keymap.ts`
- Modify: `packages/tui/src/App.tsx` (one line: pass `shift` into `KeyInput`)
- Test: `packages/tui/src/__tests__/keymap.test.ts`

**Interfaces:**
- Consumes: nothing new.
- Produces: `KeyInput.shift: boolean` (required field); `KeymapAction` gains `{ type: "extend-selection"; delta: 1 | -1 }`. Task 2 implements the App-side dispatch for it.

- [ ] **Step 1: Write the failing tests**

In `packages/tui/src/__tests__/keymap.test.ts`, first add `shift: false` to the `key()` helper defaults (the `KeyInput` type gains a required field):

```ts
function key(overrides: Partial<KeyInput> = {}): KeyInput {
	return {
		input: "",
		ctrl: false,
		shift: false,
		upArrow: false,
		downArrow: false,
		leftArrow: false,
		rightArrow: false,
		return: false,
		escape: false,
		...overrides,
	};
}
```

Then add a new describe block (list panes are `["queue", "tasks", "worktrees"]`, reuse the existing `LIST_PANES` const):

```ts
describe("handleKey — extend-selection (shift+arrows, J/K)", () => {
	it("shift+down / shift+up in a list pane → extend-selection", () => {
		for (const focus of LIST_PANES) {
			expect(
				handleKey(false, focus, key({ downArrow: true, shift: true })),
			).toEqual({
				prefixArmed: false,
				action: { type: "extend-selection", delta: 1 },
			});
			expect(
				handleKey(false, focus, key({ upArrow: true, shift: true })),
			).toEqual({
				prefixArmed: false,
				action: { type: "extend-selection", delta: -1 },
			});
		}
	});

	it("J/K (shift+j/k) in a list pane → extend-selection", () => {
		expect(handleKey(false, "queue", key({ input: "J", shift: true }))).toEqual(
			{
				prefixArmed: false,
				action: { type: "extend-selection", delta: 1 },
			},
		);
		expect(handleKey(false, "queue", key({ input: "K", shift: true }))).toEqual(
			{
				prefixArmed: false,
				action: { type: "extend-selection", delta: -1 },
			},
		);
	});

	it("plain arrows still emit move-selection", () => {
		expect(handleKey(false, "queue", key({ downArrow: true }))).toEqual({
			prefixArmed: false,
			action: { type: "move-selection", delta: 1 },
		});
	});

	it("shift+arrow in the detail pane keeps scrolling (no extend)", () => {
		expect(
			handleKey(false, "detail", key({ downArrow: true, shift: true })),
		).toEqual({
			prefixArmed: false,
			action: { type: "scroll", delta: 1 },
		});
	});

	it("armed shift+arrow still moves focus (prefix wins)", () => {
		expect(
			handleKey(true, "queue", key({ downArrow: true, shift: true })),
		).toEqual({
			prefixArmed: false,
			action: { type: "move-focus", dir: "down" },
		});
	});
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/tui && pnpm test -- keymap`
Expected: FAIL — type error on `shift` (not in `KeyInput`) and/or `extend-selection` not a `KeymapAction`. (vitest surfaces TS errors at transform time; either failure mode is fine.)

- [ ] **Step 3: Implement in keymap.ts**

Add `shift` to `KeyInput`:

```ts
export interface KeyInput {
	input: string; // the char from ink useInput
	ctrl: boolean;
	shift: boolean;
	upArrow: boolean;
	downArrow: boolean;
	leftArrow: boolean;
	rightArrow: boolean;
	return: boolean;
	escape: boolean;
}
```

Add to `KeymapAction` union:

```ts
	| { type: "extend-selection"; delta: 1 | -1 }
```

In `handleKey`, in the list-pane section (after the `if (key.escape)` line, before the plain `if (dir === "down")` line), insert:

```ts
	if (key.shift && dir === "down")
		return act({ type: "extend-selection", delta: 1 });
	if (key.shift && dir === "up")
		return act({ type: "extend-selection", delta: -1 });
	if (key.input === "J") return act({ type: "extend-selection", delta: 1 });
	if (key.input === "K") return act({ type: "extend-selection", delta: -1 });
```

(The detail-pane branch returns earlier, so shift+arrows there keep scrolling; the prefix-armed branch also returns earlier, so `C-s` + shift+arrow keeps moving focus.)

- [ ] **Step 4: Wire `shift` through App.tsx**

In `App.tsx`'s `useInput` callback, the `keyInput` literal gains one line:

```ts
		const keyInput: KeyInput = {
			input: char,
			ctrl: key.ctrl,
			shift: key.shift,
			upArrow: key.upArrow,
			downArrow: key.downArrow,
			leftArrow: key.leftArrow,
			rightArrow: key.rightArrow,
			return: key.return,
			escape: key.escape,
		};
```

`dispatch` does not handle `extend-selection` yet — the switch has no default case, so the action is a no-op until Task 2. TypeScript will not error.

- [ ] **Step 5: Run tests + typecheck**

Run: `cd packages/tui && pnpm test -- keymap && pnpm typecheck`
Expected: keymap tests PASS, typecheck clean. (`pnpm typecheck` must pass — App.tsx constructs `KeyInput` and would fail if `shift` is missing.)

- [ ] **Step 6: Commit**

```bash
git add packages/tui/src/keymap.ts packages/tui/src/App.tsx packages/tui/src/__tests__/keymap.test.ts
git commit -m "feat(tui): shift+arrow and J/K emit extend-selection keymap action"
```

---

### Task 2: Selection state — anchor+cursor model in App state

**Files:**
- Modify: `packages/tui/src/selectors.ts` (pure helpers)
- Modify: `packages/tui/src/App.tsx`
- Test: `packages/tui/src/__tests__/selectors.test.ts`

**Interfaces:**
- Consumes: `{ type: "extend-selection"; delta }` from Task 1.
- Produces (used by Tasks 3–5):
  - `selectors.ts`: `interface PaneSelection { cursor: number; anchor: number | null }`, `clampSelection(sel: PaneSelection, count: number): PaneSelection`, `selectionRange(sel: PaneSelection): { start: number; end: number }` (inclusive).
  - `App.tsx`: `TabUiState.selections: Record<ListPaneId, PaneSelection>`; derived consts `queueSel`, `tasksSel`, `wtSel` are now `PaneSelection` (cursor accessed as `queueSel.cursor`); helpers `visibleCount(pane)` and `paneSel(pane)`.

- [ ] **Step 1: Write the failing selector tests**

Append to `packages/tui/src/__tests__/selectors.test.ts` (it already imports from `../selectors.js` — extend the import list with `clampSelection`, `selectionRange`, and `type PaneSelection`):

```ts
describe("clampSelection", () => {
	it("clamps cursor and anchor to the row count", () => {
		expect(clampSelection({ cursor: 9, anchor: 4 }, 3)).toEqual({
			cursor: 2,
			anchor: 2,
		});
	});

	it("resets to origin when the list is empty", () => {
		expect(clampSelection({ cursor: 2, anchor: 0 }, 0)).toEqual({
			cursor: 0,
			anchor: null,
		});
	});

	it("keeps a null anchor null", () => {
		expect(clampSelection({ cursor: 1, anchor: null }, 5)).toEqual({
			cursor: 1,
			anchor: null,
		});
	});
});

describe("selectionRange", () => {
	it("single selection: start === end === cursor", () => {
		expect(selectionRange({ cursor: 2, anchor: null })).toEqual({
			start: 2,
			end: 2,
		});
	});

	it("orders anchor/cursor regardless of direction", () => {
		expect(selectionRange({ cursor: 1, anchor: 4 })).toEqual({
			start: 1,
			end: 4,
		});
		expect(selectionRange({ cursor: 4, anchor: 1 })).toEqual({
			start: 1,
			end: 4,
		});
	});
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/tui && pnpm test -- selectors`
Expected: FAIL — `clampSelection` / `selectionRange` not exported.

- [ ] **Step 3: Implement the helpers in selectors.ts**

Add near `windowRows`:

```ts
export interface PaneSelection {
	cursor: number;
	anchor: number | null;
}

/**
 * Clamp a selection to `count` visible rows. An emptied pane resets to the
 * origin with no range; a clamped anchor sticks to the last row rather than
 * disappearing, so a filtered-down list keeps a sensible range.
 */
export function clampSelection(
	sel: PaneSelection,
	count: number,
): PaneSelection {
	if (count <= 0) return { cursor: 0, anchor: null };
	const clamp = (n: number) => Math.max(0, Math.min(n, count - 1));
	return {
		cursor: clamp(sel.cursor),
		anchor: sel.anchor === null ? null : clamp(sel.anchor),
	};
}

/** Inclusive [start, end] visible-row span covered by a selection. */
export function selectionRange(sel: PaneSelection): {
	start: number;
	end: number;
} {
	const anchor = sel.anchor ?? sel.cursor;
	return {
		start: Math.min(anchor, sel.cursor),
		end: Math.max(anchor, sel.cursor),
	};
}
```

- [ ] **Step 4: Run selector tests to verify they pass**

Run: `cd packages/tui && pnpm test -- selectors`
Expected: PASS.

- [ ] **Step 5: Rewire App.tsx state to PaneSelection**

All in `packages/tui/src/App.tsx`:

1. Extend the `./selectors.js` import with `clampSelection`, `selectionRange`, and `type PaneSelection`.

2. `TabUiState.selections` and `DEFAULT_UI`:

```ts
interface TabUiState {
	focus: PaneId;
	lastListPane: ListPaneId;
	selections: Record<ListPaneId, PaneSelection>;
	search: Record<ListPaneId, string>;
	subTab: Record<DetailContext["kind"], number>;
	scrollOffset: number;
}

const DEFAULT_UI: TabUiState = {
	focus: "queue",
	lastListPane: "queue",
	selections: {
		queue: { cursor: 0, anchor: null },
		tasks: { cursor: 0, anchor: null },
		worktrees: { cursor: 0, anchor: null },
	},
	search: { queue: "", tasks: "", worktrees: "" },
	subTab: { run: 0, definition: 0, worktree: 0, empty: 0 },
	scrollOffset: 0,
};
```

3. Replace the three `clampIdx` derivations with (keep the `clampIdx` function itself — movement still uses it):

```ts
	const queueSel = clampSelection(ui.selections.queue, visibleQueueRows.length);
	const tasksSel = clampSelection(ui.selections.tasks, visibleDefs.length);
	const wtSel = clampSelection(ui.selections.worktrees, visibleWtRows.length);
```

4. Add lookup helpers right below (before `context`):

```ts
	const visibleCount = (pane: ListPaneId): number =>
		pane === "queue"
			? visibleQueueRows.length
			: pane === "tasks"
				? visibleDefs.length
				: visibleWtRows.length;
	const paneSel = (pane: ListPaneId): PaneSelection =>
		pane === "queue" ? queueSel : pane === "tasks" ? tasksSel : wtSel;
```

5. Every existing read of `queueSel` / `tasksSel` / `wtSel` as a number becomes `.cursor`: in the `context` derivation (`visibleQueueRows[queueSel.cursor]`, `visibleDefs[tasksSel.cursor]`, `visibleWtRows[wtSel.cursor]`), in `openActionMenu` (`visibleQueueRows[queueSel.cursor]`), and in the three pane props (`selectedIndex={queueSel.cursor}` etc. — Task 3 replaces these props entirely, for now pass `.cursor`).

6. `dispatch` cases:

```ts
			case "move-selection": {
				const pane = ui.focus as ListPaneId;
				const next = clampIdx(
					paneSel(pane).cursor + action.delta,
					visibleCount(pane),
				);
				patchTab((s) => ({
					...s,
					selections: {
						...s.selections,
						[pane]: { cursor: next, anchor: null },
					},
					scrollOffset: 0,
				}));
				return;
			}
			case "extend-selection": {
				if (ui.focus === "detail") return;
				const pane = ui.focus as ListPaneId;
				const cur = paneSel(pane);
				const next = clampIdx(cur.cursor + action.delta, visibleCount(pane));
				patchTab((s) => ({
					...s,
					selections: {
						...s.selections,
						[pane]: { cursor: next, anchor: cur.anchor ?? cur.cursor },
					},
					scrollOffset: 0,
				}));
				return;
			}
```

7. Esc layering in `clear-search`:

```ts
			case "clear-search": {
				if (ui.focus === "detail") return;
				const pane = ui.focus;
				if (paneSel(pane).anchor !== null) {
					// first Esc clears the range; a second Esc clears the filter
					const cursor = paneSel(pane).cursor;
					patchTab((s) => ({
						...s,
						selections: { ...s.selections, [pane]: { cursor, anchor: null } },
					}));
					return;
				}
				patchTab((s) => ({
					...s,
					search: { ...s.search, [pane]: "" },
					selections: {
						...s.selections,
						[pane]: { cursor: 0, anchor: null },
					},
				}));
				return;
			}
```

8. Search-mode `setQuery` (inside `useInput`, `mode.kind === "search"` branch) — the reset now also clears the anchor:

```ts
			const setQuery = (fn: (cur: string) => string) =>
				patchTab((s) => ({
					...s,
					search: { ...s.search, [pane]: fn(s.search[pane]) },
					selections: {
						...s.selections,
						[pane]: { cursor: 0, anchor: null },
					},
				}));
```

- [ ] **Step 6: Run the full suite + typecheck**

Run: `cd packages/tui && pnpm test && pnpm typecheck`
Expected: everything PASSES (behavior is unchanged for `anchor === null`; existing app tests must stay green).

- [ ] **Step 7: Commit**

```bash
git add packages/tui/src/selectors.ts packages/tui/src/App.tsx packages/tui/src/__tests__/selectors.test.ts
git commit -m "feat(tui): anchor+cursor pane selection state with esc layering"
```

---

### Task 3: Rendering — range highlight, title counts, footer hint

**Files:**
- Modify: `packages/tui/src/selectors.ts` (`paneTitle` count param)
- Modify: `packages/tui/src/components/QueuePanel.tsx`, `packages/tui/src/components/TasksPane.tsx`, `packages/tui/src/components/WorktreesPane.tsx` (prop `selectedIndex: number` → `selection: PaneSelection`)
- Modify: `packages/tui/src/components/Footer.tsx` (`selectionCount` prop)
- Modify: `packages/tui/src/App.tsx` (pass new props)
- Test: `packages/tui/src/__tests__/selectors.test.ts`, `packages/tui/src/__tests__/components.test.tsx`, `packages/tui/src/__tests__/app.test.tsx`

**Interfaces:**
- Consumes: `PaneSelection`, `selectionRange` from Task 2.
- Produces: pane components accept `selection: PaneSelection`; `paneTitle(base, filter, active, selectedCount = 0)`; `Footer` accepts `selectionCount: number`.

- [ ] **Step 1: Write the failing tests**

`selectors.test.ts` — extend the existing `paneTitle` coverage:

```ts
describe("paneTitle — selection count", () => {
	it("appends the count when more than one row is selected", () => {
		expect(paneTitle("WORKTREES", "", false, 3)).toBe(
			"WORKTREES · 3 selected",
		);
	});

	it("composes count with an active filter", () => {
		expect(paneTitle("WORKTREES", "tmp", false, 2)).toBe(
			"WORKTREES · 2 selected /tmp",
		);
	});

	it("omits the count for single selection", () => {
		expect(paneTitle("QUEUE", "", false, 1)).toBe("QUEUE");
		expect(paneTitle("QUEUE", "", false, 0)).toBe("QUEUE");
	});
});
```

`components.test.tsx` — follow the file's existing render pattern for `WorktreesPane` (it renders with ink-testing-library and asserts on `lastFrame()`). Existing callers construct `WorktreeRow` fixtures; reuse that local fixture helper if one exists, otherwise inline rows as below. The inverse ANSI marker is `[7m`:

```ts
describe("WorktreesPane — range selection", () => {
	const rows = [
		{
			kind: "worktree" as const,
			name: "wt-a",
			path: "/wt/wt-a",
			branch: "wt-a",
			state: "free" as const,
			hasMainSession: false,
			queued: 0,
		},
		{
			kind: "worktree" as const,
			name: "wt-b",
			path: "/wt/wt-b",
			branch: "wt-b",
			state: "free" as const,
			hasMainSession: false,
			queued: 0,
		},
		{
			kind: "worktree" as const,
			name: "wt-c",
			path: "/wt/wt-c",
			branch: "wt-c",
			state: "free" as const,
			hasMainSession: false,
			queued: 0,
		},
	];

	it("renders every row in the range inverse and counts them in the title", () => {
		const { lastFrame } = render(
			<WorktreesPane
				rows={rows}
				selection={{ cursor: 1, anchor: 0 }}
				focused={true}
				capacity={5}
				filter=""
				filterActive={false}
			/>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).toContain("· 2 selected");
		// both range rows carry the inverse escape, the third does not
		expect(frame).toMatch(/\[7m[^\n]*wt-a/);
		expect(frame).toMatch(/\[7m[^\n]*wt-b/);
		expect(frame).not.toMatch(/\[7m[^\n]*wt-c/);
	});

	it("single selection renders exactly one inverse row and no count", () => {
		const { lastFrame } = render(
			<WorktreesPane
				rows={rows}
				selection={{ cursor: 1, anchor: null }}
				focused={true}
				capacity={5}
				filter=""
				filterActive={false}
			/>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).not.toContain("selected");
		expect(frame).not.toMatch(/\[7m[^\n]*wt-a/);
		expect(frame).toMatch(/\[7m[^\n]*wt-b/);
	});
});
```

`app.test.tsx` — integration (constants at top of file: add `const SHIFT_DOWN = "[1;2B";`):

```ts
	it("shift+down extends the queue selection and esc collapses it", async () => {
		const { store, server, sock, base } = await startServer();
		store.create({
			prompt: "first task",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.create({
			prompt: "second task",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		server.broadcast();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={createActions(sock)}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(250);
		app.stdin.write(SHIFT_DOWN);
		await wait(50);
		expect(app.lastFrame()).toContain("· 2 selected");
		expect(app.lastFrame()).toContain("bulk actions");
		app.stdin.write(ESC);
		await wait(50);
		expect(app.lastFrame()).not.toContain("selected");
	});

	it("plain arrow collapses the range; editing the filter clears it", async () => {
		const { store, server, sock, base } = await startServer();
		store.create({
			prompt: "first task",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.create({
			prompt: "second task",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.create({
			prompt: "third task",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		server.broadcast();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={createActions(sock)}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(250);
		app.stdin.write(SHIFT_DOWN);
		await wait(50);
		expect(app.lastFrame()).toContain("· 2 selected");
		app.stdin.write(DOWN); // plain movement collapses to single selection
		await wait(50);
		expect(app.lastFrame()).not.toContain("selected");
		app.stdin.write(SHIFT_DOWN);
		await wait(50);
		expect(app.lastFrame()).toContain("· 2 selected");
		app.stdin.write("/"); // open search
		app.stdin.write("t"); // editing the query clears the range
		await wait(50);
		expect(app.lastFrame()).not.toContain("selected");
	});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/tui && pnpm test -- selectors components app`
Expected: FAIL — `paneTitle` arity, `selection` prop type errors, missing "· 2 selected" output.

- [ ] **Step 3: Implement**

`selectors.ts` — `paneTitle` gains an optional count:

```ts
export function paneTitle(
	base: string,
	filter: string,
	active: boolean,
	selectedCount = 0,
): string {
	const title = selectedCount > 1 ? `${base} · ${selectedCount} selected` : base;
	if (!active && filter === "") return title;
	return `${title} /${filter}${active ? "█" : ""}`;
}
```

`WorktreesPane.tsx` (same shape for the other two panes):

```tsx
import { Text } from "ink";
import {
	type PaneSelection,
	paneTitle,
	selectionRange,
	type WorktreeRow,
	windowRows,
	worktreeDotColor,
} from "../selectors.js";
import { Pane } from "./Pane.js";

export function WorktreesPane({
	rows,
	selection,
	focused,
	capacity,
	filter,
	filterActive,
}: {
	rows: WorktreeRow[];
	selection: PaneSelection;
	focused: boolean;
	capacity: number;
	filter: string;
	filterActive: boolean;
}) {
	const { start, end } = selectionRange(selection);
	const selectedCount = rows.length === 0 ? 0 : end - start + 1;
	const { rows: windowed, offset } = windowRows(
		rows,
		selection.cursor,
		capacity,
	);
	return (
		<Pane
			title={paneTitle("WORKTREES", filter, filterActive, selectedCount)}
			focused={focused}
			flexGrow={1}
			flexBasis={0}
		>
			{rows.length === 0 ? (
				<Text dimColor>no worktrees</Text>
			) : (
				windowed.map((row, i) => (
					<Text
						key={`${row.kind}:${row.path}`}
						inverse={focused && offset + i >= start && offset + i <= end}
						wrap="truncate"
					>
						<Text color={worktreeDotColor(row.state)}>●</Text> {row.name}
						{row.hasMainSession ? <Text color="cyan"> ◆</Text> : null}
						{row.queued > 0 ? <Text dimColor> [{row.queued}]</Text> : null}
					</Text>
				))
			)}
		</Pane>
	);
}
```

Apply the identical transformation to `QueuePanel.tsx` (`QueuePane`, title `"QUEUE"`, keep `dimColor={row.kind === "archived"}`) and `TasksPane.tsx` (title `"TASKS"`): prop `selectedIndex: number` → `selection: PaneSelection`, window on `selection.cursor`, row `inverse={focused && offset + i >= start && offset + i <= end}`, title gains `selectedCount`.

`Footer.tsx`:

```tsx
export function Footer({
	focus,
	prefixArmed,
	statusLine,
	searching,
	selectionCount,
}: {
	focus: PaneId;
	prefixArmed: boolean;
	statusLine: string | null;
	searching: boolean;
	selectionCount: number;
}) {
	if (searching)
		return <Text dimColor>type to filter · [enter] apply · [esc] clear</Text>;
	if (statusLine !== null) return <Text color="red">{statusLine}</Text>;
	if (prefixArmed) return <Text inverse>{PREFIX_HINT}</Text>;
	if (selectionCount > 1)
		return (
			<Text dimColor>
				{selectionCount} selected · [a] bulk actions · [shift+↑↓] extend ·
				[esc] clear
			</Text>
		);
	return <Text dimColor>{HINTS[focus]}</Text>;
}
```

`App.tsx` — pass the new props:

```tsx
					<QueuePane
						rows={visibleQueueRows}
						selection={queueSel}
						...
					/>
					<TasksPane
						defs={visibleDefs}
						selection={tasksSel}
						...
					/>
					<WorktreesPane
						rows={visibleWtRows}
						selection={wtSel}
						...
					/>
```

and compute the footer count from the focused list pane:

```tsx
	const focusedRange =
		ui.focus === "detail" ? null : selectionRange(paneSel(ui.focus));
	const focusedSelectionCount =
		focusedRange === null || visibleCount(ui.focus as ListPaneId) === 0
			? 0
			: focusedRange.end - focusedRange.start + 1;
```

```tsx
			<Footer
				focus={ui.focus}
				prefixArmed={prefixArmed}
				statusLine={statusLine}
				searching={mode.kind === "search"}
				selectionCount={focusedSelectionCount}
			/>
```

Note: the shift+down keypress goes through `useInput` → `setStatusLine(null)` → `handleKey` → dispatch, same as any list key. No changes needed there.

- [ ] **Step 4: Run the full suite + typecheck**

Run: `cd packages/tui && pnpm test && pnpm typecheck`
Expected: PASS. If a pre-existing `components.test.tsx` or `app.test.tsx` case constructed panes with `selectedIndex`, update those call sites to `selection={{ cursor: N, anchor: null }}`.

- [ ] **Step 5: Commit**

```bash
git add packages/tui/src/selectors.ts packages/tui/src/components/QueuePanel.tsx packages/tui/src/components/TasksPane.tsx packages/tui/src/components/WorktreesPane.tsx packages/tui/src/components/Footer.tsx packages/tui/src/App.tsx packages/tui/src/__tests__/selectors.test.ts packages/tui/src/__tests__/components.test.tsx packages/tui/src/__tests__/app.test.tsx
git commit -m "feat(tui): render range selection with title counts and footer hint"
```

---

### Task 4: Bulk action menu — `buildBulkActions` + menu-open resolution

**Files:**
- Modify: `packages/tui/src/action-menu.ts`
- Modify: `packages/tui/src/App.tsx` (`MenuTarget` variants, `openBulkMenu`)
- Test: `packages/tui/src/__tests__/action-menu.test.ts`, `packages/tui/src/__tests__/app.test.tsx`

**Interfaces:**
- Consumes: `selectionRange`, `paneSel` from Task 2; pane row arrays.
- Produces:
  - `action-menu.ts`: `type BulkContext = { kind: "bulk-queue"; rerun: number; skip: number; total: number } | { kind: "bulk-tasks"; run: number; total: number } | { kind: "bulk-worktrees"; remove: number; total: number }` and `buildBulkActions(context: BulkContext): ActionItem[]` (reuses existing `ActionId` values `"rerun" | "skip" | "run" | "remove-worktree"`).
  - `App.tsx`: `MenuTarget` gains `{ kind: "bulk-queue"; rerunIds: string[]; skipIds: string[] } | { kind: "bulk-tasks"; defs: DefinitionSummary[] } | { kind: "bulk-worktrees"; names: string[] }`. Task 5 executes these.

- [ ] **Step 1: Write the failing action-menu tests**

Append to `packages/tui/src/__tests__/action-menu.test.ts` (extend the import with `buildBulkActions`):

```ts
describe("buildBulkActions", () => {
	it("bulk-queue: rerun and skip with eligible-of-total labels", () => {
		expect(
			buildBulkActions({ kind: "bulk-queue", rerun: 2, skip: 3, total: 5 }),
		).toEqual([
			{ id: "rerun", label: "Rerun (2 of 5)" },
			{ id: "skip", label: "Skip (3 of 5)" },
		]);
	});

	it("disables an action with zero eligible rows", () => {
		expect(
			buildBulkActions({ kind: "bulk-queue", rerun: 0, skip: 1, total: 4 }),
		).toEqual([
			{ id: "rerun", label: "Rerun (0 of 4)", disabled: "no eligible rows" },
			{ id: "skip", label: "Skip (1 of 4)" },
		]);
	});

	it("bulk-tasks: run only", () => {
		expect(buildBulkActions({ kind: "bulk-tasks", run: 1, total: 3 })).toEqual([
			{ id: "run", label: "Run (1 of 3)" },
		]);
	});

	it("bulk-worktrees: remove only", () => {
		expect(
			buildBulkActions({ kind: "bulk-worktrees", remove: 2, total: 4 }),
		).toEqual([{ id: "remove-worktree", label: "Remove worktrees… (2 of 4)" }]);
	});
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/tui && pnpm test -- action-menu`
Expected: FAIL — `buildBulkActions` not exported.

- [ ] **Step 3: Implement `buildBulkActions` in action-menu.ts**

```ts
export type BulkContext =
	| { kind: "bulk-queue"; rerun: number; skip: number; total: number }
	| { kind: "bulk-tasks"; run: number; total: number }
	| { kind: "bulk-worktrees"; remove: number; total: number };

function bulkItem(
	id: ActionId,
	verb: string,
	eligible: number,
	total: number,
): ActionItem {
	const label = `${verb} (${eligible} of ${total})`;
	return eligible > 0 ? { id, label } : { id, label, disabled: "no eligible rows" };
}

/**
 * Menu rows for a multi-row selection. Only actions that make sense over a
 * batch appear; per-row eligibility is resolved by the caller at menu-open
 * time (labels show `eligible of total`, zero-eligible rows render disabled).
 */
export function buildBulkActions(context: BulkContext): ActionItem[] {
	switch (context.kind) {
		case "bulk-queue":
			return [
				bulkItem("rerun", "Rerun", context.rerun, context.total),
				bulkItem("skip", "Skip", context.skip, context.total),
			];
		case "bulk-tasks":
			return [bulkItem("run", "Run", context.run, context.total)];
		case "bulk-worktrees":
			return [
				bulkItem(
					"remove-worktree",
					"Remove worktrees…",
					context.remove,
					context.total,
				),
			];
	}
}
```

- [ ] **Step 4: Run action-menu tests to verify they pass**

Run: `cd packages/tui && pnpm test -- action-menu`
Expected: PASS.

- [ ] **Step 5: Wire the bulk menu into App.tsx**

1. Extend the action-menu import: `import { type ActionId, type ActionItem, buildActions, buildBulkActions } from "./action-menu.js";`

2. Extend `MenuTarget`:

```ts
type MenuTarget =
	| { kind: "queue"; taskId: string }
	| { kind: "task"; def: DefinitionSummary }
	| { kind: "worktree"; name: string; path: string; branch: string | null }
	| { kind: "session"; path: string }
	| { kind: "bulk-queue"; rerunIds: string[]; skipIds: string[] }
	| { kind: "bulk-tasks"; defs: DefinitionSummary[] }
	| { kind: "bulk-worktrees"; names: string[] };
```

3. Add `openBulkMenu` above `openActionMenu` (after the `paneSel` helper is in scope — place both functions after the derived consts):

```ts
	// Bulk targets are frozen at menu-open time: the id/name lists captured here
	// are what executes, so daemon pushes that reshuffle rows mid-menu cannot
	// retarget the batch.
	const openBulkMenu = (
		pane: ListPaneId,
		start: number,
		end: number,
	): Mode | null => {
		const total = end - start + 1;
		if (pane === "queue") {
			const rows = visibleQueueRows.slice(start, end + 1);
			const statusById = new Map(
				(snapshot?.tasks ?? []).map((t) => [t.id, t.status]),
			);
			const live = rows.filter((r) => r.kind === "live");
			const rerunIds = live
				.filter((r) => {
					const s = statusById.get(r.id);
					return s === "failed" || s === "needs-input";
				})
				.map((r) => r.id);
			const skipIds = live
				.filter((r) => {
					const s = statusById.get(r.id);
					return s === "failed" || s === "needs-input" || s === "done";
				})
				.map((r) => r.id);
			return {
				kind: "action-menu",
				items: buildBulkActions({
					kind: "bulk-queue",
					rerun: rerunIds.length,
					skip: skipIds.length,
					total,
				}),
				index: 0,
				target: { kind: "bulk-queue", rerunIds, skipIds },
				title: `${total} selected`,
			};
		}
		if (pane === "tasks") {
			const rows = visibleDefs.slice(start, end + 1);
			const runnable = rows.filter((d) => d.args.length === 0);
			return {
				kind: "action-menu",
				items: buildBulkActions({
					kind: "bulk-tasks",
					run: runnable.length,
					total,
				}),
				index: 0,
				target: { kind: "bulk-tasks", defs: runnable },
				title: `${total} selected`,
			};
		}
		const rows = visibleWtRows.slice(start, end + 1);
		const removable = rows.filter(
			(r) => r.kind === "worktree" && r.state !== "busy",
		);
		return {
			kind: "action-menu",
			items: buildBulkActions({
				kind: "bulk-worktrees",
				remove: removable.length,
				total,
			}),
			index: 0,
			target: { kind: "bulk-worktrees", names: removable.map((r) => r.name) },
			title: `${total} selected`,
		};
	};
```

4. Branch at the top of `openActionMenu`:

```ts
	const openActionMenu = (): Mode | null => {
		const pane = ui.lastListPane;
		const { start, end } = selectionRange(paneSel(pane));
		if (end > start) return openBulkMenu(pane, start, end);
		// ...existing single-item logic unchanged
```

5. `runMenuAction` doesn't handle bulk targets yet — every existing case narrows on `target.kind`, so bulk targets fall through as no-ops. TypeScript stays green (the switch is over `ActionId`, not `MenuTarget`). Task 5 adds execution.

- [ ] **Step 6: Write the failing app integration test**

Append to `app.test.tsx`:

```ts
	it("a over a multi-row worktree selection opens the bulk menu with skip counts", async () => {
		const { server, sock, base } = await startServer({
			worktrees: [
				{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" },
				{ name: "wt-b", path: "/wt/wt-b", branch: "wt-b" },
			],
		});
		server.broadcast();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={createActions(sock)}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(250);
		// focus the worktrees pane: C-s j (queue→tasks), C-s j (tasks→worktrees)
		app.stdin.write(CTRL_S);
		app.stdin.write("j");
		app.stdin.write(CTRL_S);
		app.stdin.write("j");
		await wait(50);
		app.stdin.write(SHIFT_DOWN);
		await wait(50);
		app.stdin.write("a");
		await wait(50);
		expect(app.lastFrame()).toContain("2 selected");
		expect(app.lastFrame()).toContain("Remove worktrees… (2 of 2)");
	});
```

Note: `startServer({ worktrees })` feeds `resolverIO.listWorktrees`; both rows are `state: "free"` (no tasks), so both are eligible.

- [ ] **Step 7: Run the failing test, implement fixes if needed, then full suite**

Run: `cd packages/tui && pnpm test -- app`
Expected: the new test PASSES with Step 5's wiring (it was written after the implementation here because menu-open behavior needs the full wiring; if it fails, debug the wiring — do not weaken the assertions).
Then run: `pnpm test && pnpm typecheck`
Expected: all PASS.

- [ ] **Step 8: Commit**

```bash
git add packages/tui/src/action-menu.ts packages/tui/src/App.tsx packages/tui/src/__tests__/action-menu.test.ts packages/tui/src/__tests__/app.test.tsx
git commit -m "feat(tui): bulk action menu with per-action eligibility counts"
```

---

### Task 5: Bulk execution + confirm-bulk-remove modal

**Files:**
- Modify: `packages/tui/src/App.tsx` (`Mode` variant, `runBulk`, `runMenuAction` bulk branches, confirm modal + key handler)
- Test: `packages/tui/src/__tests__/app.test.tsx`

**Interfaces:**
- Consumes: bulk `MenuTarget` variants from Task 4; `Actions.retry/skip/runDefinition/removeWorktree` (all return `Promise<string | null>`, null = success).
- Produces: user-facing bulk execution; nothing downstream.

- [ ] **Step 1: Write the failing tests**

In `app.test.tsx`, first extend `fakeActions` so removals are recorded — add to `FakeCalls`:

```ts
	removeWorktree: [string, string][];
```

initialize `removeWorktree: []` in the `calls` literal, and replace the stub:

```ts
		removeWorktree: async (repo, name) => {
			calls.removeWorktree.push([repo, name]);
			return null;
		},
```

Then add the tests (these use `fakeActions()` instead of `createActions(sock)` so calls are observable):

```ts
	it("bulk remove confirms with the name list and removes each worktree", async () => {
		const { server, sock, base } = await startServer({
			worktrees: [
				{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" },
				{ name: "wt-b", path: "/wt/wt-b", branch: "wt-b" },
				{ name: "wt-c", path: "/wt/wt-c", branch: "wt-c" },
			],
		});
		server.broadcast();
		const { actions, calls } = fakeActions();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={actions}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(250);
		app.stdin.write(CTRL_S);
		app.stdin.write("j");
		app.stdin.write(CTRL_S);
		app.stdin.write("j");
		await wait(50);
		app.stdin.write(SHIFT_DOWN);
		app.stdin.write(SHIFT_DOWN);
		await wait(50);
		app.stdin.write("a");
		await wait(50);
		app.stdin.write("\r"); // Remove worktrees… is the only (enabled) row
		await wait(50);
		expect(app.lastFrame()).toContain("Remove 3 worktrees");
		expect(app.lastFrame()).toContain("wt-a");
		expect(app.lastFrame()).toContain("wt-c");
		app.stdin.write("y");
		await wait(100);
		expect(calls.removeWorktree).toEqual([
			["platform", "wt-a"],
			["platform", "wt-b"],
			["platform", "wt-c"],
		]);
		// range cleared after the bulk action
		expect(app.lastFrame()).not.toContain("3 selected");
	});

	it("n cancels bulk remove without removing anything", async () => {
		const { server, sock, base } = await startServer({
			worktrees: [
				{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" },
				{ name: "wt-b", path: "/wt/wt-b", branch: "wt-b" },
			],
		});
		server.broadcast();
		const { actions, calls } = fakeActions();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={actions}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(250);
		app.stdin.write(CTRL_S);
		app.stdin.write("j");
		app.stdin.write(CTRL_S);
		app.stdin.write("j");
		await wait(50);
		app.stdin.write(SHIFT_DOWN);
		await wait(50);
		app.stdin.write("a");
		await wait(50);
		app.stdin.write("\r");
		await wait(50);
		app.stdin.write("n");
		await wait(50);
		expect(calls.removeWorktree).toEqual([]);
	});

	it("bulk rerun retries only eligible queue tasks", async () => {
		const { store, server, sock, base } = await startServer();
		store.create({
			prompt: "will fail",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.create({
			prompt: "still queued",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		const [first] = store.list();
		if (first) store.update(first.id, { status: "failed" });
		server.broadcast();
		const { actions, calls } = fakeActions();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={actions}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(250);
		app.stdin.write(SHIFT_DOWN);
		await wait(50);
		app.stdin.write("a");
		await wait(50);
		expect(app.lastFrame()).toContain("Rerun (1 of 2)");
		app.stdin.write("\r");
		await wait(100);
		expect(calls.retry).toEqual([first?.id]);
		expect(app.lastFrame()).toContain("reran 1");
	});
```

Store API note: `QueueStore.update(id, patch)` and `QueueStore.list()` exist (`packages/core/src/store.ts:70,93`) — the code above is correct as written.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/tui && pnpm test -- app`
Expected: FAIL — enter on the bulk menu does nothing (no bulk branches in `runMenuAction`), no `confirm-bulk-remove` modal.

- [ ] **Step 3: Implement in App.tsx**

1. `Mode` gains:

```ts
	| { kind: "confirm-bulk-remove"; names: string[] }
```

2. Helpers (next to `act` / `invalidateDefs`):

```ts
	// Sequential on purpose: one daemon socket, deterministic order, and each
	// item's error is independently reported into the summary line.
	const runBulk = async <T,>(
		items: T[],
		verb: string,
		fn: (item: T) => Promise<string | null>,
	): Promise<void> => {
		let failed = 0;
		let firstError: string | null = null;
		for (const item of items) {
			const err = await fn(item);
			if (err !== null) {
				failed += 1;
				firstError = firstError ?? err;
			}
		}
		const ok = items.length - failed;
		setStatusLine(
			failed === 0 ? `${verb} ${ok}` : `${verb} ${ok}, ${failed} failed: ${firstError}`,
		);
	};

	const clearRange = (pane: ListPaneId) =>
		patchTab((s) => ({
			...s,
			selections: {
				...s.selections,
				[pane]: { cursor: s.selections[pane].cursor, anchor: null },
			},
		}));
```

3. `runMenuAction` — extend the existing cases (each case keeps its single-target line and gains a bulk line; `pane` for `clearRange` is `ui.lastListPane` captured before the async loop):

```ts
	const runMenuAction = (id: ActionId, target: MenuTarget) => {
		setMode({ kind: "list" });
		const pane = ui.lastListPane;
		switch (id) {
			case "rerun":
				if (target.kind === "queue") act(actions.retry(target.taskId));
				if (target.kind === "bulk-queue") {
					clearRange(pane);
					void runBulk(target.rerunIds, "reran", (taskId) =>
						actions.retry(taskId),
					);
				}
				return;
			case "skip":
				if (target.kind === "queue") act(actions.skip(target.taskId));
				if (target.kind === "bulk-queue") {
					clearRange(pane);
					void runBulk(target.skipIds, "skipped", (taskId) =>
						actions.skip(taskId),
					);
				}
				return;
```

`case "run"` gains:

```ts
				if (target.kind === "bulk-tasks") {
					clearRange(pane);
					void runBulk(target.defs, "started", (d) =>
						actions.runDefinition(d.repo, d.name, []),
					).then(() => invalidateDefs());
					return;
				}
```

(place it before the existing `if (target.kind !== "task") return;` line)

`case "remove-worktree"` gains:

```ts
				if (target.kind === "bulk-worktrees") {
					setMode({ kind: "confirm-bulk-remove", names: target.names });
				}
```

4. `useInput` — add next to the `confirm-remove` branch:

```ts
		if (mode.kind === "confirm-bulk-remove") {
			if (char === "y") {
				const repo = activeName;
				const names = mode.names;
				if (repo !== null) {
					clearRange("worktrees");
					void runBulk(names, "removed", (name) =>
						actions.removeWorktree(repo, name),
					);
				}
				setMode({ kind: "list" });
			} else if (char === "n" || char === "q" || key.escape) {
				setMode({ kind: "list" });
			}
			return;
		}
```

5. Modal JSX — add next to the `confirm-remove` modal:

```tsx
			{mode.kind === "confirm-bulk-remove" ? (
				<Modal
					title={`Remove ${mode.names.length} worktrees`}
					columns={columns}
					rows={rows}
					hint="y confirm · n/esc cancel"
				>
					<Text>
						{padLine(
							" discards uncommitted changes and deletes each local branch",
							modalInner,
						)}
					</Text>
					{mode.names.slice(0, 8).map((name) => (
						<Text key={name}>{padLine(`  ${name}`, modalInner)}</Text>
					))}
					{mode.names.length > 8 ? (
						<Text dimColor>
							{padLine(`  …and ${mode.names.length - 8} more`, modalInner)}
						</Text>
					) : null}
				</Modal>
			) : null}
```

Note the `runBulk` summary status line intentionally reuses `statusLine` (rendered red by Footer) — good enough for v1, matches the spec.

- [ ] **Step 4: Run the full suite + typecheck**

Run: `cd packages/tui && pnpm test && pnpm typecheck`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/tui/src/App.tsx packages/tui/src/__tests__/app.test.tsx
git commit -m "feat(tui): bulk execution with confirm modal for worktree removal"
```

---

### Task 6: Final verification sweep

**Files:**
- None new; fix anything found.

- [ ] **Step 1: Full workspace verification**

Run from the repo root:

```bash
pnpm -r test && pnpm -r typecheck && pnpm exec biome check packages/tui
```

Expected: all packages green, no lint diffs. Fix and amend into the relevant commit if anything surfaces.

- [ ] **Step 2: Manual smoke (optional but recommended)**

Run the TUI against a live daemon if one is available; verify: shift+↓ grows the range, plain ↓ collapses, Esc clears range then filter, `/`-filter + shift-select + `a` → `Remove worktrees… (N of N)` → confirm modal lists names → `n` cancels.

- [ ] **Step 3: Commit any fixes**

```bash
git add -A packages/tui && git commit -m "test(tui): bulk selection verification fixes"
```

(Skip if the tree is clean.)
