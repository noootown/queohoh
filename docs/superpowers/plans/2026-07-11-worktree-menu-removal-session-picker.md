# Worktree Menu Removal + Session Picker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the worktrees-pane action menu (replaced by `r`/`g`/`x` pane hotkeys), merge the two "New task" options into one session-picker → multiline-prompt flow, and replace the per-lane `MainSessionStore` with a per-chain `SessionLineageStore`.

**Architecture:** TS daemon/core first (lineage store, worker resume rewrite, `listSessions` RPC), then Rust TUI (keymap cleanup, multiline input widget, AddTask rework, SessionPick modal, menu deletion). The daemon and TUI ship together from this repo, so wire-shape changes land on both sides within this plan.

**Tech Stack:** TypeScript (Node 22, vitest 4, zod 4, ESM with `.js` import specifiers), Rust (ratatui, crossterm, tokio; Elm-style `App::update → Update{dirty, cmds}` with side effects as `Cmd`s executed in `src/event.rs`).

**Spec:** `docs/superpowers/specs/2026-07-11-worktree-menu-removal-session-picker-design.md`

## Global Constraints

- Prompt editor semantics everywhere: **Enter submits, Shift+Enter inserts newline. No alt+enter anywhere** (remove the existing `|| alt` arm in `def_args.rs`).
- New IPC method wire name: `listSessions` (camelCase, matching `removeWorktree`/`runMeta`); params `{repo, worktree}`; response `{sessions: [{session_id, label, mtime_ms}]}` (snake_case fields, matching enqueue's `resume_session_id`).
- Session list limit: 5, sorted by jsonl mtime descending. Label fallback chain: queohoh run prompt first line → last `ai-title` record → first user prompt line → session-id 8-char prefix.
- `session: "main"` on the enqueue API is deprecated: accepted, warns, treated as fresh. MCP surface is untouched (it never had `session`).
- All new fs-reading code takes explicit base-dir params (no hardcoded `homedir()` deep in logic) so tests stay hermetic.
- Commit messages: conventional prefix, **no Co-Authored-By trailers**.
- Test commands: TS `cd packages/core && pnpm test` / `cd packages/daemon && pnpm test`, typecheck `pnpm -r typecheck`; Rust `cargo test -p qoo-tui`; full gate `mise run check`.
- TS state stores use the existing atomic-write pattern (`writeFileSync(tmp)` + `renameSync`) and `Object.create(null)` maps (prototype-chain safety).

---

### Task 1: `SessionLineageStore` (core)

**Files:**
- Create: `packages/core/src/session-lineage.ts`
- Create: `packages/core/src/__tests__/session-lineage.test.ts`
- Modify: `packages/core/src/index.ts` (add export; do NOT remove MainSessionStore exports yet — Task 3 does)

**Interfaces:**
- Produces: `class SessionLineageStore { constructor(filePath: string); recordFork(parent: string, child: string): void; tip(sessionId: string): string }` — Tasks 2, 3, 5 consume.

- [ ] **Step 1: Write the failing test**

```ts
// packages/core/src/__tests__/session-lineage.test.ts
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { SessionLineageStore } from "../session-lineage.js";

function storePath(): string {
	return join(mkdtempSync(join(tmpdir(), "lineage-")), "session-lineage.json");
}

describe("SessionLineageStore", () => {
	it("tip returns the id itself when no fork is recorded", () => {
		const s = new SessionLineageStore(storePath());
		expect(s.tip("sess-x")).toBe("sess-x");
	});

	it("follows multi-hop chains to the newest descendant", () => {
		const s = new SessionLineageStore(storePath());
		s.recordFork("x", "y");
		s.recordFork("y", "z");
		expect(s.tip("x")).toBe("z");
		expect(s.tip("y")).toBe("z");
	});

	it("keeps two chains independent", () => {
		const s = new SessionLineageStore(storePath());
		s.recordFork("x", "y");
		s.recordFork("q", "r");
		expect(s.tip("x")).toBe("y");
		expect(s.tip("q")).toBe("r");
	});

	it("is cycle-guarded", () => {
		const s = new SessionLineageStore(storePath());
		s.recordFork("x", "y");
		s.recordFork("y", "x");
		// Must terminate; returns the last id before revisiting.
		expect(["x", "y"]).toContain(s.tip("x"));
	});

	it("ignores self-forks", () => {
		const s = new SessionLineageStore(storePath());
		s.recordFork("x", "x");
		expect(s.tip("x")).toBe("x");
	});

	it("persists across instances and survives a corrupt file", () => {
		const path = storePath();
		const a = new SessionLineageStore(path);
		a.recordFork("x", "y");
		const b = new SessionLineageStore(path);
		expect(b.tip("x")).toBe("y");
		const { writeFileSync } = require("node:fs");
		writeFileSync(path, "not json");
		const c = new SessionLineageStore(path);
		expect(c.tip("x")).toBe("x");
	});

	it("is safe against prototype-chain keys", () => {
		const s = new SessionLineageStore(storePath());
		expect(s.tip("toString")).toBe("toString");
		expect(s.tip("__proto__")).toBe("__proto__");
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd packages/core && pnpm test -- session-lineage`
Expected: FAIL — cannot find module `../session-lineage.js`

- [ ] **Step 3: Implement**

```ts
// packages/core/src/session-lineage.ts
import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";

// Maps a resumed (parent) session id to the session id its run produced.
// Headless `claude -p --resume X` mints a NEW session id Y for the run, so a
// queued follow-up pinned to X must actually resume Y to see the earlier
// run's conversation. Following parent→child links resolves any pin to the
// tip of its own chain — unlike the old per-lane pointer, a task pinned to a
// different session in the same lane can never be hijacked onto this chain.
export class SessionLineageStore {
	private forks: Record<string, string> = Object.create(null);

	constructor(readonly filePath: string) {
		if (existsSync(filePath)) {
			try {
				const parsed = JSON.parse(readFileSync(filePath, "utf-8"));
				if (parsed && typeof parsed.forks === "object" && parsed.forks) {
					for (const [parent, child] of Object.entries(parsed.forks)) {
						if (typeof child === "string") this.forks[parent] = child;
					}
				}
			} catch {
				this.forks = Object.create(null);
			}
		}
	}

	private persist(): void {
		const tmp = `${this.filePath}.tmp`;
		writeFileSync(tmp, JSON.stringify({ forks: this.forks }, null, 2));
		renameSync(tmp, this.filePath);
	}

	recordFork(parent: string, child: string): void {
		if (parent === child) return;
		this.forks[parent] = child;
		this.persist();
	}

	/** Newest descendant of `sessionId` (itself when no fork recorded). */
	tip(sessionId: string): string {
		let current = sessionId;
		const seen = new Set<string>([current]);
		for (;;) {
			const next = this.forks[current];
			if (next === undefined || seen.has(next)) return current;
			seen.add(next);
			current = next;
		}
	}
}
```

Add to `packages/core/src/index.ts` next to the MainSessionStore export: `export { SessionLineageStore } from "./session-lineage.js";`

- [ ] **Step 4: Run test to verify it passes**

Run: `cd packages/core && pnpm test -- session-lineage`
Expected: PASS (7 tests)

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/session-lineage.ts packages/core/src/__tests__/session-lineage.test.ts packages/core/src/index.ts
git commit -m "feat(core): SessionLineageStore for per-chain resume resolution"
```

---

### Task 2: Worker resume via lineage

**Files:**
- Modify: `packages/core/src/worker.ts` (deps L17–39, resume block L152–168, advance block L227–237)
- Modify: `packages/core/src/__tests__/worker.test.ts` (resume/pointer tests L492–696)

**Interfaces:**
- Consumes: `SessionLineageStore.tip/recordFork` from Task 1.
- Produces: `WorkerDeps.lineage?: SessionLineageStore` (replaces `mainSessions?: MainSessionStore`). `task.session === "main"` no longer resolves a session — worker treats every task without `resumeSessionId` as fresh. Task 3 wires the daemon side.

- [ ] **Step 1: Write the failing tests**

In `worker.test.ts`, replace the MainSessionStore-based resume/pointer tests (the `mainStore()` L493–496 / `mainStoreAt()` L597–604 helpers and the assertions in L492–696) with lineage equivalents. Keep the existing `makeDeps`/`enqueue`/`withWorktree` helpers. New helper + core tests:

```ts
import { SessionLineageStore } from "../session-lineage.js";

function lineageStore(): SessionLineageStore {
	return new SessionLineageStore(join(mkdtempSync(join(tmpdir(), "lin-")), "lineage.json"));
}

it("pinned task resumes the tip of its pin's lineage", async () => {
	const lineage = lineageStore();
	lineage.recordFork("sess-x", "sess-y");
	let seenResume: string | undefined;
	const deps = makeDeps({
		lineage,
		executeClaude: async (opts) => {
			seenResume = opts.resumeSessionId;
			return { ...okResult, sessionId: "sess-z" };
		},
	});
	const task = withWorktree(deps.store, enqueue(deps.store, { resumeSessionId: "sess-x" }).id);
	await runTask(task.id, deps);
	expect(seenResume).toBe("sess-y");
	// The run recorded its own fork: y → z.
	expect(lineage.tip("sess-x")).toBe("sess-z");
});

it("fresh task resumes nothing and records no fork", async () => {
	const lineage = lineageStore();
	let seenResume: string | undefined = "sentinel";
	const deps = makeDeps({
		lineage,
		executeClaude: async (opts) => {
			seenResume = opts.resumeSessionId;
			return { ...okResult, sessionId: "sess-new" };
		},
	});
	const task = withWorktree(deps.store, enqueue(deps.store).id);
	await runTask(task.id, deps);
	expect(seenResume).toBeUndefined();
	expect(lineage.tip("sess-new")).toBe("sess-new");
});

it("session:'main' is treated as fresh (deprecated)", async () => {
	const lineage = lineageStore();
	let seenResume: string | undefined = "sentinel";
	const deps = makeDeps({
		lineage,
		executeClaude: async (opts) => {
			seenResume = opts.resumeSessionId;
			return okResult;
		},
	});
	const task = withWorktree(deps.store, enqueue(deps.store, { session: "main" }).id);
	await runTask(task.id, deps);
	expect(seenResume).toBeUndefined();
});

it("two chains in one lane stay isolated (the old lane-pointer hazard)", async () => {
	const lineage = lineageStore();
	const resumes: (string | undefined)[] = [];
	let n = 0;
	const deps = makeDeps({
		lineage,
		executeClaude: async (opts) => {
			resumes.push(opts.resumeSessionId);
			n += 1;
			return { ...okResult, sessionId: `out-${n}` };
		},
	});
	const a = withWorktree(deps.store, enqueue(deps.store, { resumeSessionId: "sess-1" }).id);
	const b = withWorktree(deps.store, enqueue(deps.store, { resumeSessionId: "sess-3" }).id);
	await runTask(a.id, deps);
	await runTask(b.id, deps);
	// A resumed its own pin; B was NOT hijacked onto A's chain.
	expect(resumes).toEqual(["sess-1", "sess-3"]);
	expect(lineage.tip("sess-1")).toBe("out-1");
	expect(lineage.tip("sess-3")).toBe("out-2");
});

it("chained tasks pinned to the same session stack", async () => {
	const lineage = lineageStore();
	const resumes: (string | undefined)[] = [];
	let n = 0;
	const deps = makeDeps({
		lineage,
		executeClaude: async (opts) => {
			resumes.push(opts.resumeSessionId);
			n += 1;
			return { ...okResult, sessionId: `hop-${n}` };
		},
	});
	const a = withWorktree(deps.store, enqueue(deps.store, { resumeSessionId: "sess-x" }).id);
	const b = withWorktree(deps.store, enqueue(deps.store, { resumeSessionId: "sess-x" }).id);
	await runTask(a.id, deps);
	await runTask(b.id, deps);
	expect(resumes).toEqual(["sess-x", "hop-1"]);
	expect(lineage.tip("sess-x")).toBe("hop-2");
});
```

Adapt the `enqueue` helper if it doesn't already accept `resumeSessionId`/`session` overrides (mirror how the existing L492–696 tests seeded pinned tasks). Match `okResult`'s actual shape from L15–23.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/core && pnpm test -- worker`
Expected: FAIL — `lineage` not a known dep / old pointer behavior asserted

- [ ] **Step 3: Implement**

In `worker.ts`:
1. Replace `import type { MainSessionStore } from "./main-sessions.js"` with `import type { SessionLineageStore } from "./session-lineage.js"`; in `WorkerDeps` replace `mainSessions?: MainSessionStore` with `lineage?: SessionLineageStore`.
2. Replace the resume-resolution block (L152–168) with:

```ts
	// Resume resolution at SPAWN time. A pinned task resumes the TIP of its
	// pin's lineage: each headless resume of X mints a new session id (the
	// fork is recorded after the run below), so following the chain makes
	// queued follow-ups stack — without hijacking a task pinned to a
	// different session in the same lane. `session: "main"` is deprecated
	// and intentionally resolves nothing (fresh).
	let resumeSessionId: string | undefined;
	if (task.resumeSessionId !== null) {
		resumeSessionId = deps.lineage?.tip(task.resumeSessionId) ?? task.resumeSessionId;
	}
```

3. Replace the pointer-advance block (L227–237) with:

```ts
	// Record the fork after any outcome (done OR failed): resuming
	// `resumeSessionId` produced `result.sessionId`, so future pins anywhere
	// on this chain resolve to the new tip. Fresh runs record nothing —
	// their session becomes a lineage root for future picks.
	if (
		resumeSessionId !== undefined &&
		deps.lineage &&
		result.sessionId !== null &&
		result.sessionId !== resumeSessionId
	) {
		deps.lineage.recordFork(resumeSessionId, result.sessionId);
	}
```

4. If `laneKey`'s only remaining worker.ts use was the resume block, keep it where still used (e.g. `registerWorker`) and delete only dead usage.

- [ ] **Step 4: Run tests**

Run: `cd packages/core && pnpm test -- worker` then `cd packages/core && pnpm test`
Expected: worker tests PASS. Note `main-sessions.test.ts` still passes (store still exists until Task 3). `pnpm -r typecheck` will fail at the daemon (engine.ts still passes `mainSessions` into runTask) — that is expected until Task 3; run only core typecheck: `cd packages/core && pnpm typecheck`.

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/worker.ts packages/core/src/__tests__/worker.test.ts
git commit -m "feat(core): worker resumes via session lineage, drops main-session pointer"
```

---

### Task 3: Daemon wiring — lineage store, main-session removal, enqueue deprecation

**Files:**
- Modify: `packages/daemon/src/paths.ts` (L20–21 area), `packages/daemon/src/daemon.ts` (L9, L20, L57, L75, L85), `packages/daemon/src/engine.ts` (L5, L65, L599), `packages/daemon/src/api.ts` (L8, L37–60 StateSnapshot, L62–76 ApiDeps, L100–117 snapshot, L223–273 enqueue)
- Delete: `packages/core/src/main-sessions.ts`, `packages/core/src/__tests__/main-sessions.test.ts`
- Modify: `packages/core/src/index.ts` (remove `MainSessionEntry`/`MainSessionStore` exports, L25–26)
- Modify tests: `packages/daemon/src/__tests__/engine.test.ts`, `api.test.ts`, `pr-review-shape.test.ts`, `paths.test.ts`, core `worker.test.ts` if any main-sessions import remains

**Interfaces:**
- Consumes: `SessionLineageStore` (Task 1), `WorkerDeps.lineage` (Task 2).
- Produces: `sessionLineagePath(state)` in paths.ts; `EngineDeps.lineage: SessionLineageStore`; `ApiDeps` without `mainSessions`; `StateSnapshot` **without** `mainSessions` (Rust side adapts in Task 11); enqueue accepts `session:"main"` but warns + stores `"fresh"`.

- [ ] **Step 1: Write failing tests first**

In `api.test.ts`: delete the `mainSessions` snapshot test (L304–314); update `setup()` (L25–121) to build `SessionLineageStore` instead of `MainSessionStore` and return it as `lineage`. Rewrite the "enqueue with session main sets the task session field" test (L374):

```ts
it("enqueue with session main is deprecated and stored as fresh", async () => {
	const { client, store } = await setup();
	await client.call("enqueue", { prompt: "p", repo: "platform", session: "main" });
	const task = store.list()[0];
	expect(task.session).toBe("fresh");
});
```

In `engine.test.ts`: replace the pointer-advance test (L106–122) with an end-to-end lineage test — seed a pinned task, run `engine.tick()` twice + `drain()`, assert `lineage.tip(<pin>)` equals the fake runner's emitted sessionId. In `pr-review-shape.test.ts` swap the store construction (L63). In `paths.test.ts` mirror the `mainSessionsPath` assertion for `sessionLineagePath` and delete the old one.

- [ ] **Step 2: Run to verify failure**

Run: `cd packages/daemon && pnpm test`
Expected: FAIL — setup still constructs MainSessionStore / snapshot shape mismatch

- [ ] **Step 3: Implement**

1. `paths.ts`: replace `mainSessionsPath` with `export const sessionLineagePath = (state: string): string => join(state, "daemon/session-lineage.json");`
2. `daemon.ts` L57: `const lineage = new SessionLineageStore(sessionLineagePath(state));` — thread into Engine deps (L75) and ApiServer deps (L85) as `lineage`; drop `mainSessions` everywhere.
3. `engine.ts`: `EngineDeps.mainSessions` → `lineage: SessionLineageStore` (L65); forward `lineage: deps.lineage` into `runTask` (L599).
4. `api.ts`: remove `mainSessions` from `ApiDeps` (L68), remove `mainSessions: this.deps.mainSessions.all()` from `snapshot()` (L114) and the `mainSessions` field from `StateSnapshot` (L52). In `enqueue` (L228), after parsing:

```ts
	const session = SessionModeSchema.default("fresh").parse(params.session);
	if (session === "main") {
		console.warn(
			"[queohoh] enqueue session:\"main\" is deprecated and treated as fresh — pass resume_session_id to pin a session",
		);
	}
```

   and pass `session: "fresh"` into `store.create` (L261–270) unconditionally.
5. Delete `packages/core/src/main-sessions.ts` + its test; remove exports from `core/src/index.ts`.

- [ ] **Step 4: Verify**

Run: `pnpm -r test && pnpm -r typecheck`
Expected: PASS across core + daemon. Grep gate: `grep -rn "MainSessionStore\|mainSessions" packages/` returns nothing.

- [ ] **Step 5: Commit**

```bash
git add -A packages
git commit -m "feat(daemon): replace MainSessionStore with SessionLineageStore, deprecate session:main"
```

---

### Task 4: Claude session discovery module (core)

**Files:**
- Create: `packages/core/src/claude-sessions.ts`
- Create: `packages/core/src/__tests__/claude-sessions.test.ts`
- Modify: `packages/core/src/index.ts` (export)

**Interfaces:**
- Produces: `encodeProjectDir(absPath: string): string`; `listClaudeSessions(claudeProjectsDir: string, worktreePath: string, limit?: number): ClaudeSessionInfo[]` where `ClaudeSessionInfo = { sessionId: string; mtimeMs: number; aiTitle: string | null; firstPrompt: string | null }`. Task 5 consumes.

- [ ] **Step 1: Write the failing test**

```ts
// packages/core/src/__tests__/claude-sessions.test.ts
import { mkdirSync, mkdtempSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { encodeProjectDir, listClaudeSessions } from "../claude-sessions.js";

describe("encodeProjectDir", () => {
	it("replaces slashes and dots with dashes", () => {
		expect(encodeProjectDir("/Users/n/Downloads/agent247/queohoh.action-menu")).toBe(
			"-Users-n-Downloads-agent247-queohoh-action-menu",
		);
	});
});

function writeSession(dir: string, id: string, lines: unknown[], mtimeSec: number): void {
	const path = join(dir, `${id}.jsonl`);
	writeFileSync(path, lines.map((l) => JSON.stringify(l)).join("\n"));
	utimesSync(path, mtimeSec, mtimeSec);
}

describe("listClaudeSessions", () => {
	it("lists newest-first, capped, with ai-title and first-prompt labels", () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		const wt = "/wt/demo";
		const dir = join(projects, encodeProjectDir(wt));
		mkdirSync(dir, { recursive: true });
		// Subdir = subagent transcripts; must be skipped.
		mkdirSync(join(dir, "aaaa-sub"), { recursive: true });
		writeSession(dir, "s-old", [{ type: "user", message: { content: "old prompt\nrest" } }], 1_000);
		writeSession(
			dir,
			"s-titled",
			[
				{ type: "user", message: { content: [{ type: "text", text: "first line here" }] } },
				{ type: "ai-title", aiTitle: "Stale title", sessionId: "s-titled" },
				{ type: "ai-title", aiTitle: "Fresh title", sessionId: "s-titled" },
			],
			3_000,
		);
		writeSession(dir, "s-untitled", [{ type: "user", message: { content: "just a prompt" } }], 2_000);

		const got = listClaudeSessions(projects, wt, 5);
		expect(got.map((s) => s.sessionId)).toEqual(["s-titled", "s-untitled", "s-old"]);
		expect(got[0].aiTitle).toBe("Fresh title"); // last ai-title wins
		expect(got[0].firstPrompt).toBe("first line here");
		expect(got[1].aiTitle).toBeNull();
		expect(got[1].firstPrompt).toBe("just a prompt");
		expect(got[2].firstPrompt).toBe("old prompt");
	});

	it("caps at limit", () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		const wt = "/wt/many";
		const dir = join(projects, encodeProjectDir(wt));
		mkdirSync(dir, { recursive: true });
		for (let i = 0; i < 8; i++) writeSession(dir, `s-${i}`, [{ type: "user", message: { content: `p${i}` } }], 1_000 + i);
		const got = listClaudeSessions(projects, wt, 5);
		expect(got).toHaveLength(5);
		expect(got[0].sessionId).toBe("s-7");
	});

	it("returns [] for a worktree with no session dir", () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		expect(listClaudeSessions(projects, "/nowhere", 5)).toEqual([]);
	});

	it("tolerates malformed jsonl lines", () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		const wt = "/wt/bad";
		const dir = join(projects, encodeProjectDir(wt));
		mkdirSync(dir, { recursive: true });
		const path = join(dir, "s-bad.jsonl");
		writeFileSync(path, 'not json\n{"type":"ai-title","aiTitle":"Ok","sessionId":"s-bad"}\n');
		const got = listClaudeSessions(projects, wt, 5);
		expect(got[0].aiTitle).toBe("Ok");
	});
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd packages/core && pnpm test -- claude-sessions`
Expected: FAIL — module not found

- [ ] **Step 3: Implement**

```ts
// packages/core/src/claude-sessions.ts
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

