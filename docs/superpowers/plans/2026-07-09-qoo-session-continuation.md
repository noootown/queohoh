# /qoo Session-Continuation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `/qoo` the universal queue interface: queue headless runs that resume the current interactive Claude session in the current worktree (pin + chain-advance), with definition routing and fail-with-guidance for unregistered repos.

**Architecture:** Tasks gain optional `resume_session_id` (pin) and `model` fields. `MainSessionStore` entries gain an `updatedAt` timestamp; the worker resumes the lane pointer only when it postdates the task's creation, else the pin, and advances the pointer after main/pinned runs. Daemon `enqueue`/`runDefinition` accept `cwd` (server-side worktree resolution), `resume_session_id`, `model`; MCP tools expose them. The `skills/qoo/SKILL.md` is rewritten around continuation-by-default.

**Tech Stack:** TypeScript (ESM, strict), zod, vitest, pnpm workspace. Spec: `docs/superpowers/specs/2026-07-09-qoo-skill-session-continuation-design.md`.

## Global Constraints

- ESM imports use explicit `.js` suffixes (existing convention).
- Indentation: tabs (biome-enforced). Run `pnpm exec biome check --write <files>` before committing if unsure.
- No new runtime dependencies.
- Test commands: `pnpm -F @queohoh/core test` and `pnpm -F @queohoh/daemon test` (vitest run). Single file: `pnpm -F @queohoh/core exec vitest run src/__tests__/task.test.ts`.
- Commits: conventional prefix, **no Co-Authored-By trailers**.
- Existing task files on disk must keep parsing (new frontmatter fields default when absent).
- `MainSessionStore.get()` / `.all()` keep their current string shapes — the TUI snapshot (`mainSessions: Record<string, string>`) must not change.

---

### Task 1: Task model — `resume_session_id` + `model` fields

**Files:**
- Modify: `packages/core/src/task.ts`
- Modify: `packages/core/src/store.ts`
- Test: `packages/core/src/__tests__/task.test.ts`, `packages/core/src/__tests__/store.test.ts`

**Interfaces:**
- Consumes: nothing new.
- Produces: `TaskInstance.resumeSessionId: string | null`, `TaskInstance.model: string | null`; `NewTaskInput.resumeSessionId?: string`, `NewTaskInput.model?: string` (later tasks rely on these exact names).

- [ ] **Step 1: Write failing tests**

