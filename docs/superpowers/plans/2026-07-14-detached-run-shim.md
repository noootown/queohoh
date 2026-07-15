# Detached Run Shim Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the daemon agnostic of the `claude -p` processes it spawns — a per-run shim process becomes claude's parent, so a daemon reload/crash never kills a running session; a returning daemon re-adopts in-flight runs and finalizes completed ones.

**Architecture:** A tiny `dist/shim.js` (daemon package) reuses core's `executeClaude` unchanged, writing an atomic `result.json` when claude exits. `runTask` in core splits into `startRun` (pre-spawn prep → `SpawnSpec`) and `finalizeRun` (classification/verify/post_run/lineage/finish); `runTask` is retained as the in-process composition of the two. The daemon spawns the shim (detached+unref) via an injectable `ShimSpawner`, awaits its result while alive, and re-adopts orphans through an adoption sweep that replaces the old orphan sweep. Core has zero knowledge of the shim path.

**Tech Stack:** TypeScript (ESM, node16 module resolution), pnpm workspaces (`@queohoh/core`, `@queohoh/daemon`), vitest, biome (tab indent), Rust TUI unaffected.

## Global Constraints

- **ESM + `.js` import specifiers.** Every relative import uses the `.js` extension even though the source is `.ts` (e.g. `import { RunStore } from "./run-store.js"`). Match existing files.
- **biome formatting:** tab indentation, double quotes. Run `pnpm lint` (auto-fix) before committing; `pnpm lint:ci` must pass clean.
- **Dense WHY-carrying doc comments** on new fields/functions/variants, matching neighboring voice (see AGENTS.md). A field without a rationale comment is incomplete.
- **Wire/TUI compatibility:** no changes to `StateSnapshot`, task status values, or run-file formats consumed by the TUI. Do not touch `crates/`.
- **The sanctioned process spawn for claude lives in `core/runner.ts`** (`executeClaude`) — do NOT reimplement claude spawning. The shim REUSES it. The daemon's shim-process spawn is a separate, new spawn (a node subprocess, not claude) and lives in `daemon/src/shim-host.ts`.
- **Single-writer invariant:** the shim writes only run-dir files (`result.json`, events, transcript). All task-store writes stay in the daemon/worker. Do not write task state from the shim.
- **Done-condition:** `mise run check` passes (TS test + typecheck + lint:ci, Rust test + check + clippy). Commit all work; leave the tree clean.

---

## File Structure

- `packages/core/src/run-store.ts` — MODIFY: add `spawn.json` / `result.json` / cancel-marker helpers.
- `packages/core/src/worker.ts` — MODIFY: split `runTask` into `startRun` + `finalizeRun` + shared `resolveRunContext`; keep `runTask` as in-process composition. Export `SpawnSpec`, `StartRunResult`.
- `packages/core/src/index.ts` — MODIFY: export the new symbols.
- `packages/daemon/src/shim.ts` — CREATE: the shim entrypoint (`dist/shim.js`).
- `packages/daemon/src/shim-host.ts` — CREATE: `ShimSpawner` type, `makeShimSpawner` (real, detached), `inProcessSpawner` (default/tests), `WORKER_DIED` sentinel handling.
- `packages/daemon/src/engine.ts` — MODIFY: adoption sweep (`adoptionDecision` pure fn), `runLive`, `adoptAndFinalize`, `settleWorkerDied`, `buildWorkerDeps` extraction, `stopTask` cancel-marker, `EngineDeps` additions (`spawnShim`, `pidAlive`, `isShimPid`).
- `packages/daemon/src/daemon.ts` — MODIFY: wire `makeShimSpawner`.
- `packages/daemon/src/reload.ts` — MODIFY: delete `busyVerdict` + `runningTasks`; `runReload(steps, log)`.
- `packages/daemon/src/cli.ts` — MODIFY: update `runReload` call; keep `--force` as a documented no-op.
- Tests: `run-store.test.ts`, `worker.test.ts`, new `shim.test.ts`, `engine.test.ts`, `reload.test.ts`.

---

## Task 1: RunStore contract files (spawn.json / result.json / cancel marker)

**Files:**
- Modify: `packages/core/src/run-store.ts`
- Test: `packages/core/src/__tests__/run-store.test.ts`

**Interfaces:**
- Consumes: `RunResult` (from `./runner.js`), existing `runDir(taskId)`.
- Produces (new `RunStore` methods):
  - `writeSpawnJson(taskId: string, spec: SpawnSpec): void` — writes `spawn.json` mode `0o600`, UNREDACTED (the shim needs the real prompt).
  - `readSpawnJson(taskId: string): SpawnSpec | null` — lenient parse; null on missing/malformed.
  - `spawnJsonPath(taskId: string): string`
  - `writeResultJson(taskId: string, result: RunResult): void` — atomic (tmp + rename).
  - `readResultJson(taskId: string): RunResult | null` — null on missing/malformed (a torn read is impossible via rename, but a parse guard is still cheap insurance).
  - `writeCancelMarker(taskId: string): void` — writes a `cancelled` file (empty content).
  - `readCancelMarker(taskId: string): boolean` — true iff the marker file exists.
  - `SpawnSpec` type is defined in `worker.ts` (Task 2). To avoid a Task-1→Task-2 import cycle, define `SpawnSpec` here as a local `interface` in `run-store.ts` matching the shape below, and in Task 2 have `worker.ts` import it from `run-store.js`. **Do this in Task 1:** add the `SpawnSpec` interface to `run-store.ts` and export it.

**`SpawnSpec` shape (add to `run-store.ts`, exported):**
```ts
/**
 * The exact inputs the shim needs to reconstruct an `executeClaude` call: the
 * rendered prompt, resolved model/cwd/timeout, optional resume id, and the two
 * run-file paths. Written to `spawn.json` by the shim spawner (0600, unredacted
 * — the shim needs the real prompt) and unlinked by the shim after it reads it.
 * `redact`/`onSpawned` are NOT here: the shim builds its own redactor from its
 * inherited env and tracks the claude pid itself.
 */
export interface SpawnSpec {
	prompt: string;
	model: string;
	cwd: string;
	timeoutMs: number;
	resumeSessionId?: string;
	eventsPath: string;
	transcriptPath: string;
}
```

