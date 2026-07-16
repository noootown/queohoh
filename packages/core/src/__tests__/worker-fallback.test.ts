import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderConfig } from "../config.js";
import { makeRedactor } from "../redact.js";
import type { Exec } from "../resolver-io.js";
import { RunStore } from "../run-store.js";
import type { RunResult } from "../runner.js";
import { executeRun } from "../runner.js";
import { SessionLineageStore } from "../session-lineage.js";
import { QueueStore } from "../store.js";
import type { WorkerDeps } from "../worker.js";
import { runTask } from "../worker.js";

// `runTask` dispatches non-claude providers through `executeRun` (with the
// provider's own adapter) rather than the injected `executeClaude` seam — the
// in-process mirror of the daemon's `inProcessSpawner`. `executeRun` spawns a
// real CLI, so a fallback test that drives a grok hop in-process has to stub
// it (WorkerDeps has no `executeRun` seam by design — the daemon injects a
// whole `spawnShim` at the engine level instead; see engine-fallback.test.ts).
// Partial mock: only `executeRun` is replaced; the rest of runner.js
// (executeClaude/executeVerify/…) stays real.
vi.mock("../runner.js", async (importOriginal) => ({
	...(await importOriginal<typeof import("../runner.js")>()),
	executeRun: vi.fn(),
}));

// Braces are load-bearing: a bare `() => mock.mockReset()` would RETURN the
// mock (mockReset returns it for chaining), and vitest treats a value returned
// from beforeEach as a teardown callback — it would then invoke the mock with
// no args during cleanup.
beforeEach(() => {
	vi.mocked(executeRun).mockReset();
});

const okResult: RunResult = {
	exitCode: 0,
	timedOut: false,
	signal: null,
	sessionId: "s",
	resultText: "did it",
	stderr: "",
	usage: { costUsd: 0.1, turns: 1, durationMs: 100 },
};

const sessionLimitResult: RunResult = {
	exitCode: 1,
	timedOut: false,
	signal: null,
	sessionId: null,
	resultText: "You've hit your session limit · resets 1pm (America/Chicago)",
	stderr: "",
	usage: { costUsd: null, turns: null, durationMs: null },
};

// Two enabled providers sharing the "sonnet" tier — the exact fallback shape
// (claude → grok) the design spec walks through in its worked example.
const PROVIDERS: ProviderConfig[] = [
	{ name: "claude", enabled: true, models: { sonnet: "claude-sonnet-5" } },
	{ name: "grok", enabled: true, models: { sonnet: "grok-composer-2.5-fast" } },
];

/** Mirrors `worker.test.ts`'s `makeDeps`, scripting the run's outcome by
 * provider hop: the FIRST spawn returns `firstResult`, every spawn after
 * succeeds (`okResult`) — one call per provider hop, so a two-call test drives
 * claude then grok. `claude` goes through the injected `executeClaude`; `grok`
 * (non-claude) goes through the mocked `executeRun`. Both funnel through the
 * same `nextResult` counter so a claude→grok fallback increments across the two
 * executors, and both record the provider they ran into `seenProviders`. */
function makeFallbackDeps(opts: {
	firstResult: RunResult;
	resumeSessionId?: string;
}) {
	const base = mkdtempSync(join(tmpdir(), "qo-worker-fallback-"));
	const store = new QueueStore(join(base, "state"));
	const runStore = new RunStore(join(base, "runs"));
	const seenProviders: (string | undefined)[] = [];
	let calls = 0;
	const nextResult = (provider: string | undefined): RunResult => {
		calls += 1;
		seenProviders.push(provider);
		return calls === 1 ? opts.firstResult : okResult;
	};
	vi.mocked(executeRun).mockImplementation((adapter, execOpts) => {
		execOpts.onSpawned?.(4321);
		return Promise.resolve(nextResult(adapter.name));
	});
	const gitClean: Exec = async () => ({ stdout: "", exitCode: 0 });
	const deps: WorkerDeps = {
		store,
		runStore,
		exec: gitClean,
		executeClaude: async (spawnOpts) =>
			nextResult((spawnOpts as unknown as { provider?: string }).provider),
		redact: makeRedactor(new Map()),
		loadDef: () => null,
		worktreePath: async () => "/wt/path",
		defaults: { model: "sonnet", timeoutMs: 60_000 },
	};
	const t = store.create({
		prompt: "do it\n",
		repo: "platform",
		ref: "temp",
		source: "tui",
		resumeSessionId: opts.resumeSessionId,
	});
	store.update(t.id, {
		target: { repo: "platform", ref: "temp", worktree: "tmp-x" },
	});
	return { deps, providers: PROVIDERS, taskId: t.id, seenProviders, runStore };
}

const reportOf = (runStore: RunStore, taskId: string): string =>
	readFileSync(join(runStore.runDir(taskId), "report.md"), "utf-8");

