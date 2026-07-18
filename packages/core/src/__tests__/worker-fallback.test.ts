import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { BUILTIN_CATALOG } from "../catalog.js";
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
	usage: {
		costUsd: 0.1,
		turns: 1,
		durationMs: 100,
		inputTokens: null,
		outputTokens: null,
	},
};

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

// Two enabled providers — the exact fallback shape (claude → grok) the
// design spec walks through in its worked example. Per-provider model tables
// are gone (model catalog design spec Section 1); model ids now come from
// the catalog, not this fixture.
const PROVIDERS: ProviderConfig[] = [
	{ name: "claude", enabled: true },
	{ name: "grok", enabled: true },
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
	/** Task's explicit model spec; omit to leave the task model-less so the
	 * chain comes from `defaultModels` (the default two-entry claude→grok list). */
	model?: string | string[];
	/** True to stamp `model_pinned` (an explicit TUI dialog pick) onto the
	 * task — see the "runTask activeProvider vs resume pin" pinned-vs-unpinned
	 * pair. Omit/false leaves the task unpinned (today's re-heading behavior). */
	modelPinned?: boolean;
	defaultModels?: string[];
	activeProvider?: string;
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
		defaults: { timeoutMs: 60_000 },
		catalog: BUILTIN_CATALOG,
		// Two-entry default chain — the claude → grok shape the design spec's
		// worked example walks. A model-less task heads onto claude, falls to grok.
		defaultModels: opts.defaultModels ?? ["claude/sonnet", "grok/grok-4.5"],
		activeProvider: opts.activeProvider ?? "claude",
	};
	const t = store.create({
		prompt: "do it\n",
		repo: "platform",
		ref: "temp",
		source: "tui",
		resumeSessionId: opts.resumeSessionId,
		model: opts.model,
		modelPinned: opts.modelPinned,
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
		expect(first.attemptedModels).toContain("claude");
		// Retry: re-queued, not settled failed.
		expect(first.status).not.toBe("failed");
		expect(first.status).toBe("queued");
		expect(seenProviders).toEqual(["claude"]);

		// The hop is rendered into THIS attempt's report.md "## Attempts" trail
		// with the "→ falling back" suffix. The trail names the head entry's
		// `provider/label` REF (claude/sonnet), not the bare provider name that
		// `attemptedModels` records for the group skip.
		const firstReport = reportOf(runStore, taskId);
		expect(firstReport).toContain("## Attempts");
		expect(firstReport).toContain(
			"attempt 1: claude/sonnet — session limit → falling back",
		);

		// The engine (Task 10) would re-drive automatically; the in-process
		// caller here just invokes runTask again, which re-resolves the chain
		// minus attemptedModels and lands on grok.
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
		// recorded in attemptedModels (a settled failure isn't a "hop").
		expect(seenProviders).toEqual(["claude"]);
		expect(out.attemptedModels).toEqual([]);
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
		// The default chain puts claude FIRST — the pin overrides that, going
		// straight to grok because that's what "s1" is tagged with.
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
			defaults: { timeoutMs: 60_000 },
			catalog: BUILTIN_CATALOG,
			defaultModels: ["claude/sonnet", "grok/grok-4.5"],
			activeProvider: "claude",
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
		expect(first.attemptedModels).toEqual(["claude"]);

		// Attempt 2: grok fails too, but the filtered chain is now down to
		// grok alone (length 1) — no next entry, so this settles terminal
		// instead of retrying a third time.
		const second = await runTask(t.id, { ...deps, providers: PROVIDERS });
		expect(second.status).toBe("failed");
		expect(second.error).toBe("provider unavailable");
		// attemptedModels is NOT bumped on the terminal settle — only the
		// `retry: true` hop appends (see worker.ts / engine-fallback.test.ts), so
		// a manual re-run can still resolve back onto grok (its limit may reset).
		expect(second.attemptedModels).toEqual(["claude"]); // no third hop recorded
		expect(seenProviders).toEqual(["claude", "grok"]);

		// Attempt 2's report.md carries the WHOLE trail: attempt 1's hop
		// (preserved across attempt 2's writeSnapshot rewrite) plus attempt 2's
		// terminal line, which has no "→ falling back" suffix (finding 5).
		const report = reportOf(runStore, t.id);
		expect(report).toContain(
			"attempt 1: claude/sonnet — session limit → falling back",
		);
		expect(report).toContain("attempt 2: grok/grok-4.5 — provider unavailable");
		expect(report).not.toContain(
			"attempt 2: grok/grok-4.5 — provider unavailable → falling back",
		);
	});
});

