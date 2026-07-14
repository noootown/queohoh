# Cron Scheduler (Slice 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the daemon fire a task definition when its `cron:` expression comes due — for both discovery-backed defs (`pr-review`) and discovery-less defs (`slack-react-release-notes`).

**Architecture:** A pure `cron.ts` module in `@queohoh/core` (parse + match + due-window), consumed by a new `evaluateCrons()` step in the daemon `Engine.pass()`. An in-memory per-definition cursor owns fire-timing dedup; fires are fire-and-forget through the existing `instantiateDefinition` path with `source: "cron"`. Mirrors the existing pure `scheduler.ts` ↔ effectful `engine.ts` split.

**Tech Stack:** TypeScript (ESM, `.js` import specifiers), zod (already used), vitest. No new runtime dependency — the cron parser is hand-rolled per the repo's no-new-deps convention.

## Global Constraints

- **ESM imports use `.js` suffix** on relative paths (e.g. `from "./cron.js"`), even for `.ts` sources.
- **No new dependencies.** Hand-roll the cron parser.
- **Pure core, effectful daemon.** `cron.ts` has zero I/O and no wall-clock reads — time enters only as an explicit `nowMs`/`Date` parameter. Side effects live in `engine.ts`.
- **Local time.** Cron expressions evaluate against the operator's local timezone (`Date.getHours()` etc.), matching the migrated `30 15 * * *` = 15:30 local.
- **Never throw into the tick.** A malformed cron or a failed fire is logged via `console.error` and skipped; the rest of `pass()` continues.
- **No fire on boot / hot-reload.** The daemon self-restarts on rebuild (`reload.ts`); an unseen cron seeds its cursor to `now` without firing.
- **Test seam for time:** pure fns take `nowMs`; the engine reads time via an optional `now?: () => number` dep (defaults to `Date.now`), so tests are deterministic.
- Single-package test run: `pnpm --filter @queohoh/core exec vitest run <substr>` / `pnpm --filter @queohoh/daemon exec vitest run <substr>`. Full gate: `mise run check`.
- After editing `@queohoh/core`, run `pnpm --filter @queohoh/core build` before the daemon consumes the new exports.

---

## File Structure

- **Create** `packages/core/src/cron.ts` — pure cron parse/match/due. One responsibility: turn a cron string + a time into a fire/no-fire decision.
- **Create** `packages/core/src/__tests__/cron.test.ts` — unit tests for the pure module.
- **Modify** `packages/core/src/index.ts` — export the cron API from the barrel.
- **Modify** `packages/core/src/instantiate.ts` — coerce dedup to `none` for discovery-less cron fires.
- **Modify** `packages/core/src/__tests__/instantiate.test.ts` (or create if absent) — cover the coercion.
- **Modify** `packages/daemon/src/engine.ts` — add `now?` to `EngineDeps`; add `cronCursor`/`cronInFlight` fields, `cronDefinitions()`, `evaluateCrons()`, `fireCron()`; call `evaluateCrons()` in `pass()`.
- **Create** `packages/daemon/src/__tests__/engine-cron.test.ts` — engine firing behavior with injected `now`, fake `exec`, temp workspace.

---

### Task 1: Pure cron parser + matcher (`cron.ts`)

**Files:**
- Create: `packages/core/src/cron.ts`
- Create: `packages/core/src/__tests__/cron.test.ts`
- Modify: `packages/core/src/index.ts`

**Interfaces:**
- Produces:
  - `interface CronSpec { minute: Set<number>; hour: Set<number>; dom: Set<number>; month: Set<number>; dow: Set<number>; domRestricted: boolean; dowRestricted: boolean }`
  - `parseCron(expr: string): CronSpec` — throws `Error` on malformed input.
  - `cronMatches(spec: CronSpec, date: Date): boolean` — local-time, minute granularity.

- [ ] **Step 1: Write the failing test**