describe("runTask provider fallback", () => {
	it("availability failure on fresh run records the attempt and signals retry", async () => {
		const { deps, providers, taskId, seenProviders, runStore } =
			makeFallbackDeps({
				firstResult: sessionLimitResult,
			});
		const first = await runTask(taskId, { ...deps, providers });
		expect(first.attemptedProviders).toContain("claude");
		// Retry: re-queued, not settled failed.
		expect(first.status).not.toBe("failed");
		expect(first.status).toBe("queued");
		expect(seenProviders).toEqual(["claude"]);

		// The hop is rendered into THIS attempt's report.md "## Attempts" trail
		// (finding 5) with the "→ falling back" suffix.
		const firstReport = reportOf(runStore, taskId);
		expect(firstReport).toContain("## Attempts");
		expect(firstReport).toContain(
			"attempt 1: claude — session limit → falling back",
		);

		// The engine (Task 10) would re-drive automatically; the in-process
		// caller here just invokes runTask again, which re-resolves the chain
		// minus attemptedProviders and lands on grok.
		const second = await runTask(taskId, { ...deps, providers });
		expect(second.status).toBe("done");
		expect(seenProviders).toEqual(["claude", "grok"]);
	});

	it("resume task does not fall back; settles session limit", async () => {
		const { deps, providers, taskId, seenProviders } = makeFallbackDeps({
			firstResult: sessionLimitResult,
			resumeSessionId: "s1",
		});
		const out = await runTask(taskId, { ...deps, providers });
		expect(out.status).toBe("failed");
		expect(out.error).toBe("session limit");
		// Never walked the chain — one attempt, on claude, and no attempt
		// recorded in attemptedProviders (a settled failure isn't a "hop").
		expect(seenProviders).toEqual(["claude"]);
		expect(out.attemptedProviders).toEqual([]);
	});

	it("resume task whose session is tagged grok resolves onto grok, not the chain head", async () => {
		const lineage = new SessionLineageStore(
			join(mkdtempSync(join(tmpdir(), "qo-lineage-")), "lineage.json"),
		);
		lineage.recordProvider("s1", "grok");
		const { deps, providers, taskId, seenProviders } = makeFallbackDeps({
			firstResult: okResult,
			resumeSessionId: "s1",
		});
		const out = await runTask(taskId, { ...deps, providers, lineage });
		expect(out.status).toBe("done");
		// Chain resolution for "sonnet" puts claude FIRST — the pin overrides
		// that, going straight to grok because that's what "s1" is tagged with.
		expect(seenProviders).toEqual(["grok"]);
	});

	it("chain exhausted (every provider unavailable) settles failed, not looping", async () => {
		const base = mkdtempSync(join(tmpdir(), "qo-worker-fallback-"));
		const store = new QueueStore(join(base, "state"));
		const runStore = new RunStore(join(base, "runs"));
		const seenProviders: (string | undefined)[] = [];
		// grok's `classifyUnavailable` matches its own wording (quota/rate-limit),
		// not claude's "session limit" phrasing — each provider fails in ITS
		// own voice so both hops are genuinely unavailable, not just the first.
		const grokUnavailableResult: RunResult = {
			...sessionLimitResult,
			resultText: "429 rate limit exceeded",
		};
		// claude → the injected executeClaude; grok → the mocked executeRun.
		vi.mocked(executeRun).mockImplementation((adapter, execOpts) => {
			execOpts.onSpawned?.(5321);
			seenProviders.push(adapter.name);
			return Promise.resolve(grokUnavailableResult);
		});
		const deps: WorkerDeps = {
			store,
			runStore,
			exec: async () => ({ stdout: "", exitCode: 0 }),
			executeClaude: async (spawnOpts) => {
				const provider = (spawnOpts as unknown as { provider?: string })
					.provider;
				seenProviders.push(provider);
				return sessionLimitResult;
			},
			redact: makeRedactor(new Map()),
			loadDef: () => null,
			worktreePath: async () => "/wt/path",
			defaults: { model: "sonnet", timeoutMs: 60_000 },
		};
		const t = store.create({
			prompt: "do it\n",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, {
			target: { repo: "platform", ref: "temp", worktree: "tmp-x" },
		});

		// Attempt 1: claude fails, chain still has grok → retry, re-queued.
		const first = await runTask(t.id, { ...deps, providers: PROVIDERS });
		expect(first.status).toBe("queued");
		expect(first.attemptedProviders).toEqual(["claude"]);

		// Attempt 2: grok fails too, but the filtered chain is now down to
		// grok alone (length 1) — no next entry, so this settles terminal
		// instead of retrying a third time.
		const second = await runTask(t.id, { ...deps, providers: PROVIDERS });
		expect(second.status).toBe("failed");
		expect(second.error).toBe("provider unavailable");
		// attemptedProviders is NOT bumped on the terminal settle — only the
		// `retry: true` hop appends (see worker.ts / engine-fallback.test.ts), so
		// a manual re-run can still resolve back onto grok (its limit may reset).
		expect(second.attemptedProviders).toEqual(["claude"]); // no third hop recorded
		expect(seenProviders).toEqual(["claude", "grok"]);

		// Attempt 2's report.md carries the WHOLE trail: attempt 1's hop
		// (preserved across attempt 2's writeSnapshot rewrite) plus attempt 2's
		// terminal line, which has no "→ falling back" suffix (finding 5).
		const report = reportOf(runStore, t.id);
		expect(report).toContain(
			"attempt 1: claude — session limit → falling back",
		);
		expect(report).toContain("attempt 2: grok — provider unavailable");
		expect(report).not.toContain(
			"attempt 2: grok — provider unavailable → falling back",
		);
	});
});