describe("runTask chain rotation + provider-group skip", () => {
	it("two-entry list rotates claude → grok on a session limit", async () => {
		const { deps, providers, taskId, seenProviders } = makeFallbackDeps({
			firstResult: sessionLimitResult,
			model: ["claude/opus", "grok/grok-4.5"],
		});
		const first = await runTask(taskId, { ...deps, providers });
		expect(first.status).toBe("queued");
		// The bare provider name is what lands in attemptedModels (group skip).
		expect(first.attemptedModels).toEqual(["claude"]);
		const second = await runTask(taskId, { ...deps, providers });
		expect(second.status).toBe("done");
		expect(seenProviders).toEqual(["claude", "grok"]);
	});

	it("single-entry list settles terminal (no retry) on an availability failure", async () => {
		const { deps, providers, taskId, seenProviders } = makeFallbackDeps({
			firstResult: sessionLimitResult,
			model: ["claude/opus"],
		});
		const out = await runTask(taskId, { ...deps, providers });
		// One entry, nowhere to hop → the availability failure settles terminal.
		expect(out.status).toBe("failed");
		expect(out.error).toBe("session limit");
		// A terminal settle never bumps attemptedModels (a re-run may resolve back
		// onto claude once its limit resets).
		expect(out.attemptedModels).toEqual([]);
		expect(seenProviders).toEqual(["claude"]);
	});

	it("provider-group skip: a failed claude entry skips ALL claude entries, hops to grok", async () => {
		const { deps, providers, taskId, seenProviders } = makeFallbackDeps({
			firstResult: sessionLimitResult,
			model: ["claude/opus", "claude/sonnet", "grok/grok-4.5"],
		});
		const first = await runTask(taskId, { ...deps, providers });
		expect(first.status).toBe("queued");
		expect(first.attemptedModels).toEqual(["claude"]);
		const second = await runTask(taskId, { ...deps, providers });
		expect(second.status).toBe("done");
		// claude/opus availability-failed → the whole claude group is attempted,
		// so claude/sonnet is never tried; the next hop is grok.
		expect(seenProviders).toEqual(["claude", "grok"]);
	});

	it("legacy attemptedProviders:['claude'] (surfaced as attemptedModels) skips every claude entry", async () => {
		const { deps, providers, taskId, seenProviders } = makeFallbackDeps({
			firstResult: okResult,
			model: ["claude/opus", "claude/sonnet", "grok/grok-4.5"],
		});
		// A pre-catalog task file's `attempted_providers: [claude]` surfaces as
		// attemptedModels via task.ts read-compat (covered in task-attempted.test.ts);
		// the worker's group-skip filter must drop BOTH claude entries on that value.
		deps.store.update(taskId, { attemptedModels: ["claude"] });
		const out = await runTask(taskId, { ...deps, providers });
		expect(out.status).toBe("done");
		expect(seenProviders).toEqual(["grok"]);
	});
});

describe("runTask activeProvider vs resume pin", () => {
	it("a fresh run re-heads onto the switched-to provider (activeProvider=grok)", async () => {
		const { deps, providers, taskId, seenProviders } = makeFallbackDeps({
			firstResult: okResult,
			activeProvider: "grok",
		});
		const out = await runTask(taskId, { ...deps, providers });
		expect(out.status).toBe("done");
		// The default chain is claude-first; the switch re-heads it onto grok.
		expect(seenProviders).toEqual(["grok"]);
	});

	it("a non-pinned task.model naming another provider still re-heads onto active (today's behavior)", async () => {
		// task.model explicitly names claude/opus, but no model_pinned stamp —
		// resolveModelChain still injects the active provider's (grok) default
		// from the pool (grok/grok-4.5), exactly like the model-less case above.
		// Contrast with the next test, where model_pinned suppresses the re-head.
		const { deps, providers, taskId, seenProviders } = makeFallbackDeps({
			firstResult: okResult,
			model: "claude/opus",
			activeProvider: "grok",
		});
		const out = await runTask(taskId, { ...deps, providers });
		expect(out.status).toBe("done");
		expect(seenProviders).toEqual(["grok"]);
	});

	it("model_pinned suppresses the active-provider re-head — runs exactly the pinned ref", async () => {
		const { deps, providers, taskId, seenProviders } = makeFallbackDeps({
			firstResult: okResult,
			model: "claude/opus",
			modelPinned: true,
			activeProvider: "grok",
		});
		const out = await runTask(taskId, { ...deps, providers });
		expect(out.status).toBe("done");
		// No grok head prepended — runs claude directly, as picked.
		expect(seenProviders).toEqual(["claude"]);
	});

	it("a resume pin ignores activeProvider (follows its session's tagged provider)", async () => {
		const lineage = new SessionLineageStore(
			join(mkdtempSync(join(tmpdir(), "qo-lineage-")), "lineage.json"),
		);
		lineage.recordProvider("s1", "claude");
		const { deps, providers, taskId, seenProviders } = makeFallbackDeps({
			firstResult: okResult,
			resumeSessionId: "s1",
			activeProvider: "grok",
		});
		const out = await runTask(taskId, { ...deps, providers, lineage });
		expect(out.status).toBe("done");
		// activeProvider=grok would re-head a fresh run; the resume pin overrides
		// it, staying on claude because that's what s1 is tagged with.
		expect(seenProviders).toEqual(["claude"]);
	});

	it("model-less task with an EMPTY defaultModels still heads onto the active provider", async () => {
		// Deliberate decision (documented on WorkerDeps.defaultModels): the worker
		// passes defaultModels through verbatim and does NOT treat [] as "no
		// runnable model". resolveModelChain still prepends the enabled active
		// provider's group head, so the task runs — here on grok's head (grok-4.5).
		const { deps, providers, taskId, seenProviders, runStore } =
			makeFallbackDeps({
				firstResult: okResult,
				defaultModels: [],
				activeProvider: "grok",
			});
		const out = await runTask(taskId, { ...deps, providers });
		expect(out.status).toBe("done");
		expect(seenProviders).toEqual(["grok"]);
		expect(runStore.readRunMeta(taskId)?.model).toBe("grok-4.5");
	});
});