export interface ClaudeSessionInfo {
	sessionId: string;
	mtimeMs: number;
	aiTitle: string | null;
	firstPrompt: string | null;
}

/** Claude Code's project-dir encoding: the absolute cwd with every `/` and
 * `.` replaced by `-` (verified against ~/.claude/projects on disk). */
export function encodeProjectDir(absPath: string): string {
	return absPath.replace(/[/.]/g, "-");
}

export function listClaudeSessions(
	claudeProjectsDir: string,
	worktreePath: string,
	limit = 5,
): ClaudeSessionInfo[] {
	const dir = join(claudeProjectsDir, encodeProjectDir(worktreePath));
	let names: string[];
	try {
		names = readdirSync(dir);
	} catch {
		return [];
	}
	const files: { path: string; sessionId: string; mtimeMs: number }[] = [];
	for (const name of names) {
		if (!name.endsWith(".jsonl")) continue; // subdirs hold subagent transcripts
		const path = join(dir, name);
		try {
			const st = statSync(path);
			if (!st.isFile()) continue;
			files.push({ path, sessionId: name.slice(0, -".jsonl".length), mtimeMs: st.mtimeMs });
		} catch {
			// raced deletion — skip
		}
	}
	files.sort((a, b) => b.mtimeMs - a.mtimeMs);
	return files.slice(0, limit).map((f) => {
		const { aiTitle, firstPrompt } = extractLabels(f.path);
		return { sessionId: f.sessionId, mtimeMs: f.mtimeMs, aiTitle, firstPrompt };
	});
}