- [ ] **Step 1: Write the failing tests**

Add to `run-store.test.ts` (a new `describe` block; reuse the `fresh()` helper and `RunResult` shapes already in that file):

```ts
import { existsSync } from "node:fs";
// ... existing imports (fresh, RunStore, etc.) ...

describe("RunStore shim contract files", () => {
	const spec = {
		prompt: "do it with shh-token",
		model: "opus",
		cwd: "/wt/x",
		timeoutMs: 60_000,
		resumeSessionId: "sess-1",
		eventsPath: "/wt/x/events.jsonl",
		transcriptPath: "/wt/x/transcript.md",
	};
	const result = {
		exitCode: 0,
		timedOut: false,
		signal: null,
		sessionId: "s1",
		resultText: "ok",
		stderr: "",
		usage: { costUsd: 1, turns: 2, durationMs: 3 },
	};

	it("spawn.json round-trips UNREDACTED and is mode 0600", () => {
		const rs = fresh();
		rs.writeSpawnJson(task.id, spec);
		const back = rs.readSpawnJson(task.id);
		expect(back).toEqual(spec);
		// Unredacted on disk: the shim needs the real prompt.
		const raw = readFileSync(rs.spawnJsonPath(task.id), "utf-8");
		expect(raw).toContain("shh-token");
		const mode = statSync(rs.spawnJsonPath(task.id)).mode & 0o777;
		expect(mode).toBe(0o600);
	});

	it("readSpawnJson returns null for missing/malformed", () => {
		const rs = fresh();
		expect(rs.readSpawnJson("01NOPE")).toBeNull();
	});

	it("result.json round-trips and readResultJson is null when absent", () => {
		const rs = fresh();
		expect(rs.readResultJson(task.id)).toBeNull();
		rs.writeResultJson(task.id, result);
		expect(rs.readResultJson(task.id)).toEqual(result);
	});

	it("cancel marker: absent → false, written → true", () => {
		const rs = fresh();
		expect(rs.readCancelMarker(task.id)).toBe(false);
		rs.writeCancelMarker(task.id);
		expect(rs.readCancelMarker(task.id)).toBe(true);
	});
});
```

Add `statSync` to the node:fs import in the test file.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd /Users/noootown/Downloads/agent247/queohoh.improve-things && pnpm --filter @queohoh/core test -- run-store`
Expected: FAIL — `writeSpawnJson is not a function` etc.

- [ ] **Step 3: Implement the methods**

In `run-store.ts`: add `renameSync` and `constants` to imports if needed for mode, and add the `SpawnSpec` interface (above) plus:

```ts
	spawnJsonPath(taskId: string): string {
		return join(this.runDir(taskId), "spawn.json");
	}

	/** Write the shim's launch spec. 0600 + UNREDACTED: it holds the real
	 * prompt, which the shim needs; the shim unlinks it immediately after read. */
	writeSpawnJson(taskId: string, spec: SpawnSpec): void {
		writeFileSync(this.spawnJsonPath(taskId), JSON.stringify(spec), {
			mode: 0o600,
		});
	}

	readSpawnJson(taskId: string): SpawnSpec | null {
		const path = this.spawnJsonPath(taskId);
		if (!existsSync(path)) return null;
		try {
			return JSON.parse(readFileSync(path, "utf-8")) as SpawnSpec;
		} catch {
			return null;
		}
	}

	private resultJsonPath(taskId: string): string {
		return join(this.runDir(taskId), "result.json");
	}

	/** Atomic (tmp + rename): the daemon must never read a torn result. */
	writeResultJson(taskId: string, result: RunResult): void {
		const path = this.resultJsonPath(taskId);
		const tmp = `${path}.tmp`;
		writeFileSync(tmp, JSON.stringify(result));
		renameSync(tmp, path);
	}

	readResultJson(taskId: string): RunResult | null {
		const path = this.resultJsonPath(taskId);
		if (!existsSync(path)) return null;
		try {
			return JSON.parse(readFileSync(path, "utf-8")) as RunResult;
		} catch {
			return null;
		}
	}

	private cancelMarkerPath(taskId: string): string {
		return join(this.runDir(taskId), "cancelled");
	}

	/** Persist a user Stop BEFORE signalling, so a stop that races a daemon death
	 * still settles the run as `cancelled` (not `failed`) on adoption. */
	writeCancelMarker(taskId: string): void {
		writeFileSync(this.cancelMarkerPath(taskId), "");
	}

	readCancelMarker(taskId: string): boolean {
		return existsSync(this.cancelMarkerPath(taskId));
	}