Create `packages/core/src/__tests__/cron.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { cronMatches, parseCron } from "../cron.js";

// A local-time Date at the given wall-clock parts.
const at = (y: number, mo: number, d: number, h: number, mi: number) =>
	new Date(y, mo - 1, d, h, mi, 0, 0);

describe("parseCron", () => {
	it("parses star fields", () => {
		const s = parseCron("* * * * *");
		expect(s.minute.size).toBe(60);
		expect(s.hour.size).toBe(24);
		expect(s.domRestricted).toBe(false);
		expect(s.dowRestricted).toBe(false);
	});

	it("parses number, list, range, and steps", () => {
		expect([...parseCron("0 * * * *").minute]).toEqual([0]);
		expect([...parseCron("1,15,30 * * * *").minute].sort((a, b) => a - b)).toEqual([1, 15, 30]);
		expect([...parseCron("0 9-11 * * *").hour].sort((a, b) => a - b)).toEqual([9, 10, 11]);
		expect([...parseCron("*/15 * * * *").minute].sort((a, b) => a - b)).toEqual([0, 15, 30, 45]);
		expect([...parseCron("0-30/10 * * * *").minute].sort((a, b) => a - b)).toEqual([0, 10, 20, 30]);
	});

	it("normalizes weekday 7 to Sunday (0)", () => {
		expect(parseCron("0 0 * * 7").dow.has(0)).toBe(true);
	});

	it("rejects wrong field count, out-of-range, and names", () => {
		expect(() => parseCron("* * * *")).toThrow();
		expect(() => parseCron("60 * * * *")).toThrow();
		expect(() => parseCron("0 0 * JAN *")).toThrow();
	});
});

describe("cronMatches", () => {
	it("matches top of every hour", () => {
		const s = parseCron("0 * * * *");
		expect(cronMatches(s, at(2026, 7, 14, 13, 0))).toBe(true);
		expect(cronMatches(s, at(2026, 7, 14, 13, 30))).toBe(false);
	});

	it("matches a daily local time", () => {
		const s = parseCron("30 15 * * *");
		expect(cronMatches(s, at(2026, 7, 14, 15, 30))).toBe(true);
		expect(cronMatches(s, at(2026, 7, 14, 15, 31))).toBe(false);
		expect(cronMatches(s, at(2026, 7, 14, 14, 30))).toBe(false);
	});

	it("matches weekdays with a dow range", () => {
		const s = parseCron("0 9 * * 1-5"); // Mon-Fri 09:00
		expect(cronMatches(s, at(2026, 7, 13, 9, 0))).toBe(true); // Mon
		expect(cronMatches(s, at(2026, 7, 18, 9, 0))).toBe(false); // Sat
	});

	it("uses OR-semantics when both dom and dow are restricted", () => {
		const s = parseCron("0 0 1 * 1"); // the 1st OR any Monday
		expect(cronMatches(s, at(2026, 7, 1, 0, 0))).toBe(true); // 1st (a Wed)
		expect(cronMatches(s, at(2026, 7, 13, 0, 0))).toBe(true); // a Monday
		expect(cronMatches(s, at(2026, 7, 14, 0, 0))).toBe(false); // neither
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @queohoh/core exec vitest run cron`
Expected: FAIL — `Cannot find module '../cron.js'`.

- [ ] **Step 3: Write minimal implementation**

Create `packages/core/src/cron.ts`:

```ts
/**
 * Pure 5-field cron: `minute hour day-of-month month day-of-week`. No I/O and no
 * wall-clock reads — time enters only as an explicit `Date`/`nowMs` argument, so
 * every function is deterministically testable. Evaluated in LOCAL time (the
 * migrated `30 15 * * *` means 15:30 in the operator's timezone).
 *
 * Supports `*`, a number, comma lists (`1,15`), ranges (`1-5`), and steps on a
 * star or range (`*/15`, `0-30/10`). Month/weekday NAMES are intentionally not
 * supported in slice 2 — a name throws rather than silently mis-scheduling.
 */
export interface CronSpec {
	minute: Set<number>; // 0-59
	hour: Set<number>; // 0-23
	dom: Set<number>; // 1-31
	month: Set<number>; // 1-12
	dow: Set<number>; // 0-6 (Sunday = 0)
	/** The day-of-month field was not `*` (drives dom/dow OR-semantics). */
	domRestricted: boolean;
	/** The day-of-week field was not `*`. */
	dowRestricted: boolean;
}

/** Expand one field into the set of integers it permits. `isDow` folds 7 → 0. */
function parseField(raw: string, lo: number, hi: number, isDow: boolean): Set<number> {
	const out = new Set<number>();
	for (const part of raw.split(",")) {
		const slash = part.indexOf("/");
		const rangePart = slash === -1 ? part : part.slice(0, slash);
		const stepStr = slash === -1 ? undefined : part.slice(slash + 1);
		const step = stepStr === undefined ? 1 : Number(stepStr);
		if (!Number.isInteger(step) || step < 1) {
			throw new Error(`cron: bad step in "${part}"`);
		}
		let start: number;
		let end: number;
		if (rangePart === "*") {
			start = lo;
			end = hi;
		} else if (rangePart.includes("-")) {
			const [a, b] = rangePart.split("-");
			start = Number(a);
			end = Number(b);
		} else {
			start = Number(rangePart);
			// A bare number with a step (`5/10`) means `5-hi/10` (standard cron).
			end = stepStr === undefined ? start : hi;
		}
		if (!Number.isInteger(start) || !Number.isInteger(end)) {
			throw new Error(`cron: non-numeric field "${part}"`);
		}
		for (let v = start; v <= end; v += step) {
			const n = isDow && v === 7 ? 0 : v;
			if (n < lo || n > hi) {
				throw new Error(`cron: value ${v} out of range [${lo}-${hi}] in "${part}"`);
			}
			out.add(n);
		}
	}
	if (out.size === 0) throw new Error(`cron: empty field "${raw}"`);
	return out;
}

export function parseCron(expr: string): CronSpec {
	const fields = expr.trim().split(/\s+/);
	if (fields.length !== 5) {
		throw new Error(`cron: expected 5 fields, got ${fields.length} in "${expr}"`);
	}
	for (const f of fields) {
		if (/[a-zA-Z]/.test(f)) {
			throw new Error(`cron: month/weekday names are not supported ("${f}")`);
		}
	}
	const [min, hr, dom, mon, dow] = fields;
	return {
		minute: parseField(min, 0, 59, false),
		hour: parseField(hr, 0, 23, false),
		dom: parseField(dom, 1, 31, false),
		month: parseField(mon, 1, 12, false),
		dow: parseField(dow, 0, 6, true),
		domRestricted: dom !== "*",
		dowRestricted: dow !== "*",
	};
}

/**
 * True iff the local minute represented by `date` satisfies every field.
 * Seconds/millis are ignored. dom/dow use OR-semantics when BOTH are restricted
 * (standard cron): a date matches if it satisfies either. When only one is
 * restricted, only that one constrains; when neither, the day always matches.
 */
export function cronMatches(spec: CronSpec, date: Date): boolean {
	if (!spec.minute.has(date.getMinutes())) return false;
	if (!spec.hour.has(date.getHours())) return false;
	if (!spec.month.has(date.getMonth() + 1)) return false;
	const domOk = spec.dom.has(date.getDate());
	const dowOk = spec.dow.has(date.getDay());
	if (spec.domRestricted && spec.dowRestricted) return domOk || dowOk;
	if (spec.domRestricted) return domOk;
	if (spec.dowRestricted) return dowOk;
	return true;
}
```

- [ ] **Step 4: Add barrel exports**