function extractLabels(path: string): { aiTitle: string | null; firstPrompt: string | null } {
	let aiTitle: string | null = null;
	let firstPrompt: string | null = null;
	let text: string;
	try {
		text = readFileSync(path, "utf-8");
	} catch {
		return { aiTitle, firstPrompt };
	}
	for (const line of text.split("\n")) {
		if (line === "") continue;
		// Cheap substring pre-filters keep JSON.parse off bulky records.
		const maybeTitle = line.includes('"ai-title"');
		const maybePrompt = firstPrompt === null && line.includes('"user"');
		if (!maybeTitle && !maybePrompt) continue;
		let record: unknown;
		try {
			record = JSON.parse(line);
		} catch {
			continue;
		}
		if (record === null || typeof record !== "object") continue;
		const r = record as Record<string, unknown>;
		if (r.type === "ai-title" && typeof r.aiTitle === "string" && r.aiTitle !== "") {
			aiTitle = r.aiTitle; // last one wins — titles refresh as the session evolves
		}
		if (firstPrompt === null && r.type === "user") {
			const content = (r.message as Record<string, unknown> | undefined)?.content;
			let textContent: string | null = null;
			if (typeof content === "string") textContent = content;
			else if (Array.isArray(content)) {
				const block = content.find(
					(c) => c !== null && typeof c === "object" && (c as Record<string, unknown>).type === "text",
				) as Record<string, unknown> | undefined;
				if (block && typeof block.text === "string") textContent = block.text;
			}
			if (textContent !== null) {
				const firstLine = textContent.split("\n", 1)[0].trim();
				if (firstLine !== "") firstPrompt = firstLine.slice(0, 120);
			}
		}
	}
	return { aiTitle, firstPrompt };
}
```

Export from `index.ts`: `export { encodeProjectDir, listClaudeSessions, type ClaudeSessionInfo } from "./claude-sessions.js";`

- [ ] **Step 4: Run tests**

Run: `cd packages/core && pnpm test -- claude-sessions`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/claude-sessions.ts packages/core/src/__tests__/claude-sessions.test.ts packages/core/src/index.ts
git commit -m "feat(core): Claude session discovery with ai-title/first-prompt labels"
```