```

Ensure `renameSync` is imported: change the `node:fs` import to include `renameSync`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `pnpm --filter @queohoh/core test -- run-store`
Expected: PASS (existing run-store tests + the 4 new ones).

- [ ] **Step 5: Commit**

```bash
cd /Users/noootown/Downloads/agent247/queohoh.improve-things
git add packages/core/src/run-store.ts packages/core/src/__tests__/run-store.test.ts
git commit -m "feat(core): RunStore spawn.json/result.json/cancel-marker helpers"
```

---

## Task 2: Split runTask into startRun + finalizeRun (core/worker.ts)

**Files:**
- Modify: `packages/core/src/worker.ts`
- Modify: `packages/core/src/index.ts`
- Test: `packages/core/src/__tests__/worker.test.ts`

**Interfaces:**
- Consumes: `SpawnSpec` (from `./run-store.js`, Task 1), `WorkerDeps`, `RunResult`, `EMPTY_RESULT`, existing helpers (`resolveModel`, `render`, `execHook`, `extractTicket`, `cleanCapturedOutput`, classification regexes, `VERIFY_TIMEOUT_MS`).
- Produces:
  - `export type StartRunResult = { kind: "settled" } | { kind: "spawn"; spec: SpawnSpec };`
  - `export async function startRun(taskId: string, deps: WorkerDeps): Promise<StartRunResult>` — stamps `running`, resolves context, writes snapshot, runs `pre_run`; on any pre-spawn failure it settles the task (via the existing `finishRun`/`store.update` failure path, INCLUDING `post_run` when a definition is loaded) and returns `{ kind: "settled" }`; on success returns `{ kind: "spawn", spec }`.
  - `export async function finalizeRun(taskId: string, result: RunResult, deps: WorkerDeps): Promise<TaskInstance>` — the classification ladder + verify gate + post_run + lineage fork + finishRun + store.update. Re-derives context from the persisted task; does NOT stamp `running`/`startedAt`.
  - `runTask(taskId, deps)` retained: `const s = await startRun(...); if (s.kind === "settled") return deps.store.get(taskId)!; const result = await deps.executeClaude({ ...s.spec, redact, onSpawned }); return finalizeRun(taskId, result, deps);`

**Design notes (read before coding):**
- Factor the shared pre/re-derivation into an INTERNAL helper `resolveRunContext(taskId, deps): Promise<{ ctx: RunContext } | { fail: string }>` where
  ```ts
  interface RunContext {
  	task: TaskInstance;
  	cwd: string;
  	worktreeContext: Record<string, string>;
  	def: TaskDefinition | null;
  	renderHook: (cmd: string) => string;
  	model: string;
  	timeoutMs: number;
  	resumeSessionId: string | undefined;
  }
  ```
  It does: read task; resolve worktree cwd (fail on null); read branch via `deps.exec` git rev-parse → `worktreeContext`; build `renderHook`; `loadDef` (fail on def-set-but-null); resolve `model`/`timeoutMs`; resolve `resumeSessionId` via `deps.lineage?.tip`. It runs NO hooks. Both `startRun` and `finalizeRun` call it.
- `startRun`:
  1. `deps.store.update(taskId, { status: "running", startedAt: now, error: null, verified: null, verifyExitCode: null, verifyOutput: null })` (verbatim from current lines 93–100).
  2. `const c = await resolveRunContext(taskId, deps); if ("fail" in c) return settleFail(c.fail)` where `settleFail` = the current `fail()` closure (finishRun failed + store.update failed) then `return { kind: "settled" }`.
  3. `deps.runStore.writeSnapshot(taskId, { task: c.ctx.task, definition: c.ctx.def, resolvedWorktree, resolvedWorktreePath: c.ctx.cwd, prompt: c.ctx.task.prompt, model: c.ctx.model }, deps.redact)` (`resolvedWorktree` = `task.target.worktree`, non-null here).
  4. `pre_run`: if `c.ctx.def?.preRun`, run it via `execHook(c.ctx.renderHook(def.preRun), deps.exec, { cwd })`. On throw: settle with `post_run` still attempted, then `finishRun`/`store.update` failed reason `pre_run failed: <msg>`, return `{ kind: "settled" }`. Reuse a shared `settleFailedWithPostRun(taskId, c.ctx, deps, reason)` helper that runs post_run (best-effort, logging on failure — verbatim from current lines 306–315), then `finishRun({ result: EMPTY_RESULT, outcome: "failed", reason })` + `store.update({ status: "failed", error: reason })`.
  5. Build the `SpawnSpec`:
     ```ts
     const spec: SpawnSpec = {
     	prompt: render(c.ctx.task.prompt, {}, {}, c.ctx.worktreeContext),
     	model: c.ctx.model,
     	cwd: c.ctx.cwd,
     	timeoutMs: c.ctx.timeoutMs,
     	resumeSessionId: c.ctx.resumeSessionId,
     	eventsPath: deps.runStore.eventsPath(taskId),
     	transcriptPath: deps.runStore.transcriptPath(taskId),
     };
     return { kind: "spawn", spec };
     ```
- `finalizeRun`:
  1. `const c = await resolveRunContext(taskId, deps); if ("fail" in c) return settleFail(c.fail)` (defensive — should not happen post-spawn).
  2. Classification ladder — VERBATIM from current lines 242–269, using `result` and `deps.isCancelled?.(taskId)` / regexes. Initialize `let outcome = "done"; let reason = null;`.
  3. Verify gate — VERBATIM from current lines 271–303 (uses `c.ctx.def?.verify ?? task.verify`, `c.ctx.renderHook`, `deps.executeVerify`, `VERIFY_TIMEOUT_MS`).
  4. `post_run` — VERBATIM from current lines 306–315 (best-effort, logs; sets `reason` append on failure).
  5. Lineage fork — VERBATIM from current lines 317–328 using `c.ctx.resumeSessionId` and `result.sessionId`.
  6. `finishRun` + `store.update` — VERBATIM from current lines 330–350.
- `runTask` (retained composition): note it must still `writeWorkerPid(process.pid)` to preserve the existing test `readWorkerPid(t.id) === process.pid`. Put that write in `runTask` AFTER a successful `startRun` (not in `startRun`, not in `finalizeRun`):
  ```ts
  export async function runTask(taskId, deps): Promise<TaskInstance> {
  	const s = await startRun(taskId, deps);
  	if (s.kind === "settled") return deps.store.get(taskId) as TaskInstance;
  	deps.runStore.writeWorkerPid(taskId, process.pid);
  	const result = await deps.executeClaude({
  		...s.spec,
  		redact: deps.redact,
  		onSpawned: (pid) => deps.onSpawned?.(taskId, pid),
  	});
  	return finalizeRun(taskId, result, deps);
  }
  ```

**Existing tests must stay green.** Every current `worker.test.ts` case exercises `runTask` and must keep passing unchanged (they inject `executeClaude` and assert on the settled task). Do not edit them except imports if needed. The split is behavior-preserving for `runTask`.

- [ ] **Step 1: Write new failing tests for the split**

Append to `worker.test.ts`:

```ts
describe("startRun / finalizeRun split", () => {
	it("startRun returns a SpawnSpec carrying the rendered prompt + resolved model", async () => {
		const { deps, store } = makeDeps({ modelTable: { sonnet: "claude-sonnet-4-6" } });
		const t = enqueue(store);
		withWorktree(store, t.id);
		const s = await startRun(t.id, deps);
		expect(s.kind).toBe("spawn");
		if (s.kind !== "spawn") throw new Error("expected spawn");
		expect(s.spec.model).toBe("claude-sonnet-4-6");
		expect(s.spec.prompt).toBe("do it\n");
		expect(store.get(t.id)?.status).toBe("running");
	});

	it("startRun settles a def-load failure without spawning", async () => {
		const { deps, store } = makeDeps({ loadDef: () => null });
		const t = enqueue(store, "platform/ghost");
		withWorktree(store, t.id);
		const s = await startRun(t.id, deps);
		expect(s.kind).toBe("settled");
		expect(store.get(t.id)?.status).toBe("failed");
		expect(store.get(t.id)?.error).toContain("definition not found");
	});

	it("finalizeRun classifies a nonzero exit as failed and writes the report", async () => {
		const { deps, store, runStore } = makeDeps();
		const t = enqueue(store);
		withWorktree(store, t.id);
		await startRun(t.id, deps); // stamps running + snapshot
		const settled = await finalizeRun(
			t.id,
			{ ...okResult, exitCode: 3 },
			deps,
		);
		expect(settled.status).toBe("failed");
		expect(settled.error).toBe("exit code 3");
		expect(runStore.readRunMeta(t.id)?.outcome).toBe("failed");
	});

	it("finalizeRun does not re-stamp startedAt (adopted runs keep the original)", async () => {
		const { deps, store } = makeDeps();
		const t = enqueue(store);
		withWorktree(store, t.id);
		await startRun(t.id, deps);
		const startedAt = store.get(t.id)?.startedAt;
		await new Promise((r) => setTimeout(r, 5));
		await finalizeRun(t.id, okResult, deps);
		expect(store.get(t.id)?.startedAt).toBe(startedAt);
	});
});
```

Add `startRun`, `finalizeRun` to the import from `../worker.js`.

- [ ] **Step 2: Run to verify failure**

Run: `pnpm --filter @queohoh/core test -- worker`
Expected: FAIL — `startRun is not exported`.

- [ ] **Step 3: Implement the split** per the Design notes above. Export `startRun`, `finalizeRun`, `StartRunResult` from `worker.ts`; add them to `packages/core/src/index.ts` (`export { ... startRun, finalizeRun, runTask, VERIFY_TIMEOUT_MS } from "./worker.js";` and `export type { ... StartRunResult, WorkerDeps } from "./worker.js";`). Also re-export `SpawnSpec` from index (`export type { ... SpawnSpec } from "./run-store.js";`).

- [ ] **Step 4: Run the full core suite**

Run: `pnpm --filter @queohoh/core test`
Expected: PASS — all existing `worker.test.ts` cases plus the 4 new split cases, plus run-store.

- [ ] **Step 5: Typecheck + commit**

```bash
pnpm --filter @queohoh/core typecheck
git add packages/core/src/worker.ts packages/core/src/index.ts packages/core/src/__tests__/worker.test.ts
git commit -m "feat(core): split runTask into startRun/finalizeRun; keep runTask as composition"
```

---

## Task 3: Shim entrypoint + ShimSpawner (daemon)

**Files:**
- Create: `packages/daemon/src/shim.ts`
- Create: `packages/daemon/src/shim-host.ts`
- Test: `packages/daemon/src/__tests__/shim.test.ts`

**Interfaces:**
- Consumes: `SpawnSpec`, `RunResult`, `RunStore`, `executeClaude`, `buildSecretMap`, `makeRedactor` (all from `@queohoh/core`).
- Produces:
  - `shim-host.ts`:
    ```ts
    /** Spawns a run and resolves when it settles. Returns null when the run
     * produced no result.json (the supervisor died) — the caller then settles
     * the task as `worker died`. onPid reports the process to signal for a Stop
     * (the shim pid in production; the claude child pid in-process). */
    export type ShimSpawner = (
    	taskId: string,
    	spec: SpawnSpec,
    	onPid: (pid: number) => void,
    ) => Promise<RunResult | null>;

    export function makeShimSpawner(opts: {
    	runStore: RunStore;
    	execPath?: string;    // default process.execPath
    	shimCliPath?: string; // default ./shim.js next to this module
    }): ShimSpawner;

    /** Default/test spawner: runs executeClaude IN-PROCESS (no detachment). Used
     * when no real ShimSpawner is injected. onPid receives the claude child pid. */
    export function inProcessSpawner(
    	executeClaude: ClaudeExecutor,
    	redact: Redactor,
    ): ShimSpawner;
    ```
  - `shim.ts`: a `#!/usr/bin/env node` module that reads `spawn.json` from `argv[2]` (the run dir), unlinks it, builds a redactor from `process.env`, traps SIGTERM (forwarding to the claude group via the captured pid), runs `executeClaude`, and writes `result.json` atomically via a `RunStore` constructed from the run dir.

