import { mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type {
	Exec,
	GlobalConfig,
	ResolverIO,
	RunResult,
	SpawnSpec,
} from "@queohoh/core";
import {
	BUILTIN_CATALOG,
	DEFAULT_PROVIDERS,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionLineageStore,
	SessionRegistry,
} from "@queohoh/core";
import { describe, expect, it } from "vitest";
import { Engine } from "../engine.js";
import type { ShimSpawner } from "../shim-host.js";

/** An availability-failure result matching claude's `classifyUnavailable`
 * (`SESSION_LIMIT_RE`) — exit 1 with the session-limit wording in the result
 * text, no distinct exit code or event field. */
const sessionLimitResult: RunResult = {
	exitCode: 1,
	timedOut: false,
	signal: null,
	sessionId: null,
	resultText: "You've hit your session limit · resets 1pm (America/Chicago)",
	stderr: "",
	usage: {
		costUsd: null,
		turns: null,
		durationMs: null,
		inputTokens: null,
		outputTokens: null,
	},
};

const okResult: RunResult = {
	exitCode: 0,
	timedOut: false,
	signal: null,
	sessionId: "sess-grok",
	resultText: "ok",
	stderr: "",
	usage: {
		costUsd: 0,
		turns: 1,
		durationMs: 10,
		inputTokens: null,
		outputTokens: null,
	},
};

/** An availability-failure result matching grok's `classifyUnavailable`
 * (`UNAVAILABLE_RE`) — grok's adapter checks stderr/resultText for its OWN
 * wording (rate-limit/quota/auth), not claude's "session limit" phrasing, so
 * the exhausted-chain test needs a per-provider fixture rather than reusing
 * `sessionLimitResult` for the grok attempt. */
const grokUnavailableResult: RunResult = {
	exitCode: 1,
	timedOut: false,
	signal: null,
	sessionId: null,
	resultText: "429 Too Many Requests: rate limit exceeded",
	stderr: "",
	usage: {
		costUsd: null,
		turns: null,
		durationMs: null,
		inputTokens: null,
		outputTokens: null,
	},
};

/** Mirrors `engine.test.ts`'s `setup`, minus the `executeClaude`/`claudeResult`
 * knobs this suite doesn't need — every run here goes through an injected
 * `spawnShim` (this test drives the daemon's out-of-process spawn path, not
 * `runTask`'s in-process one, since the retry re-drive lives in the engine's
 * `pass()`/`schedule()` loop, not inside a single worker call). */
function setup(overrides: {
	spawnShim: ShimSpawner;
	config?: Partial<GlobalConfig>;
}) {
	const base = mkdtempSync(join(tmpdir(), "qo-engine-fallback-"));
	const repoPath = join(base, "repo");
	mkdirSync(repoPath, { recursive: true });
	const store = new QueueStore(join(base, "state"));
	const runStore = new RunStore(join(base, "runs"));
	const registry = new SessionRegistry(join(base, "sessions.json"));
	const config: GlobalConfig = {
		workspace: join(base, "ws"),
		projects: [{ name: "platform", path: repoPath }],
		maxConcurrentTasks: 3,
		archiveAfterDays: 7,
		vars: {},
		catalog: BUILTIN_CATALOG,
		defaultModels: ["claude/claude-opus-4.8", "grok/grok-4.5"],
		// Default table: claude + grok enabled (codex disabled), fallback order
		// claude -> grok -> codex — exactly the chain this suite exercises.
		providers: DEFAULT_PROVIDERS,
		...overrides.config,
	};
	const resolverIO: ResolverIO = {
		listWorktrees: async () => [
			{ name: "JUS-1", path: join(base, "wt-jus1"), branch: "JUS-1" },
		],
		prBranch: async () => null,
		spawnWorktree: async (_r, name) => ({
			name,
			path: join(base, `wt-${name}`),
			branch: name,
		}),
		removeWorktree: async () => {},
	};
	const exec: Exec = async () => ({ stdout: "", exitCode: 0 });
	const lineage = new SessionLineageStore(join(base, "session-lineage.json"));
	const engine = new Engine({
		store,
		runStore,
		registry,
		config,
		resolverIO,
		exec,
		// Unused by this suite (spawnShim is always injected below), but
		// EngineDeps requires it — mirrors engine.test.ts's default.
		executeClaude: async () => okResult,
		executeVerify: async () => ({
			exitCode: 0,
			timedOut: false,
			signal: null,
			output: "",
		}),
		redact: makeRedactor(new Map()),
		lineage,
		spawnShim: overrides.spawnShim,
	});
	return { engine, store, base };
}

describe("Engine provider fallback", () => {
	it("re-drives onto the next provider after an availability failure, settling done", async () => {
		const specs: SpawnSpec[] = [];
		let call = 0;
		const spawnShim: ShimSpawner = async (_taskId, spec, onPid) => {
			specs.push(spec);
			call += 1;
			onPid(1000 + call);
			return call === 1 ? sessionLimitResult : okResult;
		};
		const { engine, store } = setup({ spawnShim });
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});

		await engine.tick(); // resolve pass: stamps target.worktree
		await engine.tick(); // start pass: attempt 1 (claude) -> availability failure -> retry
		await engine.drain();

		// Retry signal: the task is back to `queued` (NOT settled failed), with
		// claude recorded as attempted so the next resolution skips it.
		const afterFirst = store.list()[0];
		expect(afterFirst?.status).toBe("queued");
		expect(afterFirst?.attemptedModels).toEqual(["claude"]);

		await engine.tick(); // start pass: attempt 2 (grok) -> success
		await engine.drain();

		const task = store.list()[0];
		expect(task?.status).toBe("done");
		expect(task?.attemptedModels).toEqual(["claude"]);
		expect(specs).toHaveLength(2);
		expect(specs[0]?.provider).toBe("claude");
		expect(specs[1]?.provider).toBe("grok");
	});

	it("settles failed (not looping) once every provider in the chain is unavailable", async () => {
		let call = 0;
		const spawnShim: ShimSpawner = async (_taskId, spec, onPid) => {
			call += 1;
			onPid(2000 + call);
			// Each provider's OWN unavailability wording — claude's regex doesn't
			// match grok's, and vice versa (see the fixture comments above).
			return spec.provider === "grok"
				? grokUnavailableResult
				: sessionLimitResult;
		};
		const { engine, store } = setup({ spawnShim });
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});

		await engine.tick(); // resolve
		await engine.tick(); // attempt 1: claude -> retry
		await engine.drain();
		await engine.tick(); // attempt 2: grok -> chain exhausted (codex disabled) -> terminal
		await engine.drain();

		const task = store.list()[0];
		expect(task?.status).toBe("failed");
		// The last attempt's classified reason lands as the terminal error, but
		// (unlike the retry branch) attemptedModels is NOT bumped again here —
		// finalizeRun only appends on the `retry: true` path (see worker.ts).
		expect(task?.error).toBe("provider unavailable");
		expect(task?.attemptedModels).toEqual(["claude"]);
		expect(call).toBe(2);

		// No infinite loop: a further tick must NOT spawn a third attempt — the
		// task settled terminal, so schedule() no longer picks it up as `queued`.
		await engine.tick();
		await engine.drain();
		expect(call).toBe(2);
		expect(store.list()[0]?.status).toBe("failed");
	});
});