---

### Task 5: `listSessions` RPC (daemon)

**Files:**
- Modify: `packages/daemon/src/api.ts` (dispatch switch — add case near `runMeta` L611; `ApiDeps` gains `claudeProjectsDir?: string`)
- Modify: `packages/daemon/src/engine.ts` (new public method `worktreeAbsPath`, factored from the `worktreePath` closure L622–629)
- Modify: `packages/core/src/run-store.ts` (new methods `listRunTaskIds()` and reuse of run meta to read `session_id` + task prompt)
- Modify: `packages/daemon/src/__tests__/api.test.ts`

**Interfaces:**
- Consumes: `listClaudeSessions`/`encodeProjectDir` (Task 4).
- Produces: RPC `listSessions {repo, worktree}` → `{sessions: [{session_id: string, label: string, mtime_ms: number}]}` (max 5, newest first). Rust Task 10 consumes this exact shape. `RunStore.listRunTaskIds(): string[]`. `Engine.worktreeAbsPath(repo: string, worktree: string): Promise<string | null>` (honors the `@repo`/REPO_SENTINEL → primary-checkout rule exactly as the existing closure does).

- [ ] **Step 1: Write the failing test**

In `api.test.ts`, extend `setup()` to accept `opts.claudeProjectsDir` and pass it into `ApiServer` deps. New test:

```ts
it("listSessions returns labeled sessions for a worktree", async () => {
	const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
	const wtPath = "/wt/platform.wt-a";
	const dir = join(projects, wtPath.replace(/[/.]/g, "-"));
	mkdirSync(dir, { recursive: true });
	writeFileSync(
		join(dir, "sess-titled.jsonl"),
		`${JSON.stringify({ type: "ai-title", aiTitle: "Fix the parser", sessionId: "sess-titled" })}\n`,
	);
	writeFileSync(
		join(dir, "sess-run.jsonl"),
		`${JSON.stringify({ type: "user", message: { content: "ignored" } })}\n`,
	);
	const { client, store, runStore } = await setup({
		claudeProjectsDir: projects,
		worktrees: [{ name: "platform.wt-a", path: wtPath, branch: "wt-a" }],
	});
	// Seed a run whose data.json maps sess-run → its task prompt.
	const task = store.create({ prompt: "queohoh task prompt\nsecond line", repo: "platform", ref: "worktree:platform.wt-a", source: "mcp" });
	runStore.writeSnapshot(task.id, { task, definition: null, resolved_worktree: wtPath, model: null });
	runStore.finishRun(task.id, { result: { ...okRunResult, sessionId: "sess-run" }, outcome: "done", reason: null }, (s) => s);

	const res = await client.call("listSessions", { repo: "platform", worktree: "platform.wt-a" });
	const byId = Object.fromEntries(res.sessions.map((s: { session_id: string; label: string }) => [s.session_id, s.label]));
	expect(byId["sess-run"]).toBe("queohoh task prompt"); // run prompt beats jsonl content
	expect(byId["sess-titled"]).toBe("Fix the parser");   // ai-title fallback
	expect(res.sessions.length).toBe(2);
});

it("listSessions errors on an unknown worktree", async () => {
	const { client } = await setup({ worktrees: [] });
	await expect(client.call("listSessions", { repo: "platform", worktree: "nope" })).rejects.toThrow();
});
```

Adapt `writeSnapshot`/`finishRun`/`okRunResult` argument shapes to the real signatures in `run-store.ts` (L27–51, L71–115) — the test must construct them exactly as production does.

- [ ] **Step 2: Run to verify failure**

Run: `cd packages/daemon && pnpm test -- api`
Expected: FAIL — `unknown method: listSessions`

- [ ] **Step 3: Implement**

1. `run-store.ts` — add:

```ts
	/** Task ids that have a run dir with data.json (for reverse session lookup). */
	listRunTaskIds(): string[] {
		let names: string[];
		try {
			names = readdirSync(this.runsDir);
		} catch {
			return [];
		}
		return names.filter((n) => existsSync(join(this.runsDir, n, "data.json")));
	}
```

   If `readRunMeta(taskId)` already returns the parsed `data.json` (including `session_id` and `.task.prompt`), reuse it; otherwise add a `readRunData(taskId): { session_id?: string | null; task?: { prompt?: string } } | null` that parses `data.json` leniently.

2. `engine.ts` — extract the closure at L622–629 into a public method, and have the closure delegate to it:

```ts
	async worktreeAbsPath(repo: string, worktree: string): Promise<string | null> {
		// Same resolution the worker uses (incl. the @repo sentinel → primary checkout).
		...existing closure body, parameterized...
	}
```

3. `api.ts` — `ApiDeps` gains `claudeProjectsDir?: string`; resolve once in the constructor: `this.claudeProjectsDir = deps.claudeProjectsDir ?? join(homedir(), ".claude", "projects");`. Add the dispatch case:

```ts
	case "listSessions": {
		const repo = z.string().parse(params.repo);
		const worktree = z.string().parse(params.worktree);
		const path = await this.deps.engine.worktreeAbsPath(repo, worktree);
		if (path === null) throw new Error(`unknown worktree: ${repo}/${worktree}`);
		const infos = listClaudeSessions(this.claudeProjectsDir, path, 5);
		const promptBySession = this.runPromptBySession();
		return {
			sessions: infos.map((s) => ({
				session_id: s.sessionId,
				mtime_ms: Math.round(s.mtimeMs),
				label:
					promptBySession.get(s.sessionId) ??
					s.aiTitle ??
					s.firstPrompt ??
					s.sessionId.slice(0, 8),
			})),
		};
	}
```

   with the helper:

```ts
	/** session_id → first line of the task prompt that produced it. */
	private runPromptBySession(): Map<string, string> {
		const map = new Map<string, string>();
		for (const taskId of this.deps.runStore.listRunTaskIds()) {
			const data = this.deps.runStore.readRunData(taskId);
			const sid = data?.session_id;
			const prompt = data?.task?.prompt;
			if (typeof sid === "string" && sid !== "" && typeof prompt === "string") {
				const firstLine = prompt.split("\n", 1)[0].trim();
				if (firstLine !== "") map.set(sid, firstLine.slice(0, 120));
			}
		}
		return map;
	}
```

4. `daemon.ts`: no change needed (default `claudeProjectsDir` resolves from homedir).

- [ ] **Step 4: Verify**

Run: `cd packages/daemon && pnpm test && pnpm -r typecheck`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/api.ts packages/daemon/src/engine.ts packages/core/src/run-store.ts packages/daemon/src/__tests__/api.test.ts
git commit -m "feat(daemon): listSessions RPC with run-prompt/ai-title label chain"
```

---

### Task 6: Remove ScrollEdge (g/G) from the TUI

**Files:**
- Modify: `crates/qoo-tui/src/keymap.rs` (variant L56–60, bindings L121–122, tests L282–288 `g_edges`, plus any other test referencing ScrollEdge)
- Modify: `crates/qoo-tui/src/app/actions.rs` (arm L140–151 — keep the `DetailScrollEdge` arm)
- Modify: `crates/qoo-tui/src/app/mouse.rs` (doc comment L68)
- Modify: `crates/qoo-tui/src/app/tests.rs` (L121–141)
- Modify: `crates/qoo-tui/src/view/help.rs` (row `:26` — reduce to Home/End only)

**Interfaces:**
- Produces: `g` and `G` unbound in `Mode::List` (Task 9 rebinds `g` on the worktrees pane). `AppAction::DetailScrollEdge` untouched.

- [ ] **Step 1: Update the keymap tests first**

Replace `g_edges` (keymap.rs L282–288) with:

```rust
    #[test]
    fn g_and_shift_g_are_unbound() {
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('g')), f), AppAction::None);
            assert_eq!(list_mode_action(&sk(KeyCode::Char('G')), f), AppAction::None);
        }
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p qoo-tui keymap`
Expected: FAIL — `g` still maps to ScrollEdge

- [ ] **Step 3: Implement**

Delete `AppAction::ScrollEdge` (keymap.rs L56–60 doc + variant), the two bindings (L121–122), the actions.rs `A::ScrollEdge` arm (L140–150; keep `A::DetailScrollEdge`), fix the mouse.rs doc comment, delete/adapt the `app/tests.rs:121–141` ScrollEdge tests, and update help.rs row 26 from `("g/G · Home/End", ...)` to `("Home/End", "detail pane top / bottom")`.

- [ ] **Step 4: Verify**

Run: `cargo test -p qoo-tui`
Expected: PASS; `grep -rn "ScrollEdge" crates/qoo-tui/src` shows only `DetailScrollEdge`.

- [ ] **Step 5: Commit**

```bash
git add crates/qoo-tui/src
git commit -m "feat(tui): remove vim g/G jump keys, freeing g for worktree goto"
```

---

### Task 7: `MultilineInput` widget

**Files:**
- Create: `crates/qoo-tui/src/view/multiline_input.rs` (state + editing + tests)
- Modify: `crates/qoo-tui/src/view/mod.rs` (declare module)
- Modify: `crates/qoo-tui/src/view/args_form.rs` (make `wrap_value_cursor` L375–406 and `caret_line` L584–598 `pub(crate)` if they aren't already)

**Interfaces:**
- Produces: `pub struct MultilineInput { pub text: String, pub cursor: usize }` with methods `insert_char(char)`, `insert_str(&str)`, `insert_newline()`, `backspace()`, `move_left()`, `move_right()`, `move_home()`, `move_end()`. Rendering consumers use `args_form::wrap_value_cursor(&input.text, input.cursor, width)` + `args_form::caret_line`. Tasks 8–10 consume.

- [ ] **Step 1: Write failing tests** (in the same file, `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ml(text: &str, cursor: usize) -> MultilineInput {
        MultilineInput { text: text.into(), cursor }
    }

    #[test]
    fn insert_char_at_caret_advances_cursor() {
        let mut m = ml("ab", 1);
        m.insert_char('x');
        assert_eq!(m.text, "axb");
        assert_eq!(m.cursor, 2);
    }

    #[test]
    fn insert_newline_is_a_char_insert() {
        let mut m = ml("ab", 2);
        m.insert_newline();
        assert_eq!(m.text, "ab\n");
        assert_eq!(m.cursor, 3);
    }

    #[test]
    fn backspace_removes_char_before_caret() {
        let mut m = ml("abc", 2);
        m.backspace();
        assert_eq!(m.text, "ac");
        assert_eq!(m.cursor, 1);
        let mut at_start = ml("abc", 0);
        at_start.backspace();
        assert_eq!(at_start.text, "abc");
    }

    #[test]
    fn moves_clamp_at_edges_and_are_char_based() {
        let mut m = ml("héllo", 0);
        m.move_left();
        assert_eq!(m.cursor, 0);
        m.move_end();
        assert_eq!(m.cursor, 5); // chars, not bytes
        m.move_right();
        assert_eq!(m.cursor, 5);
        m.move_home();
        assert_eq!(m.cursor, 0);
    }

    #[test]
    fn insert_str_pastes_multichar_including_newlines() {
        let mut m = ml("ad", 1);
        m.insert_str("b\nc");
        assert_eq!(m.text, "ab\ncd");
        assert_eq!(m.cursor, 4);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p qoo-tui multiline`
Expected: FAIL — module missing

- [ ] **Step 3: Implement**

```rust
//! Minimal multiline text-entry state shared by every text modal (the
//! app-wide input unification seam). One string + a char-index caret;
//! rendering reuses `args_form::wrap_value_cursor` / `caret_line` so all
//! inputs look identical. Editing is char-based (the caret is a char index,
//! converted to a byte offset at the edit point).

#[derive(Debug, Clone, Default)]
pub struct MultilineInput {
    pub text: String,
    pub cursor: usize,
}

impl MultilineInput {
    fn byte_at(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    pub fn insert_char(&mut self, c: char) {
        let at = self.byte_at(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        let at = self.byte_at(self.cursor);
        self.text.insert_str(at, s);
        self.cursor += s.chars().count();
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_at(self.cursor - 1);
        let end = self.byte_at(self.cursor);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.chars().count());
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text.chars().count();
    }
}
```

Declare in `view/mod.rs` (`pub mod multiline_input;`) and widen `wrap_value_cursor`/`caret_line` visibility in args_form.rs to `pub(crate)` if needed.

- [ ] **Step 4: Verify**

Run: `cargo test -p qoo-tui multiline`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/qoo-tui/src/view
git commit -m "feat(tui): MultilineInput widget for unified text entry"
```

---

### Task 8: AddTask rework — resume pin, multiline editor, no SessionMode, no alt+enter

**Files:**
- Modify: `crates/qoo-tui/src/app/mode.rs` (delete `SessionMode` L155–160; rework `AddTask` L205)
- Modify: `crates/qoo-tui/src/app/update.rs` (AddTask arm L188–232; bracketed paste L351–359)
- Modify: `crates/qoo-tui/src/app/actions.rs` (Create arm L193–211)
- Modify: `crates/qoo-tui/src/app/menus.rs` (TaskFresh/TaskMain arms L308–314 — both construct the new fresh AddTask; full deletion happens in Task 9)
- Modify: `crates/qoo-tui/src/app/def_args.rs` (L236: `Enter if shift || alt` → `Enter if shift`)
- Modify: `crates/qoo-tui/src/view/mod.rs` (AddTask render dispatch L203–220, session label L210–211)
- Modify: `crates/qoo-tui/src/view/modal.rs` (new `render_prompt_modal` replacing `render_input_modal` for AddTask; keep `render_input_modal` for CreateWorktree)
- Modify tests: `crates/qoo-tui/src/app/input_modal_tests.rs` (SessionMode refs at :56,77,98,109,122,142,193,211), `menu_flow_tests.rs:321,426,546`, `def_pick_tests.rs:568–570`

**Interfaces:**
- Consumes: `MultilineInput` (Task 7); daemon enqueue already accepts `resume_session_id` (verified — `api.ts` L231–235).
- Produces: `Mode::AddTask { worktree: Option<String>, resume_session_id: Option<String>, resume_label: Option<String>, editor: MultilineInput }`. Enqueue params: `{prompt, repo}` + optional `"worktree"` + optional `"resume_session_id"`; the `"session"` field is **no longer sent**. Task 9/10 construct this mode variant.

- [ ] **Step 1: Rewrite the AddTask tests first**

In `input_modal_tests.rs`, replace SessionMode-based assertions. Core new tests (adapt helpers from the existing file):

```rust
#[test]
fn add_task_enter_submits_prompt_without_session_field() {
    let mut a = app_with(worktree_snapshot());
    a.mode = Mode::AddTask {
        worktree: Some("platform.wt-a".into()),
        resume_session_id: None,
        resume_label: None,
        editor: crate::view::multiline_input::MultilineInput::default(),
    };
    for c in "do it".chars() { a.update(key(c)); }
    let up = a.update(enter());
    assert!(matches!(a.mode, Mode::List));
    assert!(matches!(&up.cmds[..], [Cmd::Rpc { call, .. }]
        if call.method == "enqueue"
        && call.params == serde_json::json!({"prompt": "do it", "repo": "platform", "worktree": "platform.wt-a"})));
}

#[test]
fn add_task_with_pin_sends_resume_session_id() {
    let mut a = app_with(worktree_snapshot());
    a.mode = Mode::AddTask {
        worktree: Some("platform.wt-a".into()),
        resume_session_id: Some("sess-1".into()),
        resume_label: Some("Fix the parser".into()),
        editor: crate::view::multiline_input::MultilineInput::default(),
    };
    a.update(key('x'));
    let up = a.update(enter());
    assert!(matches!(&up.cmds[..], [Cmd::Rpc { call, .. }]
        if call.params["resume_session_id"] == serde_json::json!("sess-1")));
}

#[test]
fn shift_enter_inserts_newline_instead_of_submitting() {
    let mut a = app_with(worktree_snapshot());
    a.mode = Mode::AddTask {
        worktree: None, resume_session_id: None, resume_label: None,
        editor: crate::view::multiline_input::MultilineInput::default(),
    };
    a.update(key('a'));
    a.update(shift_enter()); // KeyEvent::new(Enter, SHIFT)
    a.update(key('b'));
    match &a.mode {
        Mode::AddTask { editor, .. } => assert_eq!(editor.text, "a\nb"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn alt_enter_does_not_insert_newline_in_def_args_form() {
    // DefArgs: shift+enter still newlines, alt+enter no longer does.
    // Build a DefArgs form on a free-text field (mirror existing def_args tests),
    // send KeyEvent::new(Enter, ALT), assert the value contains no '\n'
    // and the form attempted submit/dropdown instead.
}
```

Fill the alt+enter DefArgs test in full by mirroring the existing DefArgs test setup in the file/def_pick_tests.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p qoo-tui input_modal`
Expected: FAIL — AddTask shape mismatch

- [ ] **Step 3: Implement**

1. `mode.rs`: delete `SessionMode`; new variant:

```rust
    AddTask {
        worktree: Option<String>,
        /// Pin: resume this session (lineage-resolved at spawn). None = fresh.
        resume_session_id: Option<String>,
        /// Human label of the picked session, for the modal title.
        resume_label: Option<String>,
        editor: crate::view::multiline_input::MultilineInput,
    },
```

2. `update.rs` AddTask arm (replace L188–232):

```rust
Event::Key(k)
    if k.kind == KeyEventKind::Press && matches!(self.mode, Mode::AddTask { .. }) =>
{
    use crossterm::event::{KeyCode::*, KeyModifiers};
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    match k.code {
        // Newline chord first — must win over the plain-Enter submit arm.
        Enter if shift => {
            if let Mode::AddTask { editor, .. } = &mut self.mode {
                editor.insert_newline();
            }
            Update { dirty: true, cmds: vec![] }
        }
        Enter => {
            let (prompt, resume, worktree) =
                if let Mode::AddTask { worktree, resume_session_id, editor, .. } = &self.mode {
                    (editor.text.clone(), resume_session_id.clone(), worktree.clone())
                } else { unreachable!() };
            let repo = match self.active_repo() {
                Some(r) => r,
                None => { self.mode = Mode::List; return Update { dirty: true, cmds: vec![] }; }
            };
            let mut params = serde_json::json!({ "prompt": prompt, "repo": repo });
            if let Some(w) = worktree {
                params["worktree"] = serde_json::Value::String(w);
            }
            if let Some(sid) = resume {
                params["resume_session_id"] = serde_json::Value::String(sid);
            }
            self.mode = Mode::List;
            let cmd = self.dispatch_rpc("enqueue task", "enqueue", params, RpcOpts::default());
            Update { dirty: true, cmds: vec![cmd] }
        }
        Esc => { self.mode = Mode::List; Update { dirty: true, cmds: vec![] } }
        Backspace => { if let Mode::AddTask { editor, .. } = &mut self.mode { editor.backspace(); } Update { dirty: true, cmds: vec![] } }
        Left => { if let Mode::AddTask { editor, .. } = &mut self.mode { editor.move_left(); } Update { dirty: true, cmds: vec![] } }
        Right => { if let Mode::AddTask { editor, .. } = &mut self.mode { editor.move_right(); } Update { dirty: true, cmds: vec![] } }
        Home => { if let Mode::AddTask { editor, .. } = &mut self.mode { editor.move_home(); } Update { dirty: true, cmds: vec![] } }
        End => { if let Mode::AddTask { editor, .. } = &mut self.mode { editor.move_end(); } Update { dirty: true, cmds: vec![] } }
        Char(c) => { if let Mode::AddTask { editor, .. } = &mut self.mode { editor.insert_char(c); } Update { dirty: true, cmds: vec![] } }
        _ => Update { dirty: false, cmds: vec![] },
    }
}
```

   Bracketed paste (L351–359): route to `editor.insert_str(&pasted)` when in AddTask.
3. `actions.rs` Create arm: `Mode::AddTask { worktree: None, resume_session_id: None, resume_label: None, editor: Default::default() }`.
4. `menus.rs` L308–314: both `M::TaskFresh` and `M::TaskMain` construct the same fresh AddTask (interim until Task 9 deletes them).
5. `def_args.rs` L236: `Enter if shift || alt` → `Enter if shift`. Remove the now-unused `alt` binding at L218 if nothing else reads it (check L272 `Char(c) if !ctrl && !alt` — keep `alt` if still used there).
6. `view/mod.rs` + `view/modal.rs`: replace the AddTask `render_input_modal` call with a new `render_prompt_modal(f, hits, palette, title, &editor)` that uses `modal_frame`, renders the wrapped multiline body via `args_form::wrap_value_cursor` + `caret_line`, and a hint line `enter submit · shift+enter newline · esc cancel`. Title: `New task — {worktree|repo}` or `New task — resume: {resume_label} — {worktree}` when pinned. Delete the fresh/main title branch (view/mod.rs L210–211).

- [ ] **Step 4: Verify**

Run: `cargo test -p qoo-tui`
Expected: PASS. Grep gate: `grep -rn "SessionMode" crates/` → only test-free remnants must be gone entirely; `grep -rn '"session"' crates/qoo-tui/src` → no enqueue param.

- [ ] **Step 5: Commit**

```bash
git add crates/qoo-tui/src
git commit -m "feat(tui): multiline AddTask with resume pin; drop SessionMode and alt+enter"
```

---

### Task 9: Worktrees pane hotkeys r/g/x; delete the worktree action menu

**Files:**
- Modify: `crates/qoo-tui/src/hit.rs` (PaneButton variants L11–25, `pane_buttons` L27–42)
- Modify: `crates/qoo-tui/src/keymap.rs` (AppAction variants + `r`/`g`/`x` gating L102–116)
- Modify: `crates/qoo-tui/src/app/actions.rs` (three new action handlers)
- Modify: `crates/qoo-tui/src/app/menus.rs` (`open_action_menu` worktrees arm L76; delete `M::TaskFresh`/`M::TaskMain` arms L308–314)
- Modify: `crates/qoo-tui/src/action_menu.rs` (delete `worktree_menu` L110–159, `TaskFresh`/`TaskMain` variants L25–27, `TASK_FRESH_DESC`/`TASK_MAIN_DESC`/`OPEN_TMUX_DESC`/`REMOVE_DESC` where now unused, and their builder/filter tests)
- Modify: `crates/qoo-tui/src/app/mouse.rs` (chip routing L415–460)
- Modify: `crates/qoo-tui/src/view/panes.rs` (`button_chip` L83–109), `crates/qoo-tui/src/view/theme.rs` (chip label constants L59–65), `crates/qoo-tui/src/view/help.rs` (rows 17–22)
- Modify tests: `menu_flow_tests.rs` (worktree menu flows :303–475, click tests :507,552), `app/tests.rs` PaneButton chip tests (:840–926), `view/panes.rs` tests (:1232–1367), keymap tests

**Interfaces:**
- Consumes: reworked `Mode::AddTask` (Task 8), existing `Cmd::OpenTmux`, `Mode::ConfirmRemove`.
- Produces: `AppAction::NewTaskOnWorktree`, `AppAction::GotoWorktree`, `AppAction::RemoveSelectedWorktree`; `PaneButton::Goto`, `PaneButton::Remove`; worktrees chip set `&[Run, Goto, Remove, Tasks, Create, Collapse]`. In this task `r` opens `Mode::AddTask` directly (fresh, worktree-targeted); Task 10 reroutes it through SessionPick.

- [ ] **Step 1: Write failing tests**

keymap tests:

```rust
#[test]
fn worktree_pane_r_g_x_verbs() {
    assert_eq!(list_mode_action(&k(KeyCode::Char('r')), PaneId::Worktrees), AppAction::NewTaskOnWorktree);
    assert_eq!(list_mode_action(&k(KeyCode::Char('g')), PaneId::Worktrees), AppAction::GotoWorktree);
    assert_eq!(list_mode_action(&k(KeyCode::Char('x')), PaneId::Worktrees), AppAction::RemoveSelectedWorktree);
    // g inert off-worktrees; x still cancels on queue; a inert on worktrees now.
    assert_eq!(list_mode_action(&k(KeyCode::Char('g')), PaneId::Queue), AppAction::None);
    assert_eq!(list_mode_action(&k(KeyCode::Char('x')), PaneId::Queue), AppAction::CancelSelected);
    assert_eq!(list_mode_action(&k(KeyCode::Char('a')), PaneId::Worktrees), AppAction::None);
}
```

menu_flow_tests replacements (using existing `worktree_snapshot()`/`focus_worktrees` helpers):

```rust
#[test]
fn r_on_worktree_row_opens_add_task_with_raw_name() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    assert!(matches!(&a.mode, Mode::AddTask { worktree: Some(w), resume_session_id: None, .. } if w == "platform.wt-a"));
}

#[test]
fn x_on_worktree_row_opens_confirm_remove_and_y_dispatches_rpc() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('x'));
    assert!(matches!(&a.mode, Mode::ConfirmRemove { .. }));
    let up = a.update(key('y'));
    assert!(matches!(&up.cmds[..], [Cmd::Rpc { call, .. }]
        if call.method == "removeWorktree"
        && call.params == serde_json::json!({"repo": "platform", "name": "platform.wt-a"})));
}