**`shim.ts` implementation:**
```ts
#!/usr/bin/env node
import { basename, dirname } from "node:path";
import {
	buildSecretMap,
	executeClaude,
	makeRedactor,
	RunStore,
} from "@queohoh/core";

async function main(): Promise<void> {
	const runDir = process.argv[2];
	if (!runDir) {
		console.error("shim: missing run dir argument");
		process.exit(2);
	}
	const runStore = new RunStore(dirname(runDir));
	const taskId = basename(runDir);
	const spec = runStore.readSpawnJson(taskId);
	if (!spec) {
		console.error("shim: no spawn.json in run dir");
		process.exit(2);
	}
	// Consume the spec immediately: it holds the unredacted prompt.
	try {
		unlinkSync(runStore.spawnJsonPath(taskId));
	} catch {}

	const redact = makeRedactor(buildSecretMap(process.env));
	let claudePid: number | null = null;
	// A daemon Stop SIGTERMs the shim; forward to claude's own process group so
	// the whole tree dies. executeClaude's close handler then records the signal.
	process.on("SIGTERM", () => {
		if (claudePid !== null) {
			try {
				process.kill(-claudePid, "SIGTERM");
			} catch {}
		}
	});

	const result = await executeClaude({
		...spec,
		redact,
		onSpawned: (pid) => {
			claudePid = pid;
		},
	});
	runStore.writeResultJson(taskId, result);
	process.exit(0);
}

void main();
```
Add `import { unlinkSync } from "node:fs";`.