// Every resume test above (and in worker.test.ts's "pinned resume model
// resolution" describe) resolves via the `pinnedEntry !== undefined` branch
// at worker.ts:243-245 — the pin's provider always happens to already be in
// the resolved chain. These three tests drive the `else` branch
// (worker.ts:246-278) instead, where the pin is ABSENT from the chain and
// has to be derived straight from the catalog.
describe("runTask pinned resume — pin absent from resolved chain", () => {
	it("groupHead sub-path: spec names only a different provider, resolves the pin's group head", async () => {
		const lineage = new SessionLineageStore(
			join(mkdtempSync(join(tmpdir(), "qo-lineage-")), "lineage.json"),
		);
		lineage.recordProvider("s1", "grok");
		const { deps, providers, taskId, seenProviders, runStore } =
			makeFallbackDeps({
				firstResult: okResult,
				resumeSessionId: "s1",
				// A single string ref naming only claude — resolveModelChain never
				// puts a grok entry in the chain, so the pin (grok) is absent and
				// the else-branch has to derive it straight from the catalog.
				model: "claude/opus",
			});
		const out = await runTask(taskId, { ...deps, providers, lineage });
		expect(out.status).toBe("done");
		expect(seenProviders).toEqual(["grok"]);
		// The spec names no grok ref at all, so `refEntry` never matches and the
		// else-branch falls through to `groupHead` — grok's most powerful model
		// (grok-4.5), the group head, not some other grok entry.
		expect(runStore.readRunMeta(taskId)?.model).toBe("grok-4.5");
	});

	it("ref-match sub-path: spec names the pinned provider directly, once attemptedModels has filtered it out of the chain", async () => {
		const lineage = new SessionLineageStore(
			join(mkdtempSync(join(tmpdir(), "qo-lineage-")), "lineage.json"),
		);
		lineage.recordProvider("s1", "grok");
		const { deps, providers, taskId, seenProviders, runStore } =
			makeFallbackDeps({
				firstResult: okResult,
				resumeSessionId: "s1",
				// Names grok explicitly, but its SECOND entry (composer), not the
				// group head (grok-4.5) — resolving to composer is the only way to
				// prove this took the ref-match path rather than groupHead, which
				// would also happen to land on a grok model but the wrong one.
				model: "grok/composer",
			});
		// If left alone, resolveModelChain would put `grok/composer` right into
		// the chain (findModel resolves it, grok is enabled) — the initial
		// `chain.find(pinnedProvider)` would find it directly, hitting the
		// `pinnedEntry`-found branch instead of the one under test. Marking it
		// already attempted is the only way to force it out of `chain` (worker.ts
		// line 221's filter) while `modelSpec` — read straight off the task, not
		// off `chain` — still names grok: this is the sole reachable path into
		// the ref-match sub-path (worker.ts:259-264).
		deps.store.update(taskId, { attemptedModels: ["grok/composer"] });
		const out = await runTask(taskId, { ...deps, providers, lineage });
		expect(out.status).toBe("done");
		expect(seenProviders).toEqual(["grok"]);
		expect(runStore.readRunMeta(taskId)?.model).toBe("grok-composer-2.5-fast");
	});

	it("resume provider unavailable: the pin's provider is disabled in config", async () => {
		const lineage = new SessionLineageStore(
			join(mkdtempSync(join(tmpdir(), "qo-lineage-")), "lineage.json"),
		);
		lineage.recordProvider("s1", "grok");
		const { deps, taskId } = makeFallbackDeps({
			firstResult: okResult,
			resumeSessionId: "s1",
			model: "claude/opus",
		});
		const providers: ProviderConfig[] = [
			{ name: "claude", enabled: true },
			{ name: "grok", enabled: false },
		];
		const out = await runTask(taskId, { ...deps, providers, lineage });
		expect(out.status).toBe("failed");
		expect(out.error).toBe("resume provider unavailable: grok");
	});
});