#[test]
fn g_on_worktree_row_opens_tmux_when_inside_tmux() {
    let mut a = app_with(worktree_snapshot());
    a.inside_tmux = true; // adapt to however tests set tmux context today
    focus_worktrees(&mut a);
    let up = a.update(key('g'));
    assert!(matches!(&up.cmds[..], [Cmd::OpenTmux { path }] if path == "/wt/wt-a"));
}

#[test]
fn r_and_x_are_noops_on_session_rows_but_g_works() {
    // Build a snapshot whose selected worktrees row is an interactive session
    // (is_session: true). r and x must produce no mode change + a status line;
    // g must emit Cmd::OpenTmux with the session's cwd path.
}

#[test]
fn a_no_longer_opens_a_menu_on_worktrees() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('a'));
    assert!(matches!(a.mode, Mode::List));
}
```

Fill the session-row test fully using the fixture pattern from `selectors.rs:538–552` / `test_fixtures.rs`. Delete the old worktree-menu flow tests (`menu_typing_filters_then_enter_executes_through_filter` :303, `worktree_menu_task_fresh_opens_add_task_with_raw_name` :417, remove-via-menu :432–475 — replaced above) and the `action_menu.rs` `worktree_menu` builder tests (:330–366).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p qoo-tui`
Expected: FAIL — new AppAction variants missing