**`shim-host.ts` implementation:**
```ts
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import type {
	ClaudeExecutor,
	Redactor,
	RunResult,
	RunStore,
	SpawnSpec,
} from "@queohoh/core";
import { executeClaude as _executeClaude } from "@queohoh/core";

export type ShimSpawner = (
	taskId: string,
	spec: SpawnSpec,
	onPid: (pid: number) => void,
) => Promise<RunResult | null>;

export function makeShimSpawner(opts: {
	runStore: RunStore;
	execPath?: string;
	shimCliPath?: string;
}): ShimSpawner {
	const execPath = opts.execPath ?? process.execPath;
	const shimCli =
		opts.shimCliPath ?? fileURLToPath(new URL("./shim.js", import.meta.url));
	return (taskId, spec, onPid) => {
		// spawn.json first: the shim reads it on boot.
		opts.runStore.writeSpawnJson(taskId, spec);
		const child = spawn(execPath, [shimCli, opts.runStore.runDir(taskId)], {
			detached: true, // own process group; survives daemon death
			stdio: "ignore",
			env: process.env,
		});
		child.unref(); // do not keep the daemon's event loop alive for it
		if (child.pid) onPid(child.pid);
		return new Promise<RunResult | null>((resolve) => {
			// While the daemon is alive it is the shim's parent, so `close` fires
			// on exit. A returning daemon has no handle and adopts via the sweep.
			child.on("close", () => resolve(opts.runStore.readResultJson(taskId)));
			child.on("error", () => resolve(null));
		});
	};
}

export function inProcessSpawner(
	executeClaude: ClaudeExecutor = _executeClaude,
	redact: Redactor = (s) => s,
): ShimSpawner {
	return (_taskId, spec, onPid) =>
		executeClaude({ ...spec, redact, onSpawned: onPid });
}
```

- [ ] **Step 1: Write the failing integration test**

Create `packages/daemon/src/__tests__/shim.test.ts`. It spawns the REAL built `dist/shim.js` with the existing fake-claude fixture and asserts `result.json` + events/transcript. (Requires a build; the test builds core+daemon first via the run-store/spawn path — instead, run the shim from source under vitest by pointing at the compiled dist. Since `dist/shim.js` requires a build, gate the test on its existence and build in Step 3.)

```ts
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { RunStore } from "@queohoh/core";

const FAKE = join(
	dirname(fileURLToPath(import.meta.url)),
	"..", "..", "..", "core", "src", "__tests__", "fixtures", "fake-claude.mjs",
);
const SHIM = fileURLToPath(new URL("../../dist/shim.js", import.meta.url));

describe("shim round-trip", () => {
	it("runs executeClaude and writes result.json + events + transcript", () => {
		if (!existsSync(SHIM)) {
			// dist not built in this environment; the mise `check` gate builds first.
			return;
		}
		const runsDir = mkdtempSync(join(tmpdir(), "qo-shim-"));
		const taskId = "01SHIMTEST0000000000000000";
		const runStore = new RunStore(runsDir);
		const runDir = runStore.runDir(taskId);
		runStore.writeSpawnJson(taskId, {
			prompt: "do the thing",
			model: "opus",
			cwd: runDir,
			timeoutMs: 30_000,
			eventsPath: runStore.eventsPath(taskId),
			transcriptPath: runStore.transcriptPath(taskId),
		});
		// The shim resolves `claude` from PATH; point it at the fake via arg? The
		// shim hardcodes claudeBin default. So run with CLAUDE overridden: the fake
		// is an executable .mjs — invoke node on it by symlinking is overkill. The
		// shim uses executeClaude's default "claude"; to exercise it, prepend a
		// PATH dir containing a `claude` shim. Simpler: assert graceful behavior by
		// pointing PATH at a dir with an executable `claude` → the fixture.
		execFileSync(process.execPath, [SHIM, runDir], {
			env: {
				...process.env,
				PATH: `${dirname(FAKE)}:${process.env.PATH}`,
			},
			stdio: "ignore",
		});
		const result = runStore.readResultJson(taskId);
		expect(result?.exitCode).toBe(0);
		expect(result?.sessionId).toBe("sess-123");
		expect(existsSync(runStore.eventsPath(taskId))).toBe(true);
		expect(readFileSync(runStore.transcriptPath(taskId), "utf-8")).toContain(
			"### Tool: Bash",
		);
		// spawn.json consumed.
		expect(existsSync(runStore.spawnJsonPath(taskId))).toBe(false);
	});
});
```

**Note for implementer:** `executeClaude` invokes `claude` from PATH by default. The fixture file is `fake-claude.mjs` (has a shebang `#!/usr/bin/env node` and is executable). Provide a `claude` name on PATH: create a temp dir, symlink/copy `fake-claude.mjs` → `<tmp>/claude` (chmod +x), and set `PATH=<tmp>:$PATH`. Adjust the test to build that PATH dir rather than pointing at the fixtures dir (which has no file literally named `claude`). Implement whichever is cleanest; the assertion targets (`result.json`, events, transcript, spawn.json unlinked) are the contract.

- [ ] **Step 2: Run to verify failure** (shim.js not built yet, or test asserts before impl)

Run: `pnpm -r build && pnpm --filter @queohoh/daemon test -- shim`
Expected: FAIL until `shim.ts`/`shim-host.ts` exist and build.

- [ ] **Step 3: Implement `shim.ts` + `shim-host.ts`**, then `pnpm -r build`.