In `packages/core/src/index.ts`, add (keep the file's alphabetical-ish grouping; place near the other domain exports):

```ts
export type { CronSpec } from "./cron.js";
export { cronMatches, parseCron } from "./cron.js";
```

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm --filter @queohoh/core exec vitest run cron`
Expected: PASS (all cases in `cron.test.ts`).

- [ ] **Step 6: Commit**

```bash
git add packages/core/src/cron.ts packages/core/src/__tests__/cron.test.ts packages/core/src/index.ts
git commit -m "feat(core): pure 5-field cron parser and matcher"
```

---

### Task 2: Due-window function (`cronDue`)

**Files:**
- Modify: `packages/core/src/cron.ts`
- Modify: `packages/core/src/__tests__/cron.test.ts`
- Modify: `packages/core/src/index.ts`

**Interfaces:**
- Consumes: `CronSpec`, `cronMatches` (Task 1).
- Produces:
  - `const CRON_LOOKBACK_MINUTES: number` (= `48 * 60`).
  - `cronDue(spec: CronSpec, lastCheckedMs: number, nowMs: number): boolean` — true iff some whole minute `m` with `lastCheckedMs < m <= nowMs` matches. Fires at most once regardless of how many slots the window spans (catch-up-once); the scan is clamped to `CRON_LOOKBACK_MINUTES`.

- [ ] **Step 1: Write the failing test**

Append to `packages/core/src/__tests__/cron.test.ts`:

```ts
import { cronDue } from "../cron.js";

describe("cronDue", () => {
	const ms = (y: number, mo: number, d: number, h: number, mi: number) =>
		new Date(y, mo - 1, d, h, mi, 0, 0).getTime();

	it("is not due when no minute in the window matches", () => {
		const s = parseCron("0 * * * *"); // top of hour
		// window 13:01 -> 13:59, no :00 boundary crossed
		expect(cronDue(s, ms(2026, 7, 14, 13, 1), ms(2026, 7, 14, 13, 59))).toBe(false);
	});

	it("is due when the boundary is crossed", () => {
		const s = parseCron("0 * * * *");
		// window 13:59 -> 14:00 includes the 14:00 slot
		expect(cronDue(s, ms(2026, 7, 14, 13, 59), ms(2026, 7, 14, 14, 0))).toBe(true);
	});

	it("fires once when the window spans many matching slots (catch-up-once)", () => {
		const s = parseCron("0 * * * *"); // hourly
		// asleep 6 hours: still a single boolean true (caller fires once)
		expect(cronDue(s, ms(2026, 7, 14, 8, 0), ms(2026, 7, 14, 14, 0))).toBe(true);
	});

	it("returns false when now <= lastChecked", () => {
		const s = parseCron("* * * * *");
		expect(cronDue(s, ms(2026, 7, 14, 14, 0), ms(2026, 7, 14, 14, 0))).toBe(false);
	});

	it("still fires with a far-past cursor (clamped look-back)", () => {
		const s = parseCron("30 15 * * *"); // daily 15:30
		const now = ms(2026, 7, 14, 15, 31);
		const yearAgo = now - 365 * 24 * 60 * 60 * 1000;
		expect(cronDue(s, yearAgo, now)).toBe(true);
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @queohoh/core exec vitest run cron`
Expected: FAIL — `cronDue is not a function` / import error.

- [ ] **Step 3: Write minimal implementation**

Append to `packages/core/src/cron.ts`:

```ts
/**
 * Longest span `cronDue` scans backward, in minutes (48h). Bounds per-tick work
 * if a cursor is somehow far in the past; a match anywhere in the clamped window
 * still fires exactly once (catch-up-once).
 */
export const CRON_LOOKBACK_MINUTES = 48 * 60;

/**
 * True iff at least one whole minute `m` with `lastCheckedMs < m <= nowMs`
 * satisfies `spec` (local time). Walks minute boundaries from the clamped lower
 * bound to now and returns on the first match — so the caller fires ONCE even
 * when many matching slots were missed.
 */
export function cronDue(spec: CronSpec, lastCheckedMs: number, nowMs: number): boolean {
	if (nowMs <= lastCheckedMs) return false;
	const MIN = 60_000;
	// First whole-minute epoch strictly after lastChecked.
	let m = Math.floor(lastCheckedMs / MIN) * MIN + MIN;
	const floor = Math.floor(nowMs / MIN) * MIN - CRON_LOOKBACK_MINUTES * MIN;
	if (m < floor) m = floor;
	for (; m <= nowMs; m += MIN) {
		if (cronMatches(spec, new Date(m))) return true;
	}
	return false;
}
```

- [ ] **Step 4: Add barrel export**

In `packages/core/src/index.ts`, update the cron value export line to:

```ts
export { CRON_LOOKBACK_MINUTES, cronDue, cronMatches, parseCron } from "./cron.js";
```

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm --filter @queohoh/core exec vitest run cron`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/core/src/cron.ts packages/core/src/__tests__/cron.test.ts packages/core/src/index.ts
git commit -m "feat(core): cronDue due-window with catch-up-once semantics"
```

---

### Task 3: Dedup coercion for discovery-less cron fires (`instantiate.ts`)

**Files:**
- Modify: `packages/core/src/instantiate.ts` (the `filterNewItems` call inside `instantiateDefinition`)
- Test: `packages/core/src/__tests__/instantiate.test.ts` (create if absent)

**Interfaces:**
- Consumes: existing `instantiateDefinition(def, trigger, deps)`, `TaskSource` (`deps.source`), `def.discovery`, `def.dedup`.
- Produces: no signature change. Behavior change only: when `deps.source === "cron"` **and** `def.discovery` is null, dedup is forced to `"none"` so the static item key does not permanently block later fires.

- [ ] **Step 1: Write the failing test**

Create/append `packages/core/src/__tests__/instantiate.test.ts`. This uses an in-memory `QueueStore` on a temp dir and a no-op `exec`:

```ts
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { TaskDefinition } from "../definition.js";
import { instantiateDefinition } from "../instantiate.js";
import type { Exec } from "../resolver-io.js";
import { QueueStore } from "../store.js";

const noopExec: Exec = async () => ({ exitCode: 0, stdout: "", stderr: "" });

function discoverylessDef(dedup: TaskDefinition["dedup"]): TaskDefinition {
	return {
		name: "slack-react",
		repo: "workspace",
		description: null,
		discovery: null,
		cron: "30 15 * * *",
		args: [],
		dedup,
		worktree: "repo",
		preRun: null,
		postRun: null,
		verify: null,
		model: "sonnet",
		timeoutMs: 600_000,
		priority: "normal",
		prompt: "do the thing",
	};
}

function freshStore(): QueueStore {
	return new QueueStore(mkdtempSync(join(tmpdir(), "qoo-inst-")));
}

describe("instantiateDefinition — cron dedup coercion", () => {
	it("fires a discovery-less skip_seen def more than once when source is cron", async () => {
		const store = freshStore();
		const deps = {
			store,
			exec: noopExec,
			cwd: "/tmp",
			source: "cron" as const,
		};
		const def = discoverylessDef("skip_seen");
		const first = await instantiateDefinition(def, { mode: "args", values: [] }, deps);
		const second = await instantiateDefinition(def, { mode: "args", values: [] }, deps);
		expect(first).toHaveLength(1);
		expect(second).toHaveLength(1); // NOT deduped away
	});

	it("still dedups a discovery-less skip_seen def when source is NOT cron", async () => {
		const store = freshStore();
		const deps = {
			store,
			exec: noopExec,
			cwd: "/tmp",
			source: "tui" as const,
		};
		const def = discoverylessDef("skip_seen");
		const first = await instantiateDefinition(def, { mode: "args", values: [] }, deps);
		const second = await instantiateDefinition(def, { mode: "args", values: [] }, deps);
		expect(first).toHaveLength(1);
		expect(second).toHaveLength(0); // skip_seen blocks the repeat
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @queohoh/core exec vitest run instantiate`
Expected: FAIL — the first test's `second` is `[]` (length 0) because `skip_seen` currently dedups it.

- [ ] **Step 3: Write minimal implementation**

In `packages/core/src/instantiate.ts`, inside `instantiateDefinition`, replace the `filterNewItems` call. Change:

```ts
	const fresh = filterNewItems(items, {
		definition,
		itemKeyTemplate,
		mode: def.dedup,
		existing,
	});
```

to:

```ts
	// A discovery-less cron fire always yields the identical item (from arg
	// defaults / the static `adhoc` key), so `skip_seen` would drop every fire
	// after the first. Fire-timing dedup is owned by the engine's cron cursor, so
	// item dedup is meaningless here — force it off. Discovery-backed crons keep
	// their configured dedup (hourly pr-review must still skip PRs already queued).
	const dedupMode =
		deps.source === "cron" && !def.discovery ? "none" : def.dedup;
	const fresh = filterNewItems(items, {
		definition,
		itemKeyTemplate,
		mode: dedupMode,
		existing,
	});
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm --filter @queohoh/core exec vitest run instantiate`
Expected: PASS (both cases).

- [ ] **Step 5: Build core so the daemon consumes the change**

Run: `pnpm --filter @queohoh/core build`
Expected: exits 0.

- [ ] **Step 6: Commit**

```bash
git add packages/core/src/instantiate.ts packages/core/src/__tests__/instantiate.test.ts
git commit -m "feat(core): cron fires of discovery-less defs bypass item dedup"
```

---

### Task 4: Engine cron firing (`engine.ts`)

**Files:**
- Modify: `packages/daemon/src/engine.ts` — imports; `EngineDeps.now?`; fields `cronCursor`/`cronInFlight`; methods `cronDefinitions()`, `evaluateCrons()`, `fireCron()`; call `evaluateCrons()` in `pass()`.
- Create: `packages/daemon/src/__tests__/engine-cron.test.ts`

**Interfaces:**
- Consumes: `parseCron`, `cronDue` (Tasks 1-2); `instantiateDefinition`, `listDefinitions`, `globalWorkspaceDir`, `projectWorkspaceDir`, `loadProjectVars` (barrel); `TaskDefinition` type; existing `this.deps.store` / `this.deps.exec` / `this.deps.config` / `this.deps.onChange`.
- Produces: `EngineDeps` gains optional `now?: () => number` (defaults to `Date.now`). `pass()` calls `this.evaluateCrons()` after `registry.sweep()`, before building the task list for `schedule()`.

- [ ] **Step 1: Write the failing test**

Create `packages/daemon/src/__tests__/engine-cron.test.ts`. It builds a real temp workspace with one discovery-less cron def, drives ticks with an injected clock, and asserts fire behavior. (`Engine` is constructed directly with fakes, mirroring `engine.test.ts`.)

```ts
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Exec, GlobalConfig, ResolverIO } from "@queohoh/core";
import {
	makeRedactor,
	QueueStore,
	RunStore,
	SessionLineageStore,
	SessionRegistry,
} from "@queohoh/core";
import { describe, expect, it } from "vitest";
import { Engine } from "../engine.js";

const noopResolverIO: ResolverIO = {
	listWorktrees: async () => [],
	prBranch: async () => null,
	spawnWorktree: async (_r, name) => ({ name, path: "/tmp/wt", branch: name }),
	removeWorktree: async () => {},
};
const noopExec: Exec = async () => ({ exitCode: 0, stdout: "", stderr: "" });

// Build a workspace with `<workspace>/<project>/tasks/<name>/{config.yaml,prompt.md}`
// and return a GlobalConfig pointing at it. The project `path` is a throwaway repo
// dir (cron firing only enqueues; it does not resolve a worktree).
function workspaceWith(cronExpr: string) {
	const workspace = mkdtempSync(join(tmpdir(), "qoo-ws-"));
	const repoPath = mkdtempSync(join(tmpdir(), "qoo-repo-"));
	const taskDir = join(workspace, "demo", "tasks", "ping");
	mkdirSync(taskDir, { recursive: true });
	writeFileSync(
		join(taskDir, "config.yaml"),
		`description: ping\ncron: "${cronExpr}"\nworktree: repo\ndedup: none\nmodel: sonnet\n`,
	);
	writeFileSync(join(taskDir, "prompt.md"), "ping\n");
	// The `global` dir must exist for listDefinitions(globalWorkspaceDir) not to matter;
	// it returns [] when absent, which is fine.
	const config: GlobalConfig = {
		workspace,
		projects: [{ name: "demo", path: repoPath }],
		maxConcurrentTasks: 3,
		archiveAfterDays: 7,
		vars: {},
		models: {},
	};
	return { workspace, config };
}

function engineWith(config: GlobalConfig, now: () => number) {
	const stateDir = mkdtempSync(join(tmpdir(), "qoo-state-"));
	const store = new QueueStore(join(stateDir, "queue"));
	const engine = new Engine({
		store,
		runStore: new RunStore(join(stateDir, "runs")),
		registry: new SessionRegistry(join(stateDir, "sessions.json")),
		config,
		resolverIO: noopResolverIO,
		exec: noopExec,
		executeClaude: async () => ({
			exitCode: 0,
			timedOut: false,
			signal: null,
			sessionId: null,
			resultText: "",
			stderr: "",
			usage: { costUsd: null, turns: null, durationMs: null },
		}),
		executeVerify: async () => ({ exitCode: 0, timedOut: false, output: "" }),
		redact: makeRedactor(new Map()),
		lineage: new SessionLineageStore(join(stateDir, "lineage.json")),
		now,
	});
	return { engine, store };
}

// A local-time epoch for the given wall-clock.
const at = (y: number, mo: number, d: number, h: number, mi: number) =>
	new Date(y, mo - 1, d, h, mi, 0, 0).getTime();

describe("Engine cron firing", () => {
	it("does not fire on first sight (seeds the cursor)", async () => {
		const { config } = workspaceWith("30 15 * * *");
		const { engine, store } = engineWith(config, () => at(2026, 7, 14, 15, 30));
		await engine.tick(); // first sight: seed only
		expect(store.list().filter((t) => t.source === "cron")).toHaveLength(0);
	});

	it("fires once when a slot comes due after seeding", async () => {
		const { config } = workspaceWith("30 15 * * *");
		let clock = at(2026, 7, 14, 15, 29);
		const { engine, store } = engineWith(config, () => clock);
		await engine.tick(); // seed at 15:29
		clock = at(2026, 7, 14, 15, 30); // slot crosses
		await engine.tick(); // schedules the async fire
		// The fire is fire-and-forget; let microtasks settle, then a final tick.
		await new Promise((r) => setTimeout(r, 20));
		const cronTasks = store.list().filter((t) => t.source === "cron");
		expect(cronTasks).toHaveLength(1);
		expect(cronTasks[0].definition).toBe("demo/ping");
	});

	it("does not double-fire the same slot on a later tick", async () => {
		const { config } = workspaceWith("30 15 * * *");
		let clock = at(2026, 7, 14, 15, 29);
		const { engine, store } = engineWith(config, () => clock);
		await engine.tick();
		clock = at(2026, 7, 14, 15, 30);
		await engine.tick();
		await new Promise((r) => setTimeout(r, 20));
		clock = at(2026, 7, 14, 15, 31); // same 15:30 slot already fired
		await engine.tick();
		await new Promise((r) => setTimeout(r, 20));
		expect(store.list().filter((t) => t.source === "cron")).toHaveLength(1);
	});
});
```

> Verified against `packages/daemon/src/__tests__/engine.test.ts` `setup()`: `QueueStore(dir)`, `RunStore(dir)`, `SessionRegistry(path)`, `SessionLineageStore(path)`; `ResolverIO` requires `listWorktrees`, `prBranch`, `spawnWorktree` (returns `{name, path, branch}`), `removeWorktree`; `Exec` returns `{stdout, exitCode}`.

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @queohoh/daemon exec vitest run engine-cron`
Expected: FAIL — `now` is not an accepted dep and/or cron tasks are never created (seed test may pass vacuously, but the "fires once" test fails with length 0).

- [ ] **Step 3: Add `now?` to `EngineDeps`**

In `packages/daemon/src/engine.ts`, add to the `EngineDeps` interface (after `onChange?`):

```ts
	/** Wall-clock seam for cron evaluation; defaults to Date.now. Tests inject a
	 * controllable clock. */
	now?: () => number;
```

- [ ] **Step 4: Extend the imports**

In `packages/daemon/src/engine.ts`, add `TaskDefinition` to the `import type { … } from "@queohoh/core"` block, and add `cronDue`, `globalWorkspaceDir`, `instantiateDefinition`, `listDefinitions`, `parseCron` to the value `import { … } from "@queohoh/core"` block (keep alphabetical order).

- [ ] **Step 5: Add engine fields**

In `packages/daemon/src/engine.ts`, add alongside the other private fields (near `ticking`):

```ts
	// Cron fire-timing dedup: definition key ("repo/name") -> epoch ms of last
	// evaluation. In-memory by design — survives macOS sleep (process suspended,
	// not restarted); a true restart re-seeds to `now`, which is why nothing fires
	// on boot / hot-reload. See docs/superpowers/specs/2026-07-14-cron-scheduler-design.md.
	private cronCursor = new Map<string, number>();
	// Definitions whose async fire has not yet settled — guards a slow discovery
	// from being fired twice on consecutive ticks.
	private cronInFlight = new Set<string>();
```

- [ ] **Step 6: Add `cronDefinitions()`, `evaluateCrons()`, `fireCron()`**

In `packages/daemon/src/engine.ts`, add these private methods (place them after `pass()`):

```ts
	/** Every definition with a non-null `cron`, across all projects. Global defs
	 * are shadowed by a project-local def of the same name (matches the API's
	 * `definitions` enumeration). A project whose tasks dir is unreadable is
	 * skipped, not fatal. */
	private cronDefinitions(): TaskDefinition[] {
		const out: TaskDefinition[] = [];
		for (const project of this.deps.config.projects) {
			try {
				const byName = new Map<string, TaskDefinition>();
				for (const def of listDefinitions(
					globalWorkspaceDir(this.deps.config),
					project.name,
				)) {
					byName.set(def.name, def);
				}
				for (const def of listDefinitions(
					projectWorkspaceDir(this.deps.config, project.name),
					project.name,
				)) {
					byName.set(def.name, def);
				}
				for (const def of byName.values()) {
					if (def.cron) out.push(def);
				}
			} catch {
				// Unreadable tasks dir: skip this project's crons for this tick.
			}
		}
		return out;
	}

	/** Fire any cron definition whose schedule has come due since its cursor.
	 * Synchronous and cheap when nothing is due (an in-memory `cronDue` check);
	 * the expensive discovery shell-out only runs on a due slot, and even then
	 * off the pass via fire-and-forget `fireCron`. */
	private evaluateCrons(): void {
		const now = this.deps.now?.() ?? Date.now();
		const defs = this.cronDefinitions();
		const liveKeys = new Set(defs.map((d) => `${d.repo}/${d.name}`));
		// Prune vanished defs so a re-added def re-seeds (no surprise catch-up).
		for (const key of [...this.cronCursor.keys()]) {
			if (!liveKeys.has(key)) this.cronCursor.delete(key);
		}
		for (const def of defs) {
			const key = `${def.repo}/${def.name}`;
			const cursor = this.cronCursor.get(key);
			if (cursor === undefined) {
				this.cronCursor.set(key, now); // first sight: seed, never fire on boot
				continue;
			}
			if (this.cronInFlight.has(key)) continue;
			let due: boolean;
			try {
				due = cronDue(parseCron(def.cron as string), cursor, now);
			} catch (err) {
				console.error(
					`cron parse error for ${key}: ${err instanceof Error ? err.message : String(err)}`,
				);
				this.cronCursor.set(key, now); // don't re-log every tick
				continue;
			}
			if (!due) continue;
			this.cronCursor.set(key, now); // advance BEFORE the async fire (no double-fire)
			this.cronInFlight.add(key);
			void this.fireCron(def).finally(() => this.cronInFlight.delete(key));
		}
	}

	/** Enqueue a cron fire through the same path as the runDefinition API: run
	 * discovery (if any) and create tasks with source "cron". Never throws — a
	 * failure is logged and the cursor stays advanced (no retry-spam). */
	private async fireCron(def: TaskDefinition): Promise<void> {
		const { deps } = this;
		const project = deps.config.projects.find((p) => p.name === def.repo);
		if (!project) return;
		const projectDir = projectWorkspaceDir(deps.config, def.repo);
		try {
			const repoVars = loadProjectVars(projectDir);
			const created = await instantiateDefinition(
				def,
				def.discovery ? { mode: "discover" } : { mode: "args", values: [] },
				{
					store: deps.store,
					exec: deps.exec,
					cwd: projectDir,
					source: "cron",
					globalVars: {
						project: def.repo,
						repo_path: project.path,
						...deps.config.vars,
					},
					repoVars,
				},
			);
			if (created.length > 0) deps.onChange?.();
		} catch (err) {
			console.error(
				`cron fire failed for ${def.repo}/${def.name}: ${err instanceof Error ? err.message : String(err)}`,
			);
		}
	}
```

- [ ] **Step 7: Call `evaluateCrons()` from `pass()`**

In `packages/daemon/src/engine.ts`, in `pass()`, add the call right after `deps.registry.sweep();`:

```ts
		deps.registry.sweep();
		this.evaluateCrons();
```

- [ ] **Step 8: Run the cron engine tests**

Run: `pnpm --filter @queohoh/daemon exec vitest run engine-cron`
Expected: PASS (seed / fires-once / no-double-fire).

- [ ] **Step 9: Full gate**

Run: `mise run check`
Expected: build, `pnpm -r test`, `pnpm -r typecheck`, and `pnpm lint:ci` all pass. (Rust snapshot tests are unaffected — no wire-shape change.)

- [ ] **Step 10: Commit**

```bash
git add packages/daemon/src/engine.ts packages/daemon/src/__tests__/engine-cron.test.ts
git commit -m "feat(daemon): fire cron definitions on schedule from the engine tick"
```

---

## Self-Review

**Spec coverage:**
- Pure `cron.ts` parse/match/due → Tasks 1-2. ✓
- Local-time evaluation → `cronMatches` uses `Date` local getters; tested. ✓
- dom/dow OR-semantics → implemented + tested. ✓
- Catch-up-once + look-back clamp → `cronDue` + `CRON_LOOKBACK_MINUTES`; tested. ✓
- In-memory cursor, seed-on-boot, no-fire-on-hot-reload → engine fields + `evaluateCrons` seed branch; tested "no fire on first sight". ✓
- In-flight guard, advance-before-fire → `evaluateCrons`; tested "no double-fire". ✓
- Discovery vs discovery-less firing → `fireCron` trigger selection. ✓
- Discovery-less dedup coercion → Task 3; tested both directions. ✓
- Malformed cron logged & skipped → try/catch in `evaluateCrons`. ✓
- Def removed from config → cursor prune. ✓
- No new dependency; pure/effectful split; never-throw-into-tick → honored across tasks. ✓
- No tick-cadence change → `pass()` only gains one synchronous call. ✓

**Placeholder scan:** none — every step has concrete code/commands.

**Type consistency:** `CronSpec`, `parseCron`, `cronMatches`, `cronDue`, `CRON_LOOKBACK_MINUTES` names match across cron.ts, its exports, and engine usage. `EngineDeps.now` defaulted via `?? Date.now()`. `TaskDefinition.dedup` union includes `"none"`. Trigger reuses existing `{mode:"discover"}` / `{mode:"args"}` (no new mode) — consistent with `instantiate.ts`.

**Open verification note for the executor:** Task 4 Step 1 assumes the constructor shapes of `QueueStore`/`RunStore`/`SessionRegistry`/`SessionLineageStore`. Before running, read `packages/daemon/src/__tests__/engine.test.ts` `setup()` and copy its exact construction if it differs.