- [ ] **Step 3: Implement**

1. `hit.rs`: add `Goto` and `Remove` to `PaneButton`; `PaneId::Worktrees => &[Run, Goto, Remove, Tasks, Create, Collapse]` (Actions chip removed).
2. `keymap.rs`: add the three `AppAction` variants;
   - `Char('r')`: `PaneId::Queue => gated(Run, RequeueSelected)`, `PaneId::Worktrees => gated(Run, NewTaskOnWorktree)`, else `gated(Run, RunSelectedDef)`.
   - `Char('g') => gated(PaneButton::Goto, AppAction::GotoWorktree)`.
   - `Char('x')`: `PaneId::Worktrees => gated(Remove, RemoveSelectedWorktree)`, else `gated(Cancel, CancelSelected)`.
3. `actions.rs` handlers (mirror how `open_actions_or_run` resolves the selected `WorktreeRow`):
   - `NewTaskOnWorktree`: selected row; if `is_session` → status line "tasks target worktrees, not sessions", no mode change; else `Mode::AddTask { worktree: Some(row.raw_name.clone()), resume_session_id: None, resume_label: None, editor: Default::default() }`.
   - `GotoWorktree`: if not inside tmux → status "not inside tmux"; else `Cmd::OpenTmux { path: row.path.clone() }` (works for session rows too).
   - `RemoveSelectedWorktree`: if `is_session` → status "not a worktree"; if `WtState::Busy` → status "a task is running here"; else `Mode::ConfirmRemove { repo, worktree: row.raw_name.clone(), branch: row.branch.clone() }`.
4. `menus.rs`: `open_action_menu` worktrees arm → `None` (no menu; the `a` key is now chip-gated off anyway); delete `M::TaskFresh`/`M::TaskMain` arms.
5. `action_menu.rs`: delete `worktree_menu`, `TaskFresh`/`TaskMain` variants, now-unused DESC consts + tests. `queue_menu`/bulk menus stay.
6. `mouse.rs`: route `PaneButton::Goto → GotoWorktree`, `PaneButton::Remove → RemoveSelectedWorktree`, and `PaneButton::Run` on Worktrees → `NewTaskOnWorktree` (extend the existing L440–447 Run match).
7. `view/panes.rs` `button_chip`: `Goto → ('g', "goto")`, `Remove → ('x', "remove")`, and Run's label on worktrees stays "run"; add theme label constants.
8. `help.rs`: update rows — `a` row now queue-only ("action menu (queue: resume)"), add `("r", "run: new task on worktree (worktrees) · re-queue (queue) · run def (tasks)")`, `("g", "goto: open worktree in tmux (worktrees)")`, `("x", "cancel (queue) · remove worktree (worktrees)")`.

- [ ] **Step 4: Verify**

Run: `cargo test -p qoo-tui`
Expected: PASS. Grep gate: `grep -rn "TaskFresh\|TaskMain\|worktree_menu" crates/` → empty.

- [ ] **Step 5: Commit**

```bash
git add crates/qoo-tui/src
git commit -m "feat(tui): worktrees r/g/x hotkeys replace the action menu"
```

---

### Task 10: SessionPick modal