- [ ] **Step 4: Run the shim test**

Run: `pnpm -r build && pnpm --filter @queohoh/daemon test -- shim`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/shim.ts packages/daemon/src/shim-host.ts packages/daemon/src/__tests__/shim.test.ts
git commit -m "feat(daemon): shim entrypoint + ShimSpawner (real detached + in-process)"
```

---

## Task 4: Adoption sweep + Stop marker + engine wiring

**Files:**
- Modify: `packages/daemon/src/engine.ts`
- Modify: `packages/daemon/src/daemon.ts`
- Test: `packages/daemon/src/__tests__/engine.test.ts`

**Interfaces:**
- Consumes: `startRun`, `finalizeRun`, `SpawnSpec` (core), `ShimSpawner`, `inProcessSpawner`, `makeShimSpawner` (shim-host), `RunResult`.
- Produces:
  - `export function adoptionDecision(hasResult: boolean, pidAlive: boolean, argvLooksLikeShim: boolean): "finalize" | "adopt" | "orphan"` — pure:
    ```ts
    export function adoptionDecision(hasResult, pidAlive, argvLooksLikeShim) {
    	if (hasResult) return "finalize";
    	if (pidAlive && argvLooksLikeShim) return "adopt";
    	return "orphan";
    }
    ```
  - `EngineDeps` gains (all optional, additive):
    - `spawnShim?: ShimSpawner` — production injects `makeShimSpawner(...)`; absent → the Engine builds `inProcessSpawner(deps.executeClaude, deps.redact)`.
    - `pidAlive?: (pid: number) => boolean` — default `process.kill(pid, 0)` guard.
    - `isShimPid?: (pid: number) => boolean` — default a `ps -p <pid> -o command=` check for `shim.js`; injectable for tests.

**Engine internal changes:**
- Add `private finalizing = new Map<string, Promise<void>>();` (guards the async adopt/finalize from double-firing across ticks; also awaited by `drain`).
- Resolve the spawner once in the constructor: `private readonly spawnShim: ShimSpawner = this.deps.spawnShim ?? inProcessSpawner(this.deps.executeClaude, this.deps.redact);`
- `drain()` awaits both maps: `await Promise.all([...this.running.values(), ...this.finalizing.values()]);`
- Extract `buildWorkerDeps(task): WorkerDeps | null` from the current `startWorker` body (the vars/model/table resolution + the deps object literal, MINUS `onSpawned`/`isCancelled` which are set per-call). Return `null` after having already marked the task failed on a vars.yaml error (preserve current behavior). Wire `isCancelled: (id) => this.cancelledTaskIds.has(id) || deps.runStore.readCancelMarker(id)`.
- Replace `startWorker(task)` with:
  ```ts
  private startWorker(task: TaskInstance): void {
  	const deps = this.buildWorkerDeps(task);
  	if (deps === null) return; // already marked failed + onChange fired
  	const lane = laneKey(task) ?? task.id;
  	this.deps.registry.registerWorker(task.id, lane, process.pid);
  	const promise = this.runLive(task.id, deps)
  		.catch((err) => {
  			try {
  				this.deps.store.update(task.id, {
  					status: "failed",
  					error: err instanceof Error ? err.message : String(err),
  				});
  			} catch {}
  		})
  		.then(() => this.cleanupRun(task.id));
  	this.running.set(task.id, promise);
  	this.deps.onChange?.();
  }

  private async runLive(taskId: string, deps: WorkerDeps): Promise<void> {
  	const start = await startRun(taskId, deps);
  	if (start.kind === "settled") return; // failed pre-spawn; nothing spawned
  	const result = await this.spawnShim(taskId, start.spec, (pid) => {
  		this.childPids.set(taskId, pid);
  		deps.runStore.writeWorkerPid(taskId, pid); // shim pid (production)
  	});
  	if (result === null) {
  		await this.settleWorkerDied(taskId, deps);
  		return;
  	}
  	await finalizeRun(taskId, result, deps);
  }

  private cleanupRun(taskId: string): void {
  	this.running.delete(taskId);
  	this.childPids.delete(taskId);
  	this.cancelledTaskIds.delete(taskId);
  	this.deps.registry.unregisterWorker(taskId);
  	this.deps.onChange?.();
  }

  /** No result.json and the shim is gone: settle as worker died (a report is
   * still written so the detail pane isn't blank). Mirrors the sweep's orphan. */
  private async settleWorkerDied(taskId: string, deps: WorkerDeps): Promise<void> {
  	deps.runStore.finishRun(
  		taskId,
  		{
  			result: {
  				exitCode: 1, timedOut: false, signal: null, sessionId: null,
  				resultText: "", stderr: "worker died",
  				usage: { costUsd: null, turns: null, durationMs: null },
  			},
  			outcome: "failed",
  			reason: "worker died",
  		},
  		deps.redact,
  	);
  	deps.store.update(taskId, { status: "failed", error: "worker died" });
  }
  ```
- Replace the **orphan sweep** (current lines 308–316) with the **adoption sweep**:
  ```ts
  // Adoption sweep: a task that is `running` on disk but not managed by THIS
  // process (fresh boot, reload, or crash recovery). result.json present → the
  // shim finished while we were away, finalize now; shim pid still alive (and
  // its argv is a shim, guarding pid reuse) → re-adopt, keep polling via the
  // tick; neither → the supervisor is gone, fail it.
  for (const t of deps.store.list()) {
  	if (
  		t.status !== "running" ||
  		this.running.has(t.id) ||
  		this.finalizing.has(t.id)
  	) {
  		continue;
  	}
  	const hasResult = deps.runStore.readResultJson(t.id) !== null;
  	const pid = deps.runStore.readWorkerPid(t.id);
  	const alive = pid !== null && this.isPidAlive(pid);
  	const shimArgv = alive && this.isShimPidCheck(pid as number);
  	const decision = adoptionDecision(hasResult, alive, shimArgv);
  	if (decision === "finalize") {
  		const deps2 = this.buildWorkerDeps(t);
  		if (deps2) {
  			const p = this.adoptAndFinalize(t.id, deps2).finally(() =>
  				this.finalizing.delete(t.id),
  			);
  			this.finalizing.set(t.id, p);
  		}
  	} else if (decision === "adopt") {
  		// Idempotent re-registration so Stop works and the lane stays busy.
  		if (pid !== null) this.childPids.set(t.id, pid);
  		const lane = laneKey(t) ?? t.id;
  		deps.registry.registerWorker(t.id, lane, pid ?? process.pid);
  	} else {
  		deps.store.update(t.id, { status: "failed", error: "worker died" });
  		this.childPids.delete(t.id);
  		deps.registry.unregisterWorker(t.id);
  	}
  }
  ```
  where `adoptAndFinalize`:
  ```ts
  private async adoptAndFinalize(taskId: string, deps: WorkerDeps): Promise<void> {
  	const result = deps.runStore.readResultJson(taskId);
  	if (result === null) {
  		await this.settleWorkerDied(taskId, deps);
  	} else {
  		await finalizeRun(taskId, result, deps);
  	}
  	this.childPids.delete(taskId);
  	this.cancelledTaskIds.delete(taskId);
  	deps.registry.unregisterWorker(taskId);
  	deps.onChange?.();
  }
  ```
  Add helpers `private isPidAlive(pid): boolean { return (this.deps.pidAlive ?? defaultPidAlive)(pid); }` and `private isShimPidCheck(pid): boolean { return (this.deps.isShimPid ?? defaultIsShimPid)(pid); }` with module-level `defaultPidAlive` (process.kill 0) and `defaultIsShimPid` (execFileSync `ps -p <pid> -o command=` → includes `shim.js`; wrap in try/catch → false).
- `stopTask` gains the marker write BEFORE signalling:
  ```ts
  stopTask(taskId: string): void {
  	const pid = this.childPids.get(taskId);
  	if (pid === undefined) {
  		throw new Error(`no running child tracked for task: ${taskId}`);
  	}
  	this.cancelledTaskIds.add(taskId);
  	this.deps.runStore.writeCancelMarker(taskId); // persist BEFORE the signal
  	// ... existing SIGTERM group + 5s SIGKILL escalation, unchanged ...
  }
  ```

**`daemon.ts` wiring:** construct the spawner and pass it:
```ts
import { makeShimSpawner } from "./shim-host.js";
// ...
const engine = new Engine({
	// ...existing deps...
	spawnShim: makeShimSpawner({ runStore }),
});
```
(Keep `executeClaude` in the deps — the Engine still uses it to build the fallback in-process spawner if `spawnShim` were ever absent, and `buildWorkerDeps` still passes it down for `runTask`-less paths that reference it via WorkerDeps.)

**Update `engine.test.ts`:**
- The test "marks running tasks with no live worker as orphaned" (line ~316) now expects `"worker died"`:
  ```ts
  it("marks a running task with no result and no live shim as worker died", async () => {
  	const { engine, store } = setup();
  	const t = store.create({ prompt: "p", repo: "platform", ref: "temp", source: "tui" });
  	store.update(t.id, {
  		status: "running",
  		target: { repo: "platform", ref: "temp", worktree: "JUS-1" },
  	});
  	await engine.tick();
  	expect(store.get(t.id)?.status).toBe("failed");
  	expect(store.get(t.id)?.error).toBe("worker died");
  });
  ```
- Add adoption tests (use `setup()` + on-disk run files + injected `pidAlive`/`isShimPid`):
  ```ts
  it("finalizes an adopted task whose result.json is already present", async () => {
  	const { engine, store, base } = setup();
  	const t = store.create({ prompt: "p", repo: "platform", ref: "temp", source: "tui" });
  	store.update(t.id, {
  		status: "running",
  		target: { repo: "platform", ref: "temp", worktree: "JUS-1" },
  	});
  	const rs = new RunStore(join(base, "runs"));
  	rs.writeResultJson(t.id, { ...okResult, resultText: "done" });
  	await engine.tick();
  	await engine.drain();
  	expect(store.get(t.id)?.status).toBe("done");
  });

  it("adopts a live shim (result absent, pid alive & argv is shim) and leaves it running", async () => {
  	const { engine, store, base } = setupWith({ pidAlive: () => true, isShimPid: () => true });
  	const t = store.create({ prompt: "p", repo: "platform", ref: "temp", source: "tui" });
  	store.update(t.id, {
  		status: "running",
  		target: { repo: "platform", ref: "temp", worktree: "JUS-1" },
  	});
  	new RunStore(join(base, "runs")).writeWorkerPid(t.id, 999999);
  	await engine.tick();
  	expect(store.get(t.id)?.status).toBe("running"); // still adopted, not settled
  });
  ```
  Add `adoptionDecision` unit tests (pure, table-driven):
  ```ts
  describe("adoptionDecision", () => {
  	it("result present → finalize regardless of pid", () => {
  		expect(adoptionDecision(true, false, false)).toBe("finalize");
  		expect(adoptionDecision(true, true, true)).toBe("finalize");
  	});
  	it("no result, live shim → adopt", () => {
  		expect(adoptionDecision(false, true, true)).toBe("adopt");
  	});
  	it("no result, live pid but not a shim (reuse) → orphan", () => {
  		expect(adoptionDecision(false, true, false)).toBe("orphan");
  	});
  	it("no result, dead pid → orphan", () => {
  		expect(adoptionDecision(false, false, false)).toBe("orphan");
  	});
  });
  ```
  Extend `setup()` to accept `pidAlive`/`isShimPid` overrides (add a `setupWith` variant or thread through `overrides`). Import `adoptionDecision` from `../engine.js` and `RunStore` (already imported).
- The existing `stopTask` test still passes (in-process spawner + onPid → childPids; cancel marker is written harmlessly).

- [ ] **Step 1: Write the new/updated tests** (adoptionDecision, adoption finalize/adopt, worker-died rename) as above; run to verify they fail.

Run: `pnpm -r build && pnpm --filter @queohoh/daemon test -- engine`
Expected: FAIL (`adoptionDecision` not exported; orphan string still old).

- [ ] **Step 2: Implement the engine changes + daemon.ts wiring** per Interfaces above.

- [ ] **Step 3: Run the engine + api suites**

Run: `pnpm -r build && pnpm --filter @queohoh/daemon test -- engine && pnpm --filter @queohoh/daemon test -- api`
Expected: PASS (api.test.ts's parked-worker test still works via the in-process spawner default; note engine tests here inject `executeClaude` only, so the Engine uses `inProcessSpawner(deps.executeClaude, deps.redact)`).

- [ ] **Step 4: Typecheck**

Run: `pnpm --filter @queohoh/daemon typecheck`

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/engine.ts packages/daemon/src/daemon.ts packages/daemon/src/__tests__/engine.test.ts
git commit -m "feat(daemon): adoption sweep + detached shim wiring + Stop cancel marker"
```