Append to `packages/core/src/__tests__/task.test.ts` (match the file's existing helper style — it builds a frontmatter string and round-trips via `parseTaskFile`/`serializeTaskFile`; read the file first and reuse its minimal-valid-meta helper if one exists):

```ts
describe("resume_session_id and model fields", () => {
	it("default to null when absent (legacy task files)", () => {
		// Build meta WITHOUT resume_session_id/model keys using the file's
		// existing minimal valid frontmatter fixture.
		const task = parseTaskFile(minimalTaskFileContent());
		expect(task.resumeSessionId).toBeNull();
		expect(task.model).toBeNull();
	});

	it("round-trip when set", () => {
		const task = parseTaskFile(minimalTaskFileContent());
		const withFields = {
			...task,
			resumeSessionId: "c77252c9-11d1-4e68-ab81-f099af529091",
			model: "claude-fable-5",
		};
		const reparsed = parseTaskFile(serializeTaskFile(withFields));
		expect(reparsed.resumeSessionId).toBe(
			"c77252c9-11d1-4e68-ab81-f099af529091",
		);
		expect(reparsed.model).toBe("claude-fable-5");
	});
});
```

Append to `packages/core/src/__tests__/store.test.ts` (reuse its existing store-construction helper):

```ts
it("create persists resumeSessionId and model", () => {
	const store = makeStore(); // the file's existing tmpdir QueueStore helper
	const t = store.create({
		prompt: "p",
		repo: "platform",
		ref: "temp",
		source: "mcp",
		resumeSessionId: "sess-1",
		model: "claude-fable-5",
	});
	const reloaded = store.get(t.id);
	expect(reloaded?.resumeSessionId).toBe("sess-1");
	expect(reloaded?.model).toBe("claude-fable-5");
});

it("create defaults resumeSessionId and model to null", () => {
	const store = makeStore();
	const t = store.create({
		prompt: "p",
		repo: "platform",
		ref: "temp",
		source: "mcp",
	});
	expect(t.resumeSessionId).toBeNull();
	expect(t.model).toBeNull();
});
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `pnpm -F @queohoh/core exec vitest run src/__tests__/task.test.ts src/__tests__/store.test.ts`
Expected: FAIL (unknown properties / type errors).

- [ ] **Step 3: Implement**

In `packages/core/src/task.ts`:

1. In `TaskMetaSchema`, after the `session:` line, add:

```ts
		resume_session_id: z.string().nullable().default(null),
		model: z.string().nullable().default(null),
```

2. In `interface TaskInstance`, after `session: SessionMode;`, add:

```ts
	resumeSessionId: string | null;
	model: string | null;
```

3. In `parseTaskFile`'s returned object, after `session: m.session,`, add:

```ts
		resumeSessionId: m.resume_session_id,
		model: m.model,
```

4. In `serializeTaskFile`'s `meta` object, after `session: task.session,`, add:

```ts
		resume_session_id: task.resumeSessionId,
		model: task.model,
```

In `packages/core/src/store.ts`:

1. In `interface NewTaskInput`, after `session?: SessionMode;`, add:

```ts
	resumeSessionId?: string;
	model?: string;
```

2. In `create()`'s task literal, after `session: input.session ?? "fresh",`, add:

```ts
			resumeSessionId: input.resumeSessionId ?? null,
			model: input.model ?? null,
```

- [ ] **Step 4: Run tests, verify pass**

Run: `pnpm -F @queohoh/core test`
Expected: ALL PASS (whole core suite — worker/instantiate construct TaskInstances and must still compile).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/task.ts packages/core/src/store.ts packages/core/src/__tests__/task.test.ts packages/core/src/__tests__/store.test.ts
git commit -m "feat(core): resume_session_id and model fields on tasks"
```

---

### Task 2: MainSessionStore — timestamped entries with legacy migration

**Files:**
- Modify: `packages/core/src/main-sessions.ts` (full rewrite below)
- Modify (if needed): `packages/core/src/index.ts` — ensure `MainSessionEntry` is exported (if index uses `export *` from the module, nothing to do)
- Test: `packages/core/src/__tests__/main-sessions.test.ts`

**Interfaces:**
- Consumes: nothing new.
- Produces: `MainSessionStore.entry(lane: string): MainSessionEntry | null` where `MainSessionEntry = { sessionId: string; updatedAt: string }`. `get()`, `set()`, `all()` keep existing signatures (`all()` still returns `Record<string, string>`).

- [ ] **Step 1: Write failing tests**

Append to `packages/core/src/__tests__/main-sessions.test.ts`:

```ts
describe("timestamped entries", () => {
	it("entry() returns sessionId with an ISO updatedAt after set()", () => {
		const store = new MainSessionStore(file());
		const before = new Date().toISOString();
		store.set("platform:JUS-1", "sess-abc");
		const entry = store.entry("platform:JUS-1");
		expect(entry?.sessionId).toBe("sess-abc");
		expect(entry?.updatedAt && entry.updatedAt >= before).toBe(true);
	});

	it("entry() on missing lane returns null", () => {
		const store = new MainSessionStore(file());
		expect(store.entry("nope")).toBeNull();
	});

	it("upgrades legacy bare-string entries to epoch updatedAt", () => {
		const path = file();
		writeFileSync(
			path,
			JSON.stringify({ sessions: { "platform:JUS-1": "sess-legacy" } }),
		);
		const store = new MainSessionStore(path);
		expect(store.get("platform:JUS-1")).toBe("sess-legacy");
		expect(store.entry("platform:JUS-1")).toEqual({
			sessionId: "sess-legacy",
			updatedAt: "1970-01-01T00:00:00.000Z",
		});
	});

	it("persists timestamped entries across reload", () => {
		const path = file();
		const store = new MainSessionStore(path);
		store.set("platform:JUS-1", "sess-abc");
		const reloaded = new MainSessionStore(path);
		expect(reloaded.entry("platform:JUS-1")?.sessionId).toBe("sess-abc");
		expect(typeof reloaded.entry("platform:JUS-1")?.updatedAt).toBe("string");
	});

	it("all() still maps lane to bare sessionId strings", () => {
		const store = new MainSessionStore(file());
		store.set("lane-a", "id-a");
		expect(store.all()).toEqual({ "lane-a": "id-a" });
	});
});
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `pnpm -F @queohoh/core exec vitest run src/__tests__/main-sessions.test.ts`
Expected: FAIL (`entry` is not a function).

- [ ] **Step 3: Implement — replace `packages/core/src/main-sessions.ts` with:**

```ts
import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";

export interface MainSessionEntry {
	sessionId: string;
	updatedAt: string;
}

// Legacy bare-string entries get the epoch so an old pointer never outranks
// a task's pinned session (worker compares updatedAt > task.created).
const LEGACY_UPDATED_AT = "1970-01-01T00:00:00.000Z";

function parseEntry(value: unknown): MainSessionEntry | null {
	if (typeof value === "string") {
		return { sessionId: value, updatedAt: LEGACY_UPDATED_AT };
	}
	if (value !== null && typeof value === "object") {
		const v = value as Record<string, unknown>;
		if (typeof v.sessionId === "string" && typeof v.updatedAt === "string") {
			return { sessionId: v.sessionId, updatedAt: v.updatedAt };
		}
	}
	return null;
}

export class MainSessionStore {
	private sessions: Record<string, MainSessionEntry> = Object.create(null);

	constructor(readonly filePath: string) {
		if (existsSync(filePath)) {
			try {
				const parsed = JSON.parse(readFileSync(filePath, "utf-8"));
				if (parsed && typeof parsed.sessions === "object" && parsed.sessions) {
					for (const [lane, value] of Object.entries(parsed.sessions)) {
						const entry = parseEntry(value);
						if (entry) this.sessions[lane] = entry;
					}
				}
			} catch {
				this.sessions = Object.create(null);
			}
		}
	}

	private persist(): void {
		const tmp = `${this.filePath}.tmp`;
		writeFileSync(tmp, JSON.stringify({ sessions: this.sessions }, null, 2));
		renameSync(tmp, this.filePath);
	}

	get(lane: string): string | null {
		return this.sessions[lane]?.sessionId ?? null;
	}

	entry(lane: string): MainSessionEntry | null {
		return this.sessions[lane] ?? null;
	}

	set(lane: string, sessionId: string): void {
		this.sessions[lane] = { sessionId, updatedAt: new Date().toISOString() };
		this.persist();
	}

	/** lane -> sessionId snapshot; timestamps omitted (TUI/API shape). */
	all(): Record<string, string> {
		return Object.fromEntries(
			Object.entries(this.sessions).map(([lane, e]) => [lane, e.sessionId]),
		);
	}
}
```

Check `packages/core/src/index.ts`: if it re-exports `main-sessions.js` with `export *`, `MainSessionEntry` is already exported; otherwise add it.

- [ ] **Step 4: Run tests, verify pass**

Run: `pnpm -F @queohoh/core test`
Expected: ALL PASS (existing main-sessions tests — including proto-key and `all()`-copy tests — must still pass unchanged).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/main-sessions.ts packages/core/src/__tests__/main-sessions.test.ts packages/core/src/index.ts
git commit -m "feat(core): timestamped MainSessionStore entries with legacy migration"
```

---

### Task 3: Worker — pin + chain-advance resume, per-task model

**Files:**
- Modify: `packages/core/src/worker.ts`
- Test: `packages/core/src/__tests__/worker.test.ts`

**Interfaces:**
- Consumes: `TaskInstance.resumeSessionId` / `.model` (Task 1), `MainSessionStore.entry()` (Task 2).
- Produces: worker behavior only; no new exports.

- [ ] **Step 1: Write failing tests**

Append to `packages/core/src/__tests__/worker.test.ts` (reuses the file's `makeDeps`, `withWorktree`, `okResult`; lane for a `withWorktree`'d platform task is `"platform:tmp-x"`). Note tests write `main-sessions.json` directly to control `updatedAt` deterministically:

```ts
describe("runTask pinned resume (resume_session_id)", () => {
	const LANE = "platform:tmp-x";

	const mainStoreAt = (entries: Record<string, unknown>) => {
		const path = join(
			mkdtempSync(join(tmpdir(), "qo-main-")),
			"main-sessions.json",
		);
		writeFileSync(path, JSON.stringify({ sessions: entries }));
		return new MainSessionStore(path);
	};

	const enqueuePinned = (store: QueueStore, model?: string) =>
		store.create({
			prompt: "continue\n",
			repo: "platform",
			ref: "temp",
			source: "mcp",
			resumeSessionId: "pin-sess",
			model,
		});

	it("pin with no pointer → executor resumes the pin", async () => {
		const mainSessions = mainStoreAt({});
		let seenResume: string | undefined;
		const { deps, store } = makeDeps({
			mainSessions,
			executeClaude: async (opts) => {
				seenResume = opts.resumeSessionId;
				return okResult;
			},
		});
		const t = enqueuePinned(store);
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenResume).toBe("pin-sess");
	});

	it("pointer updated after task creation → pointer wins (chain-advance)", async () => {
		const mainSessions = mainStoreAt({
			[LANE]: {
				sessionId: "descendant-sess",
				updatedAt: "2999-01-01T00:00:00.000Z",
			},
		});
		let seenResume: string | undefined;
		const { deps, store } = makeDeps({
			mainSessions,
			executeClaude: async (opts) => {
				seenResume = opts.resumeSessionId;
				return okResult;
			},
		});
		const t = enqueuePinned(store);
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenResume).toBe("descendant-sess");
	});

	it("stale pointer (before task creation, incl. legacy) → pin wins", async () => {
		const mainSessions = mainStoreAt({ [LANE]: "legacy-old-sess" });
		let seenResume: string | undefined;
		const { deps, store } = makeDeps({
			mainSessions,
			executeClaude: async (opts) => {
				seenResume = opts.resumeSessionId;
				return okResult;
			},
		});
		const t = enqueuePinned(store);
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenResume).toBe("pin-sess");
	});

	it("pinned run advances the lane pointer on done", async () => {
		const mainSessions = mainStoreAt({});
		const { deps, store } = makeDeps({
			mainSessions,
			executeClaude: async () => ({ ...okResult, sessionId: "new-sess" }),
		});
		const t = enqueuePinned(store);
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(mainSessions.get(LANE)).toBe("new-sess");
	});

	it("pinned run advances the pointer even on failure", async () => {
		const mainSessions = mainStoreAt({});
		const { deps, store } = makeDeps({
			mainSessions,
			executeClaude: async () => ({
				...okResult,
				exitCode: 3,
				sessionId: "failed-sess",
			}),
		});
		const t = enqueuePinned(store);
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("failed");
		expect(mainSessions.get(LANE)).toBe("failed-sess");
	});

	it("task.model overrides defaults; definition model still wins", async () => {
		let seenModel = "";
		const { deps, store } = makeDeps({
			executeClaude: async (opts) => {
				seenModel = opts.model;
				return okResult;
			},
		});
		const t = enqueuePinned(store, "claude-fable-5");
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenModel).toBe("claude-fable-5");
	});
});
```

For the "definition model still wins" half, add inside the same describe:

```ts
	it("def.model beats task.model", async () => {
		const def: TaskDefinition = {
			name: "d",
			repo: "platform",
			discovery: null,
			args: [],
			dedup: "none",
			worktree: "temp",
			preRun: null,
			postRun: null,
			model: "opus",
			timeoutMs: 60_000,
			priority: "normal",
			prompt: "p",
		};
		let seenModel = "";
		const { deps, store } = makeDeps({
			loadDef: () => def,
			executeClaude: async (opts) => {
				seenModel = opts.model;
				return okResult;
			},
		});
		const t = store.create({
			prompt: "p\n",
			repo: "platform",
			ref: "temp",
			source: "mcp",
			definition: "platform/d",
			item: {},
			itemKey: "adhoc",
			model: "claude-fable-5",
		});
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenModel).toBe("opus");
	});
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `pnpm -F @queohoh/core exec vitest run src/__tests__/worker.test.ts`
Expected: new tests FAIL (resume undefined where pin expected; model "sonnet" where fable expected).

- [ ] **Step 3: Implement in `packages/core/src/worker.ts`**

1. Replace the model line (`const model = def?.model ?? deps.defaults.model;`) with:

```ts
	const model = def?.model ?? task.model ?? deps.defaults.model;
```

2. Replace the main-session pointer resolution block (currently lines ~107–113) with:

```ts
	// Resume resolution at SPAWN time. A pinned task (resume_session_id set)
	// resumes its pin — unless a later run in this lane already advanced the
	// pointer after the task was created (chain-advance): queueing several
	// follow-ups from one session makes each resume the previous run's
	// resulting session. laneKey is null only when the worktree is unresolved
	// (guarded above).
	const lane = laneKey(task);
	let resumeSessionId: string | undefined;
	if (task.resumeSessionId !== null) {
		const ptr = lane !== null ? (deps.mainSessions?.entry(lane) ?? null) : null;
		resumeSessionId =
			ptr !== null && ptr.updatedAt > task.created
				? ptr.sessionId
				: task.resumeSessionId;
	} else if (task.session === "main" && deps.mainSessions && lane !== null) {
		resumeSessionId = deps.mainSessions.get(lane) ?? undefined;
	}
```

3. Replace the pointer-advance block (currently lines ~153–162) with:

```ts
	// Advance the pointer after any outcome (done OR failed) when a main or
	// pinned run captured a sessionId; runs with a null sessionId leave it
	// unchanged.
	if (
		(task.session === "main" || task.resumeSessionId !== null) &&
		deps.mainSessions &&
		lane !== null &&
		result.sessionId !== null
	) {
		deps.mainSessions.set(lane, result.sessionId);
	}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `pnpm -F @queohoh/core test`
Expected: ALL PASS (the existing "fresh task never reads or writes the store" test guards the fresh path).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/worker.ts packages/core/src/__tests__/worker.test.ts
git commit -m "feat(core): worker pin + chain-advance resume and per-task model"
```

---

### Task 4: Definition instantiation — thread `resumeSessionId`

**Files:**
- Modify: `packages/core/src/instantiate.ts`
- Test: `packages/core/src/__tests__/instantiate.test.ts`

**Interfaces:**
- Consumes: `NewTaskInput.resumeSessionId` (Task 1).
- Produces: `InstantiateDeps.resumeSessionId?: string` — Task 5's `runDefinition` passes it.

- [ ] **Step 1: Write failing test**

Append to `packages/core/src/__tests__/instantiate.test.ts` (reuse the file's existing deps/definition fixtures — read it first; the shape mirrors `instantiateDefinition(def, {mode:"args", values:[...]}, deps)`):

```ts
it("stamps resumeSessionId on every created task when provided", async () => {
	const { def, deps } = makeArgsFixture(); // the file's existing helper pattern
	const created = await instantiateDefinition(
		def,
		{ mode: "args", values: ["257"] },
		{ ...deps, resumeSessionId: "sess-pin" },
	);
	expect(created).toHaveLength(1);
	expect(created[0]?.resumeSessionId).toBe("sess-pin");
});

it("leaves resumeSessionId null when not provided", async () => {
	const { def, deps } = makeArgsFixture();
	const created = await instantiateDefinition(
		def,
		{ mode: "args", values: ["258"] },
		deps,
	);
	expect(created[0]?.resumeSessionId).toBeNull();
});
```

(If the test file has no shared fixture helper, inline the same setup its other arg-mode tests use.)

- [ ] **Step 2: Run tests, verify they fail**

Run: `pnpm -F @queohoh/core exec vitest run src/__tests__/instantiate.test.ts`
Expected: FAIL (unknown property `resumeSessionId` on deps / null vs "sess-pin").

- [ ] **Step 3: Implement in `packages/core/src/instantiate.ts`**

1. In `interface InstantiateDeps`, after `refOverride?: string;`, add:

```ts
	resumeSessionId?: string;
```

2. In the `deps.store.create({...})` call, after `itemKey,`, add:

```ts
			resumeSessionId: deps.resumeSessionId,
```

- [ ] **Step 4: Run tests, verify pass**

Run: `pnpm -F @queohoh/core test`
Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/instantiate.ts packages/core/src/__tests__/instantiate.test.ts
git commit -m "feat(core): thread resumeSessionId through definition instantiation"
```

---

### Task 5: Daemon — `resolveCwd` + enqueue/runDefinition params

**Files:**
- Modify: `packages/daemon/src/engine.ts`
- Modify: `packages/daemon/src/api.ts`
- Test: `packages/daemon/src/__tests__/api.test.ts`

**Interfaces:**
- Consumes: `NewTaskInput.resumeSessionId`/`.model` (Task 1), `InstantiateDeps.resumeSessionId` (Task 4).
- Produces: `Engine.resolveCwd(cwd: string): Promise<{repo: string; worktree: string} | null>`, `Engine.gitToplevel(cwd: string): Promise<string | null>`; API `enqueue` params `cwd`, `resume_session_id`, `model`; API `runDefinition` params `cwd`, `resume_session_id`. Task 6's MCP tools call these params by exactly these names.

- [ ] **Step 1: Write failing tests**

Append to `packages/daemon/src/__tests__/api.test.ts` inside `describe("ApiServer")`. The `setup()` helper already accepts `opts.worktrees` which its stub `resolverIO.listWorktrees` returns for every repo:

```ts
	describe("enqueue with cwd / resume_session_id / model", () => {
		const WT = [
			{ name: "repo", path: "/wt/repo", branch: "main" },
			{ name: "repo.fix-x", path: "/wt/repo.fix-x", branch: "fix-x" },
		];

		it("resolves cwd to repo + worktree and stamps resume/model", async () => {
			const { client } = await setup({ worktrees: WT });
			const task = (await client.call("enqueue", {
				prompt: "continue",
				cwd: "/wt/repo.fix-x/src/deep",
				resume_session_id: "sess-1",
				model: "claude-fable-5",
			})) as {
				target: { repo: string; ref: string };
				resumeSessionId: string;
				model: string;
			};
			expect(task.target.repo).toBe("platform");
			expect(task.target.ref).toBe("worktree:repo.fix-x");
			expect(task.resumeSessionId).toBe("sess-1");
			expect(task.model).toBe("claude-fable-5");
		});

		it("prefers the longest matching worktree path", async () => {
			// /wt/repo is a prefix of /wt/repo.fix-x only path-segment-wise;
			// use a genuinely nested pair to prove longest-match.
			const nested = [
				{ name: "outer", path: "/wt/outer", branch: "main" },
				{ name: "inner", path: "/wt/outer/inner", branch: "b" },
			];
			const { client } = await setup({ worktrees: nested });
			const task = (await client.call("enqueue", {
				prompt: "p",
				cwd: "/wt/outer/inner/src",
			})) as { target: { ref: string } };
			expect(task.target.ref).toBe("worktree:inner");
		});

		it("unresolvable cwd fails with config.yaml guidance", async () => {
			const { client } = await setup({ worktrees: WT });
			await expect(
				client.call("enqueue", { prompt: "p", cwd: "/elsewhere/repo" }),
			).rejects.toThrow(/config\.yaml/);
		});

		it("enqueue without repo and without cwd is rejected", async () => {
			const { client } = await setup();
			await expect(client.call("enqueue", { prompt: "p" })).rejects.toThrow(
				/repo or cwd/,
			);
		});
	});

	describe("runDefinition with cwd / resume_session_id", () => {
		const WT = [{ name: "repo.fix-x", path: "/wt/repo.fix-x", branch: "fix-x" }];

		it("targets the resolved worktree and stamps resumeSessionId", async () => {
			const { client } = await setup({ worktrees: WT });
			const created = (await client.call("runDefinition", {
				repo: "platform",
				name: "greet",
				args: ["world"],
				cwd: "/wt/repo.fix-x",
				resume_session_id: "sess-2",
			})) as { target: { ref: string }; resumeSessionId: string }[];
			expect(created).toHaveLength(1);
			expect(created[0]?.target.ref).toBe("worktree:repo.fix-x");
			expect(created[0]?.resumeSessionId).toBe("sess-2");
		});

		it("cwd resolving to a different repo is rejected", async () => {
			// setup registers only "platform"; resolveCwd maps any listed worktree
			// to it, so simulate mismatch by passing an unknown repo name.
			const { client } = await setup({ worktrees: WT });
			await expect(
				client.call("runDefinition", {
					repo: "ghost",
					name: "greet",
					args: ["world"],
					cwd: "/wt/repo.fix-x",
				}),
			).rejects.toThrow(/unknown repo/);
		});
	});
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `pnpm -F @queohoh/daemon exec vitest run src/__tests__/api.test.ts`
Expected: new tests FAIL (empty-repo task created instead of rejection; no cwd resolution).

- [ ] **Step 3: Implement**

In `packages/daemon/src/engine.ts`, add two public methods after `laneOfCwd`:

```ts
	/** Map an absolute path to the registered project + worktree containing it. */
	async resolveCwd(
		cwd: string,
	): Promise<{ repo: string; worktree: string } | null> {
		let best: { repo: string; worktree: string; path: string } | null = null;
		for (const project of this.deps.config.projects) {
			let list: WorktreeInfo[];
			try {
				list = await this.deps.resolverIO.listWorktrees(project.path);
			} catch {
				continue;
			}
			for (const wt of list) {
				if (cwd !== wt.path && !cwd.startsWith(`${wt.path}/`)) continue;
				if (best === null || wt.path.length > best.path.length) {
					best = { repo: project.name, worktree: wt.name, path: wt.path };
				}
			}
		}
		return best === null ? null : { repo: best.repo, worktree: best.worktree };
	}

	/** Best-effort git toplevel of a path, used for error guidance. */
	async gitToplevel(cwd: string): Promise<string | null> {
		try {
			const { stdout, exitCode } = await this.deps.exec(
				"git",
				["-C", cwd, "rev-parse", "--show-toplevel"],
				{ cwd },
			);
			const top = stdout.trim();
			return exitCode === 0 && top.length > 0 ? top : null;
		} catch {
			return null;
		}
	}
```

In `packages/daemon/src/api.ts`:

1. Add `import { basename } from "node:path";` and a module-level helper above `export class ApiServer`:

```ts
function unregisteredCwdMessage(cwd: string, toplevel: string | null): string {
	const repoPath = toplevel ?? cwd;
	return [
		`no registered project contains: ${cwd}`,
		"Add the repo to ~/.config/queohoh/config.yaml under projects:, then retry:",
		"projects:",
		`  - name: ${basename(repoPath)}`,
		`    path: ${repoPath}`,
	].join("\n");
}
```

2. Replace the whole `case "enqueue":` block with:

```ts
			case "enqueue": {
				const worktree =
					typeof params.worktree === "string" && params.worktree.length > 0
						? params.worktree
						: undefined;
				const session = SessionModeSchema.default("fresh").parse(
					params.session,
				);
				const resumeSessionId =
					typeof params.resume_session_id === "string" &&
					params.resume_session_id.length > 0
						? params.resume_session_id
						: undefined;
				const model =
					typeof params.model === "string" && params.model.length > 0
						? params.model
						: undefined;
				const cwd =
					typeof params.cwd === "string" && params.cwd.length > 0
						? params.cwd
						: undefined;
				let repo = typeof params.repo === "string" ? params.repo : "";
				let ref = worktree
					? `worktree:${worktree}`
					: String(params.ref ?? "temp");
				if (cwd !== undefined) {
					const resolved = await deps.engine.resolveCwd(cwd);
					if (resolved === null) {
						throw new Error(
							unregisteredCwdMessage(cwd, await deps.engine.gitToplevel(cwd)),
						);
					}
					repo = resolved.repo;
					ref = `worktree:${resolved.worktree}`;
				}
				if (repo.length === 0) {
					throw new Error("enqueue requires repo or cwd");
				}
				const task = deps.store.create({
					prompt: String(params.prompt ?? ""),
					repo,
					ref,
					source: "mcp",
					priority: (params.priority as "low" | "normal" | "high") ?? "normal",
					session,
					resumeSessionId,
					model,
				});
				deps.onMutation();
				return task;
			}
```

3. In `case "runDefinition":`, after the existing `const worktree = ...` declaration, add:

```ts
				const resumeSessionId =
					typeof params.resume_session_id === "string" &&
					params.resume_session_id.length > 0
						? params.resume_session_id
						: undefined;
				let refOverride = worktree ? `worktree:${worktree}` : undefined;
				if (typeof params.cwd === "string" && params.cwd.length > 0) {
					const resolved = await deps.engine.resolveCwd(params.cwd);
					if (resolved === null) {
						throw new Error(
							unregisteredCwdMessage(
								params.cwd,
								await deps.engine.gitToplevel(params.cwd),
							),
						);
					}
					if (resolved.repo !== repo) {
						throw new Error(
							`cwd resolves to repo ${resolved.repo}, not ${repo}`,
						);
					}
					refOverride = `worktree:${resolved.worktree}`;
				}
```

and change the `instantiateDefinition(...)` options object to use the new values:

```ts
					{
						store: deps.store,
						exec: defaultExec,
						cwd: projectDir,
						source,
						globalVars: deps.config.vars,
						repoVars: loadProjectVars(projectDir),
						refOverride,
						resumeSessionId,
					},
```

Note: the `unknown repo` check (`if (!project) throw ...`) already precedes this — keep it first so the repo-mismatch test hits `unknown repo`.

- [ ] **Step 4: Run tests, verify pass**

Run: `pnpm -F @queohoh/daemon test`
Expected: ALL PASS (existing enqueue/runDefinition tests unchanged: `repo`-only, `worktree`, `session` paths preserved).

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/engine.ts packages/daemon/src/api.ts packages/daemon/src/__tests__/api.test.ts
git commit -m "feat(daemon): cwd resolution, resume_session_id and model on enqueue/runDefinition"
```

---

### Task 6: MCP tool surface

**Files:**
- Modify: `packages/daemon/src/mcp-tools.ts`
- Modify: `packages/daemon/src/mcp.ts`
- Test: `packages/daemon/src/__tests__/mcp-tools.test.ts`

**Interfaces:**
- Consumes: API params from Task 5 (`cwd`, `resume_session_id`, `model`).
- Produces: MCP tools `enqueue_task` (params: `prompt`, `repo?`, `cwd?`, `ref?`, `priority?`, `resume_session_id?`, `model?`) and `run_task_definition` (params: `repo`, `name`, `args?`, `cwd?`, `resume_session_id?`) — the skill (Task 7) calls these.

- [ ] **Step 1: Update/extend tests**

In `packages/daemon/src/__tests__/mcp-tools.test.ts`, update the existing `mcpEnqueueTask` params assertion and add pass-through tests:

Replace the first test's expected params object with:

```ts
			expect(calls).toEqual([
				{
					method: "enqueue",
					params: {
						prompt: "fix it",
						repo: "platform",
						cwd: undefined,
						ref: undefined,
						priority: undefined,
						resume_session_id: undefined,
						model: undefined,
					},
				},
			]);
```

Add:

```ts
	it("passes cwd, resume_session_id and model through", async () => {
		const { caller, calls } = fakeCaller(() => ({ id: "01Y" }));
		await mcpEnqueueTask(caller, {
			prompt: "continue",
			cwd: "/wt/repo.fix-x",
			resume_session_id: "sess-1",
			model: "claude-fable-5",
		});
		expect(calls[0]?.params).toEqual({
			prompt: "continue",
			repo: undefined,
			cwd: "/wt/repo.fix-x",
			ref: undefined,
			priority: undefined,
			resume_session_id: "sess-1",
			model: "claude-fable-5",
		});
	});
```

And in the `mcpRunTaskDefinition` describe, update the existing exact-params assertion to include `cwd: undefined, resume_session_id: undefined`, plus:

```ts
	it("passes cwd and resume_session_id through", async () => {
		const { caller, calls } = fakeCaller(() => [{ id: "01C" }]);
		await mcpRunTaskDefinition(caller, {
			repo: "platform",
			name: "pr-ready",
			cwd: "/wt/repo.fix-x",
			resume_session_id: "sess-2",
		});
		expect(calls[0]?.params).toEqual({
			repo: "platform",
			name: "pr-ready",
			args: [],
			source: "mcp",
			cwd: "/wt/repo.fix-x",
			resume_session_id: "sess-2",
		});
	});
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `pnpm -F @queohoh/daemon exec vitest run src/__tests__/mcp-tools.test.ts`
Expected: FAIL (type errors on new arg names / params mismatch).

- [ ] **Step 3: Implement**

In `packages/daemon/src/mcp-tools.ts`, replace `mcpEnqueueTask` and `mcpRunTaskDefinition`:

```ts
export function mcpEnqueueTask(
	caller: McpCaller,
	args: {
		prompt: string;
		repo?: string;
		cwd?: string;
		ref?: string;
		priority?: "low" | "normal" | "high";
		resume_session_id?: string;
		model?: string;
	},
): Promise<ToolResult> {
	return withPort(caller, (port) =>
		port.call("enqueue", {
			prompt: args.prompt,
			repo: args.repo,
			cwd: args.cwd,
			ref: args.ref,
			priority: args.priority,
			resume_session_id: args.resume_session_id,
			model: args.model,
		}),
	);
}
```

```ts
export function mcpRunTaskDefinition(
	caller: McpCaller,
	args: {
		repo: string;
		name: string;
		args?: string[];
		cwd?: string;
		resume_session_id?: string;
	},
): Promise<ToolResult> {
	return withPort(caller, (port) =>
		port.call("runDefinition", {
			repo: args.repo,
			name: args.name,
			args: args.args ?? [],
			source: "mcp",
			cwd: args.cwd,
			resume_session_id: args.resume_session_id,
		}),
	);
}
```

In `packages/daemon/src/mcp.ts`, replace the `enqueue_task` tool registration's description + schema:

```ts
	server.tool(
		"enqueue_task",
		"Enqueue an ad-hoc task into the queohoh queue. The task runs end-to-end in a worktree and commits its work. Pass cwd (absolute path inside the target worktree) to target the current worktree, and resume_session_id to make the run RESUME that Claude session instead of starting fresh — resumed runs keep the full conversation context. Without resume_session_id workers never see this conversation: transcribe any images, error text, or rich context into the prompt verbatim. Returns the created task as JSON.",
		{
			prompt: z.string().describe("Task prompt (directive if resuming)"),
			repo: z
				.string()
				.optional()
				.describe("Registered project name; omit when cwd is given"),
			cwd: z
				.string()
				.optional()
				.describe(
					"Absolute path inside the target worktree; the daemon resolves repo + worktree from it",
				),
			ref: z
				.string()
				.optional()
				.describe(
					"Target ref: pr:<N> | ticket:<ID> | worktree:<name> | temp (default: temp; ignored when cwd is given)",
				),
			priority: z.enum(["low", "normal", "high"]).optional(),
			resume_session_id: z
				.string()
				.optional()
				.describe(
					"Claude session id to resume; the run continues that session's context",
				),
			model: z
				.string()
				.optional()
				.describe(
					"Model for the run (e.g. claude-fable-5); defaults to the daemon default",
				),
		},
		async (args) => toCallResult(mcpEnqueueTask(caller, args)),
	);
```

and extend `run_task_definition`'s schema with:

```ts
			cwd: z
				.string()
				.optional()
				.describe(
					"Absolute path inside the target worktree; overrides the definition's worktree",
				),
			resume_session_id: z
				.string()
				.optional()
				.describe("Claude session id to resume for the created task(s)"),
```

- [ ] **Step 4: Run tests, verify pass**

Run: `pnpm -F @queohoh/daemon test && pnpm -r typecheck`
Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/mcp-tools.ts packages/daemon/src/mcp.ts packages/daemon/src/__tests__/mcp-tools.test.ts
git commit -m "feat(daemon): expose cwd, resume_session_id and model via MCP tools"
```

---

### Task 7: Rewrite the /qoo skill + docs

**Files:**
- Modify: `skills/qoo/SKILL.md` (full replacement below)
- Modify: `docs/setup.md` (two small edits)

**Interfaces:**
- Consumes: MCP tools from Task 6 (`enqueue_task` with `cwd`/`resume_session_id`/`model`; `run_task_definition` with `cwd`/`resume_session_id`).
- Produces: the user-facing skill.

- [ ] **Step 1: Replace `skills/qoo/SKILL.md` with:**

````markdown
---
name: "qoo"
description: Queue work onto the queohoh orchestrator — the single skill interface to the queue. By default queues a headless CONTINUATION of the current session in the current worktree, so hours-long work runs in the background while you close the tab. Also routes to project task definitions (e.g. pr-ready). Requires the queohoh MCP server and daemon.
user-invocable: true
argument-hint: "<what you want done> | status"
---

# /qoo — queue it and close the tab

Turn the user's request into a queued queohoh run. The daemon runs it
headless in the right worktree; by default the run RESUMES the current
session, so the full conversation context carries over. Your job is ONLY
to route and enqueue — never do the work yourself.

**Input:** `$ARGUMENTS` — free text describing the work, or exactly `status`.

## Routing

- `status` (single token) → call `list_tasks`, render a compact table
  (id-suffix, status, repo:worktree, first ~60 chars of prompt), done.
- Anything else → the Enqueue procedure.

## Enqueue procedure

### 1. Resolve context

In one Bash call, gather:

- `TOPLEVEL=$(git rev-parse --show-toplevel)` — the current worktree.
- `DIRTY=$(git status --porcelain)` — pre-existing uncommitted changes.
- Your own session ID: the UUID path segment of your scratchpad directory
  (from your system prompt: `.../<munged>/<SESSION-UUID>/scratchpad`).
  Verify `~/.claude/projects/<munged>/<uuid>.jsonl` exists, where
  `<munged>` is $TOPLEVEL with every non-alphanumeric character replaced
  by `-`. If the scratchpad UUID has no matching transcript, fall back to
  the newest `.jsonl` in that directory. If neither works, STOP and tell
  the user — never queue a "continuation" that would silently run fresh.

You also know your own model ID (e.g. `claude-fable-5`) — pass it as
`model` so the resumed run doesn't downgrade to the daemon default.

### 2. Match against task definitions

Call `list_task_definitions` and match the request by meaning, not string
equality:

- Exactly one definition plausibly fits → use it. If its args or mode are
  genuinely ambiguous, ask ONE short question (e.g. "/qoo pr-ready" →
  which mode?), then
  `run_task_definition(repo, name, args?, cwd=$TOPLEVEL, resume_session_id=<id>)`.
- 2+ fit → ask the user to pick (one short question).
- None fit → ad-hoc enqueue (step 3).
- Never invent definition names — only names returned by
  `list_task_definitions`.

### 3. Ad-hoc: compose the handoff prompt

The run resumes this session — it already has the full conversation. The
prompt is a short directive, not a context dump. It is not limited to
implementation work; match it to whatever was agreed (implement, make a
PR ready, write docs, run an audit, ...):

- Restate the concrete deliverable and its done-condition as agreed in
  the conversation. Fold the user's /qoo arguments in verbatim.
- Add verification steps only if the task has a runnable check (tests,
  build, lint).
- If $DIRTY was non-empty, add: "The tree already has uncommitted
  changes; fold them into your commits or commit them separately first."
- Always end with: "Commit all work when done — a run that leaves the
  tree dirty is marked failed."

Then:
`enqueue_task(prompt, cwd=$TOPLEVEL, resume_session_id=<id>, model=<your model id>)`.

### Session mode

ALWAYS continuation — for definition runs too — unless the user
explicitly says "fresh". For a fresh run, omit `resume_session_id` and
make the prompt fully self-contained: transcribe verbatim error messages,
file paths, stack traces, and a faithful description of any pasted images
(a fresh worker sees ONLY the prompt).

## Report

One line: what was queued, target repo:worktree, "resumes this session",
model. Then two short notes:

- The run starts once this session goes idle (~5 min after the last
  prompt) or the tab is closed; continuing to work in this session
  afterwards diverges from the fork point.
- Progress is visible in the queohoh TUI.

## Failure modes

- MCP tools missing or connection refused → the daemon isn't running:
  `queohoh daemon` (foreground) or `queohoh launchd:install`.
- Enqueue error "no registered project contains ..." → relay the error's
  config.yaml snippet to the user verbatim; do not try to work around it.
- `run_task_definition` timed out → call `list_tasks` to check whether
  the tasks were created before telling the user anything failed.

## Rules

- Never implement the work inline — even if it looks quick. The point of
  the queue is that the user stays in flow.
- One clarifying question max; prefer sensible defaults.
````

- [ ] **Step 2: Update `docs/setup.md`**

In section 5 ("Enqueue from anywhere"), replace the first bullet with:

```markdown
- In any Claude Code session: `/qoo <request>` — by default this queues a
  headless continuation of that session in the current worktree (close the
  tab; the daemon resumes it with full context). `/qoo status` shows the
  queue.
```

- [ ] **Step 3: Install the skill globally**

```bash
ln -sfn /Users/noootown/Downloads/agent247/queohoh.qoo-skill/skills/qoo ~/.claude/skills/qoo
ls -l ~/.claude/skills/qoo
```

Expected: symlink pointing at the repo's `skills/qoo`. (Note: once the branch merges and the canonical checkout is used, re-point the symlink; mention this in the final report.)

- [ ] **Step 4: Commit**

```bash
git add skills/qoo/SKILL.md docs/setup.md
git commit -m "feat(skill): /qoo continuation-by-default rewrite"
```

---

### Task 8: Build, full test pass, and manual verification

**Files:** none created — verification only.

- [ ] **Step 1: Full build + tests + typecheck**

```bash
pnpm -r build && pnpm -r test && pnpm -r typecheck
```

Expected: all packages green.

- [ ] **Step 2: Verify `claude -p --resume <id>` works from the worktree root for a session launched in a subdirectory** (spec verification item)

```bash
cd /Users/noootown/Downloads/agent247/queohoh.qoo-skill/packages
SESSION_JSON=$(claude -p "Reply with exactly the word: pineapple" --model haiku --output-format json)
SESSION_ID=$(echo "$SESSION_JSON" | python3 -c "import sys,json;print(json.load(sys.stdin)['session_id'])")
cd /Users/noootown/Downloads/agent247/queohoh.qoo-skill
claude -p --resume "$SESSION_ID" "What word did I ask you to reply with?" --model haiku
```

Expected: the answer references "pineapple" → resume-by-id works across launch directories. If it fails, record it in the final report (spec fallback: key resume on transcript-dir munging — a follow-up, not part of this plan).

- [ ] **Step 3: Report**

Summarize: tests green, resume verification outcome, symlink installed, and the remaining user-side steps (register repos in `~/.config/queohoh/config.yaml`, restart the daemon so the new build serves the MCP tools, then try `/qoo` from a real session).