**Files:**
- Modify: `crates/qoo-tui/src/event.rs` (new `Cmd::FetchSessions` + `Event::SessionsLoaded` + executor)
- Modify: `crates/qoo-tui/src/app/mode.rs` (new `Mode::SessionPick`)
- Modify: `crates/qoo-tui/src/app/actions.rs` (`NewTaskOnWorktree` now opens SessionPick), new key handler module or extend `crates/qoo-tui/src/app/menus.rs`
- Modify: `crates/qoo-tui/src/app/update.rs` (route SessionPick key events + consume `Event::SessionsLoaded`)
- Modify: `crates/qoo-tui/src/view/mod.rs` + `crates/qoo-tui/src/view/menu.rs` (render, mirroring `render_menu`)
- Test: extend `crates/qoo-tui/src/app/menu_flow_tests.rs`

**Interfaces:**
- Consumes: `listSessions` RPC (Task 5), `Mode::AddTask` (Task 8), `AppAction::NewTaskOnWorktree` (Task 9).
- Produces: `SessionChoice { session_id: String, label: String, mtime_ms: u64 }` (serde, snake_case wire match); `Mode::SessionPick { worktree: String, repo: String, items: Vec<SessionChoice>, loading: bool, index: usize, query: String }`; `Cmd::FetchSessions { repo: String, worktree: String }`; `Event::SessionsLoaded { worktree: String, result: Result<Vec<SessionChoice>, String> }`.

- [ ] **Step 1: Write failing flow tests**

```rust
#[test]
fn r_on_worktree_opens_session_pick_and_fetches() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    let up = a.update(key('r'));
    assert!(matches!(&a.mode, Mode::SessionPick { worktree, loading: true, items, .. }
        if worktree == "platform.wt-a" && items.is_empty()));
    assert!(matches!(&up.cmds[..], [Cmd::FetchSessions { repo, worktree }]
        if repo == "platform" && worktree == "platform.wt-a"));
}

fn loaded(worktree: &str) -> Event {
    Event::SessionsLoaded {
        worktree: worktree.into(),
        result: Ok(vec![
            SessionChoice { session_id: "sess-1".into(), label: "Fix the parser".into(), mtime_ms: 2_000 },
            SessionChoice { session_id: "sess-2".into(), label: "Redesign TUI".into(), mtime_ms: 1_000 },
        ]),
    }
}

#[test]
fn sessions_loaded_fills_items_and_enter_on_first_row_is_new_session() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    a.update(loaded("platform.wt-a"));
    // Row 0 is the synthetic "New session"; loaded sessions follow.
    a.update(enter());
    assert!(matches!(&a.mode, Mode::AddTask { resume_session_id: None, worktree: Some(w), .. } if w == "platform.wt-a"));
}

#[test]
fn picking_a_session_pins_it() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    a.update(loaded("platform.wt-a"));
    a.update(down());
    a.update(enter());
    assert!(matches!(&a.mode, Mode::AddTask { resume_session_id: Some(s), resume_label: Some(l), .. }
        if s == "sess-1" && l == "Fix the parser"));
}

#[test]
fn session_pick_type_to_filter_matches_labels() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    a.update(loaded("platform.wt-a"));
    for c in "redesign".chars() { a.update(key(c)); }
    a.update(enter());
    assert!(matches!(&a.mode, Mode::AddTask { resume_session_id: Some(s), .. } if s == "sess-2"));
}

#[test]
fn stale_sessions_loaded_for_other_worktree_is_ignored_and_esc_cancels() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    a.update(loaded("platform.other"));
    assert!(matches!(&a.mode, Mode::SessionPick { loading: true, .. }));
    a.update(esc());
    assert!(matches!(a.mode, Mode::List));
}
```

Note on filtering: the synthetic "New session" row always stays visible regardless of query; type-to-filter applies to loaded session labels (reuse `selectors::filter_rows` semantics).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p qoo-tui session_pick`
Expected: FAIL — variants missing

- [ ] **Step 3: Implement**

1. `event.rs`:

```rust
#[derive(Debug, Clone, serde::Deserialize, PartialEq)]
pub struct SessionChoice {
    pub session_id: String,
    pub label: String,
    pub mtime_ms: u64,
}
```

   `Cmd::FetchSessions { repo: String, worktree: String }`; executor (mirror `Cmd::Rpc` at L268–281, but typed): call `rpc_once(sock, "listSessions", json!({"repo": repo, "worktree": worktree}))`, deserialize `result["sessions"]` into `Vec<SessionChoice>`, send `Event::SessionsLoaded { worktree, result }` (Err(msg) on RPC/parse failure).
2. `mode.rs`: `SessionPick { repo: String, worktree: String, items: Vec<crate::event::SessionChoice>, loading: bool, index: usize, query: String }`.
3. `actions.rs` `NewTaskOnWorktree`: instead of opening AddTask directly (Task 9 interim), set `Mode::SessionPick { repo, worktree: row.raw_name.clone(), items: vec![], loading: true, index: 0, query: String::new() }` and return `Cmd::FetchSessions { repo, worktree: row.raw_name.clone() }`. Session-row guard unchanged.
4. `update.rs`: consume `Event::SessionsLoaded` — only if currently `Mode::SessionPick` with matching `worktree`: `Ok(v)` → `items = v; loading = false`; `Err(e)` → set a status line with the error and keep the modal usable ("New session" still selectable). Key handler (mirror `action_menu_key` in menus.rs L155–): Up/Down move `index` over the filtered view (row 0 = "New session" + filtered items), printable chars extend `query`, Backspace pops, Esc → `Mode::List`, Enter → construct `Mode::AddTask` (index 0 → `resume_session_id: None`; else the picked item's id + label).
5. `view`: render via a compact popup mirroring `render_menu` (menu.rs L352–508): title = worktree display name, rows = `New session` then `"{label} · {relative_age}"` (write a small `fn relative_age(mtime_ms: u64, now_ms: u64) -> String` — `"3m ago"`, `"2h ago"`, `"4d ago"`; unit-test it), bottom description = full session id + absolute time for the highlighted row, `loading` shows a `loading sessions…` placeholder row. Register `HitTarget::Modal`/`MenuItem` like `render_menu` does.

- [ ] **Step 4: Verify**

Run: `cargo test -p qoo-tui`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/qoo-tui/src
git commit -m "feat(tui): session picker for new tasks (r flow) backed by listSessions"
```

---

### Task 11: Main-session remnant sweep + full gate

**Files:**
- Modify: `crates/qoo-tui/src/ipc/types.rs` (remove `mainSessions` from `StateSnapshot` L20–40 area)
- Modify: `crates/qoo-tui/src/selectors.rs` (remove `has_main_session` from `WorktreeRow` L51–91 and everything derived from it — grep drives the list)
- Modify: `crates/qoo-tui/src/test_fixtures.rs`, any snapshot fixtures carrying `mainSessions`/`has_main_session`
- Modify: `AGENTS.md` if it documents the worktree action menu / main-session concept
- Verify: whole-repo gates

**Interfaces:**
- Consumes: daemon no longer sends `mainSessions` (Task 3).

- [ ] **Step 1: Grep-driven removal**

Run `grep -rn "main_session\|mainSessions\|has_main_session" crates/ packages/ AGENTS.md docs/` — remove every production remnant: the `StateSnapshot.mainSessions` field + serde attr, `WorktreeRow.has_main_session` and its consumers (row glyphs/descriptions), fixture fields, and stale AGENTS.md prose. `QueueRow.main_session` display flag: if it only reflected `task.session == "main"`, remove it and its glyph; tasks persisted with `session: "main"` still deserialize (field is a plain string in types.rs L71–93 — keep the field itself for wire compat, just stop styling on it).

- [ ] **Step 2: Run the full gates**

Run: `cargo test -p qoo-tui && cargo clippy -p qoo-tui -- -D warnings && pnpm -r test && pnpm -r typecheck && mise run check`
Expected: all PASS

- [ ] **Step 3: Manual smoke test**

Run the daemon + TUI against this repo (per AGENTS.md dev instructions). Verify: worktrees pane shows `[r]un [g]oto [x] remove` chips; `r` opens the picker listing real named sessions for this worktree; picking one → multiline prompt; Shift+Enter newlines; Enter enqueues; queue shows the task with the pin; `g` opens tmux window; `x` prompts removal confirm; `a` does nothing on worktrees.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: sweep main-session remnants from TUI snapshot and docs"
```