---

## Task 5: Remove the reload busy guard

**Files:**
- Modify: `packages/daemon/src/reload.ts`
- Modify: `packages/daemon/src/cli.ts`
- Test: `packages/daemon/src/__tests__/reload.test.ts`

**Interfaces:**
- Remove: `busyVerdict`, `BusyVerdict`, and `runningTasks` from `ReloadSteps`.
- Change: `export async function runReload(steps: ReloadSteps, log: ReloadLog): Promise<number>` — no `opts`/`force`. Body: `repoRoot` (null → error, exit 1) → `build` (non-zero → error, exit 1) → `restart` → `verify` (false → error+logTail, exit 1) → info "daemon reloaded", exit 0.
- `defaultReloadSteps` drops the `runningTasks` implementation.
- `cli.ts`: keep the `--force` option registered but document it as a no-op; call `runReload(defaultReloadSteps(cliPath), { info: console.log, error: console.error })` (drop the `{ force }` arg). Update the option description to `"(no-op, kept for compatibility) reload always proceeds now"`.

- [ ] **Step 1: Update the tests**

In `reload.test.ts`: delete the `describe("busyVerdict", ...)` block and the three `runReload` cases that assert on busy/force behavior ("busy without force", "task starts during the build → exit 1", "task starts during the build but --force", "busy with force"). Update `makeSteps` to drop `runningTasks`. Update the remaining `runReload` calls to the new signature `runReload(steps, silentLog)`. Keep: no-repo-root, build-failure, happy-path, verify-failure. Remove `busyVerdict` from the import.

Example updated happy-path:
```ts
it("happy path → build, restart, verify in order, exit 0", async () => {
	const { steps, calls } = makeSteps();
	expect(await runReload(steps, silentLog)).toBe(0);
	expect(calls).toEqual(["build", "restart"]);
});
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm --filter @queohoh/daemon test -- reload`
Expected: FAIL (signature mismatch).

- [ ] **Step 3: Implement** the `reload.ts` and `cli.ts` changes.

- [ ] **Step 4: Run reload tests + typecheck**

Run: `pnpm --filter @queohoh/daemon test -- reload && pnpm --filter @queohoh/daemon typecheck`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/reload.ts packages/daemon/src/cli.ts packages/daemon/src/__tests__/reload.test.ts
git commit -m "feat(daemon): reload always proceeds (busy guard removed; --force kept as no-op)"
```

---

## Task 6: Full gate + AGENTS.md touch-up

**Files:**
- Modify: `AGENTS.md` (note the shim in the daemon hierarchy + the run-dir contract).
- Modify: `packages/core/AGENTS.md` if it names `runTask`/`executeClaude` boundaries (keep accurate).

- [ ] **Step 1: Update AGENTS.md**

In the root `AGENTS.md` "Architectural invariants" / daemon section, add one line each:
- Daemon spawns a detached per-run **shim** (`dist/shim.js`) that owns the `claude -p` child; the daemon re-adopts in-flight runs after a restart via the adoption sweep (`result.json` present → finalize; live shim pid → adopt; else → `worker died`).
- Run-dir contract: `spawn.json` (0600, daemon→shim, unlinked after read), `result.json` (shim→daemon, atomic), `worker.json` (shim pid), `cancelled` (Stop marker).

Keep additions terse (one line each per the AGENTS.md "what earns a line" rule).

- [ ] **Step 2: Run the full gate**

Run: `cd /Users/noootown/Downloads/agent247/queohoh.improve-things && mise run check`
Expected: PASS — `test`, `typecheck`, `lint:ci`, `test:rs`, `typecheck:rs`, `lint:rs` all green. If lint flags formatting, run `pnpm lint` (auto-fix) and re-run.

- [ ] **Step 3: Commit**

```bash
git add AGENTS.md packages/core/AGENTS.md
git commit -m "docs: AGENTS.md — detached shim + run-dir contract"
```

---

## Self-Review (completed during planning)

- **Spec coverage:** shim entry (Task 3) ✓; spawn/result/worker/cancel contract (Task 1) ✓; startRun/finalizeRun split (Task 2) ✓; adoption sweep replacing orphan sweep (Task 4) ✓; Stop via shim pid + persisted marker (Task 4) ✓; reload busy guard removed, `--force` no-op (Task 5) ✓; unit (worker split, adoptionDecision pure) + integration (shim round-trip) tests (Tasks 2–4) ✓.
- **Refinement vs spec:** `spawn.json` is written by the **shim spawner** (`shim-host.ts`), not `startRun`, keeping core free of the shim path — a deliberate improvement over the spec's prose; behavior is identical. `worker.json` holds only the shim pid (claude pid dropped — no consumer, YAGNI).
- **Type consistency:** `SpawnSpec` defined once in `run-store.ts`, imported by `worker.ts`/`shim-host.ts`/`shim.ts`/`engine.ts`. `ShimSpawner` returns `Promise<RunResult | null>` (null = worker died) everywhere. `adoptionDecision(hasResult, pidAlive, argvLooksLikeShim)` signature matches the spec and its test.
