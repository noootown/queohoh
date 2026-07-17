import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { BUILTIN_CATALOG } from "../catalog.js";
import type { TaskDefinition } from "../definition.js";
import { makeRedactor } from "../redact.js";
import type { Exec } from "../resolver-io.js";
import { RunStore } from "../run-store.js";
import type { RunResult } from "../runner.js";
import { SessionLineageStore } from "../session-lineage.js";
import { QueueStore } from "../store.js";
import type { SessionMode } from "../task.js";
import type { WorkerDeps } from "../worker.js";
import { finalizeRun, runTask, startRun } from "../worker.js";

const okResult: RunResult = {
	exitCode: 0,
	timedOut: false,
	signal: null,
	sessionId: "s",
	resultText: "did it",
	stderr: "",
	usage: { costUsd: 0.1, turns: 1, durationMs: 100 },
};

function makeDeps(overrides: Partial<WorkerDeps> = {}) {
	const base = mkdtempSync(join(tmpdir(), "qo-worker-"));
	const store = new QueueStore(join(base, "state"));
	const runStore = new RunStore(join(base, "runs"));
	const hookCalls: string[] = [];
	const gitClean: Exec = async (cmd, args) => {
		// git calls (status guard, branch read) never count as hooks; hooks shell
		// out via /bin/bash. Branch read returns empty here — tests that assert on
		// the derived branch/ticket supply their own exec.
		if (cmd === "git") return { stdout: "", exitCode: 0 };
		hookCalls.push(args.join(" ").replace("-lc ", ""));
		return { stdout: "", exitCode: 0 };
	};
	const deps: WorkerDeps = {
		store,
		runStore,
		exec: gitClean,
		executeClaude: async () => okResult,
		redact: makeRedactor(new Map()),
		loadDef: () => null,
		worktreePath: async () => "/wt/path",
		defaults: { timeoutMs: 60_000 },
		// A model-less task resolves against `defaultModels`; the built-in catalog
		// maps `claude/sonnet` → `claude-sonnet-5` (the id these tests assert on).
		catalog: BUILTIN_CATALOG,
		defaultModels: ["claude/sonnet"],
		activeProvider: "claude",
		...overrides,
	};
	return { deps, store, runStore, hookCalls };
}

interface EnqueueOpts {
	definition?: string;
	resumeSessionId?: string;
	session?: SessionMode;
}

// Second arg is either a definition string (existing callers) or an options
// object carrying resume/session overrides (lineage tests).
const enqueue = (store: QueueStore, opts: string | EnqueueOpts = {}) => {
	const o: EnqueueOpts = typeof opts === "string" ? { definition: opts } : opts;
	return store.create({
		prompt: "do it\n",
		repo: "platform",
		ref: "temp",
		source: "tui",
		definition: o.definition,
		item: o.definition ? { number: "1" } : undefined,
		itemKey: o.definition ? "1" : undefined,
		resumeSessionId: o.resumeSessionId,
		session: o.session,
	});
};

/** A verify executor recording each invocation and returning a scripted result. */
function fakeVerify(
	result: { exitCode: number; timedOut?: boolean; output?: string },
	calls: string[] = [],
) {
	return async (opts: { command: string }) => {
		calls.push(opts.command);
		return {
			exitCode: result.exitCode,
			timedOut: result.timedOut ?? false,
			signal: null,
			output: result.output ?? "",
		};
	};
}

function withWorktree(store: QueueStore, id: string) {
	return store.update(id, {
		target: { repo: "platform", ref: "temp", worktree: "tmp-x" },
	});
}

describe("runTask", () => {
	it("happy path: adhoc task ends done with report + snapshot", async () => {
		const { deps, store, runStore } = makeDeps();
		const t = enqueue(store);
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		expect(result.error).toBeNull();
		const meta = runStore.readRunMeta(t.id);
		expect(meta?.outcome).toBe("done");
		// A model-less task resolves against `defaultModels` (["claude/sonnet"])
		// through the built-in catalog → the concrete `claude-sonnet-5` id.
		expect(meta?.model).toBe("claude-sonnet-5");
		expect(runStore.readWorkerPid(t.id)).toBe(process.pid);
	});

	it("stamps startedAt at run start, re-stamping on a re-run", async () => {
		const { deps, store } = makeDeps();
		const t = enqueue(store);
		withWorktree(store, t.id);
		const first = await runTask(t.id, deps);
		// Stamped, and no earlier than the task's creation.
		expect(first.startedAt).toBeTruthy();
		expect(Date.parse(first.startedAt ?? "")).toBeGreaterThanOrEqual(
			Date.parse(first.created),
		);
		// Simulate a re-run of a task whose prior run started long ago: the timer
		// must anchor on THIS run, not the stale stamp.
		const stale = "2020-01-01T00:00:00.000Z";
		store.update(t.id, { status: "queued", startedAt: stale });
		const second = await runTask(t.id, deps);
		expect(second.startedAt).not.toBe(stale);
		expect(Date.parse(second.startedAt ?? "")).toBeGreaterThan(
			Date.parse(stale),
		);
	});

	it("nonzero exit → failed with exit reason", async () => {
		const { deps, store } = makeDeps({
			executeClaude: async () => ({ ...okResult, exitCode: 3 }),
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("failed");
		expect(result.error).toBe("exit code 3");
	});

	it("timeout → failed with timed out reason", async () => {
		const { deps, store } = makeDeps({
			executeClaude: async () => ({ ...okResult, timedOut: true, exitCode: 1 }),
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		expect((await runTask(t.id, deps)).error).toBe("timed out");
	});

	it("session limit message in result text → failed with session limit reason", async () => {
		const { deps, store } = makeDeps({
			executeClaude: async () => ({
				...okResult,
				exitCode: 1,
				resultText:
					"You've hit your session limit · resets 1pm (America/Chicago)",
			}),
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		expect((await runTask(t.id, deps)).error).toBe("session limit");
	});

	it("credit-balance message in result text → failed with out of budget reason", async () => {
		const { deps, store } = makeDeps({
			executeClaude: async () => ({
				...okResult,
				exitCode: 1,
				resultText:
					"Your credit balance is too low to access the Anthropic API.",
			}),
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		expect((await runTask(t.id, deps)).error).toBe("out of budget");
	});

	it("out of budget wins over session limit when both messages appear", async () => {
		// The billing signal is the more specific/actionable one (needs a top-up),
		// so it is checked first — a rerun after a session-limit reset still fails
		// while the account is out of credits.
		const { deps, store } = makeDeps({
			executeClaude: async () => ({
				...okResult,
				exitCode: 1,
				resultText:
					"You've hit your usage limit. Your credit balance is too low.",
			}),
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		expect((await runTask(t.id, deps)).error).toBe("out of budget");
	});

	it("nonzero exit without a session-limit message keeps the generic exit reason", async () => {
		const { deps, store } = makeDeps({
			executeClaude: async () => ({
				...okResult,
				exitCode: 1,
				resultText: "something else went wrong",
			}),
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		expect((await runTask(t.id, deps)).error).toBe("exit code 1");
	});

	it("dirty tree alone no longer fails a run (dirty-check moved to per-def verify)", async () => {
		// Policy change: the universal dirty-tree guard punished `worktree: repo`
		// tasks for pre-existing dirt in the user's own checkout. Defs that want
		// the guard declare it as their `verify` command.
		const dirtyGit: Exec = async (_c, args) =>
			args.join(" ").includes("status")
				? { stdout: " M src/x.ts\n", exitCode: 0 }
				: { stdout: "", exitCode: 0 };
		const { deps, store } = makeDeps({ exec: dirtyGit });
		const t = enqueue(store);
		withWorktree(store, t.id);
		const done = await runTask(t.id, deps);
		expect(done.status).toBe("done");
		expect(done.error).toBeNull();
	});

	it("signal → failed with stopped reason, winning over exit code", async () => {
		// A stopped run: killed by SIGTERM with a non-zero exit. The signal
		// reason must win over the exit code.
		const { deps, store } = makeDeps({
			executeClaude: async () => ({
				...okResult,
				exitCode: 143,
				signal: "SIGTERM",
			}),
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		expect((await runTask(t.id, deps)).error).toBe("stopped (SIGTERM)");
	});

	it("signal WITH isCancelled → cancelled 'stopped by user' (a user Stop, not a failure)", async () => {
		const { deps, store } = makeDeps({
			executeClaude: async () => ({
				...okResult,
				exitCode: 143,
				signal: "SIGTERM",
			}),
			isCancelled: () => true,
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("cancelled");
		expect(result.error).toBe("stopped by user");
	});

	it("signal WITHOUT isCancelled → failed (external/OOM kill stays a failure)", async () => {
		const { deps, store } = makeDeps({
			executeClaude: async () => ({
				...okResult,
				exitCode: 137,
				signal: "SIGKILL",
			}),
			isCancelled: () => false,
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("failed");
		expect(result.error).toBe("stopped (SIGKILL)");
	});

	it("nonzero exit WITHOUT a signal but WITH isCancelled → cancelled 'stopped by user'", async () => {
		// A user Stop where Claude traps SIGTERM, cleans up its terminal, and
		// exits by CODE (no signal reported). The recorded user-cancel must still
		// win over the exit-code branch — otherwise a deliberate stop masquerades
		// as a `failed` run (the red ✗) instead of the cancelled `⊘`.
		const { deps, store } = makeDeps({
			executeClaude: async () => ({
				...okResult,
				exitCode: 143,
				signal: null,
			}),
			isCancelled: () => true,
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("cancelled");
		expect(result.error).toBe("stopped by user");
	});

	it("reports the spawned child pid via onSpawned", async () => {
		const seen: Array<{ id: string; pid: number }> = [];
		const { deps, store } = makeDeps({
			executeClaude: async (opts) => {
				opts.onSpawned?.(4242);
				return okResult;
			},
			onSpawned: (id, pid) => seen.push({ id, pid }),
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seen).toEqual([{ id: t.id, pid: 4242 }]);
	});

	it("definition task uses def model/timeout and runs hooks around claude", async () => {
		const def: TaskDefinition = {
			name: "pr-review",
			repo: "platform",
			discovery: null,
			description: null,
			cron: null,
			args: [{ name: "number" }],
			dedup: "none",
			worktree: "temp",
			lane: null,
			verify: null,
			preRun: "mise run setup",
			postRun: "echo done",
			model: "claude/opus",
			timeoutMs: 120_000,
			priority: "normal",
			prompt: "review {{number}}",
		};
		let claudeModel = "";
		const { deps, store, hookCalls, runStore } = makeDeps({
			loadDef: () => def,
			executeClaude: async (opts) => {
				claudeModel = opts.model;
				return okResult;
			},
		});
		const t = enqueue(store, "platform/pr-review");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		// def.model "claude/opus" resolves through the catalog → claude-opus-4-8.
		expect(claudeModel).toBe("claude-opus-4-8");
		expect(hookCalls).toEqual(["mise run setup", "echo done"]);
		expect(runStore.readRunMeta(t.id)?.model).toBe("claude-opus-4-8");
	});

	it("renders pre_run hooks with global/repo/item vars (item wins)", async () => {
		const def: TaskDefinition = {
			name: "pr-review",
			repo: "platform",
			discovery: null,
			description: null,
			cron: null,
			args: [{ name: "number" }],
			dedup: "none",
			worktree: "temp",
			lane: null,
			verify: null,
			preRun: "setup.sh {{number}} {{repo_slug}}",
			postRun: null,
			model: "claude/opus",
			timeoutMs: 120_000,
			priority: "normal",
			prompt: "review {{number}}",
		};
		const { deps, store, hookCalls } = makeDeps({
			loadDef: () => def,
			repoVars: { repo_slug: "org/repo" },
		});
		const t = store.create({
			prompt: "review 7\n",
			repo: "platform",
			ref: "temp",
			source: "tui",
			definition: "platform/pr-review",
			item: { number: "7" },
			itemKey: "7",
		});
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		expect(hookCalls).toEqual(["setup.sh 7 org/repo"]);
	});

	it("pre_run failure → failed, claude never runs, post_run still runs", async () => {
		const def: TaskDefinition = {
			name: "d",
			repo: "platform",
			discovery: null,
			description: null,
			cron: null,
			args: [],
			dedup: "none",
			worktree: "temp",
			lane: null,
			verify: null,
			preRun: "bad-setup",
			postRun: "cleanup",
			model: "claude/opus",
			timeoutMs: 60_000,
			priority: "normal",
			prompt: "p",
		};
		const calls: string[] = [];
		const exec: Exec = async (_c, args) => {
			const cmd = args[1] ?? "";
			calls.push(cmd);
			if (cmd === "bad-setup") return { stdout: "", exitCode: 1 };
			return { stdout: "", exitCode: 0 };
		};
		let claudeRan = false;
		const { deps, store } = makeDeps({
			exec,
			loadDef: () => def,
			executeClaude: async () => {
				claudeRan = true;
				return okResult;
			},
		});
		const t = enqueue(store, "platform/d");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("failed");
		expect(result.error).toContain("pre_run failed");
		expect(claudeRan).toBe(false);
		expect(calls).toContain("cleanup");
	});

	it("unresolved worktree path → failed", async () => {
		const { deps, store } = makeDeps({ worktreePath: async () => null });
		const t = enqueue(store);
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("failed");
		expect(result.error).toContain("worktree path not found");
	});

	it("definition set but loadDef returns null → failed with definition not found", async () => {
		const { deps, store } = makeDeps({ loadDef: () => null });
		const t = enqueue(store, "platform/ghost");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("failed");
		expect(result.error).toContain("definition not found");
	});

	it("renders execution-time {{branch}}/{{ticket}}/{{worktree}} into the prompt", async () => {
		const exec: Exec = async (cmd, args) => {
			if (cmd === "git") {
				if (args.join(" ").includes("rev-parse"))
					return { stdout: "jus-1008-fix-thing\n", exitCode: 0 };
				return { stdout: "", exitCode: 0 };
			}
			return { stdout: "", exitCode: 0 };
		};
		let claudePrompt = "";
		const { deps, store } = makeDeps({
			exec,
			executeClaude: async (opts) => {
				claudePrompt = opts.prompt;
				return okResult;
			},
		});
		const t = store.create({
			prompt: "work {{branch}} for {{ticket}} in {{worktree}}\n",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		// worktree name is "tmp-x" (from withWorktree); ticket derived JUS-1008.
		expect(claudePrompt).toBe(
			"work jus-1008-fix-thing for JUS-1008 in tmp-x\n",
		);
	});

	it("unknown placeholders stay literal; failed branch read leaves them empty", async () => {
		const exec: Exec = async (cmd, args) => {
			if (cmd === "git") {
				if (args.join(" ").includes("rev-parse"))
					return { stdout: "", exitCode: 1 }; // branch read fails
				return { stdout: "", exitCode: 0 };
			}
			return { stdout: "", exitCode: 0 };
		};
		let claudePrompt = "";
		const { deps, store } = makeDeps({
			exec,
			executeClaude: async (opts) => {
				claudePrompt = opts.prompt;
				return okResult;
			},
		});
		const t = store.create({
			prompt: "b=[{{branch}}] t=[{{ticket}}] {{nope}}\n",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		expect(claudePrompt).toBe("b=[] t=[] {{nope}}\n");
	});

	it("hooks fill worktree context at lowest precedence; explicit vars win", async () => {
		const def: TaskDefinition = {
			name: "d",
			repo: "platform",
			discovery: null,
			description: null,
			cron: null,
			args: [],
			dedup: "none",
			worktree: "temp",
			lane: null,
			verify: null,
			preRun: "run {{ticket}} {{branch}} {{worktree}}",
			postRun: null,
			model: "claude/opus",
			timeoutMs: 60_000,
			priority: "normal",
			prompt: "p",
		};
		const hookCalls: string[] = [];
		const exec: Exec = async (cmd, args) => {
			if (cmd === "git") {
				if (args.join(" ").includes("rev-parse"))
					return { stdout: "jus-99-x\n", exitCode: 0 };
				return { stdout: "", exitCode: 0 };
			}
			hookCalls.push(args.join(" ").replace("-lc ", ""));
			return { stdout: "", exitCode: 0 };
		};
		const { deps, store } = makeDeps({
			exec,
			loadDef: () => def,
			// explicit global `branch` must beat the worktree-derived one
			globalVars: { branch: "override-branch" },
		});
		const t = enqueue(store, "platform/d");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		// ticket JUS-99 + worktree tmp-x come from context; branch overridden.
		expect(hookCalls).toEqual(["run JUS-99 override-branch tmp-x"]);
	});

	it("post_run failure after done stays done but logs the failure", async () => {
		const def: TaskDefinition = {
			name: "d",
			repo: "platform",
			discovery: null,
			description: null,
			cron: null,
			args: [],
			dedup: "none",
			worktree: "temp",
			lane: null,
			verify: null,
			preRun: null,
			postRun: "cleanup",
			model: "claude/opus",
			timeoutMs: 60_000,
			priority: "normal",
			prompt: "p",
		};
		const exec: Exec = async (_c, args) => {
			const joined = args.join(" ");
			if (joined.includes("status")) return { stdout: "", exitCode: 0 };
			if (joined.includes("cleanup")) return { stdout: "", exitCode: 1 };
			return { stdout: "", exitCode: 0 };
		};
		const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
		const { deps, store, runStore } = makeDeps({ exec, loadDef: () => def });
		const t = enqueue(store, "platform/d");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		expect(result.error).toBeNull();
		expect(runStore.readRunMeta(t.id)?.reason).toBeNull();
		expect(errSpy).toHaveBeenCalledWith(
			expect.stringContaining("post_run failed"),
		);
		errSpy.mockRestore();
	});
});

describe("runTask model-ref resolution", () => {
	it("resolves a provider/label ref to its concrete id at spawn (snapshot + spawn)", async () => {
		// The catalog is authoritative now (no per-repo modelTable): a
		// `provider/label` ref lands as the provider-specific id on both the
		// spawn options and the run-store snapshot.
		let seenModel = "";
		const { deps, store, runStore } = makeDeps({
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
			model: "claude/sonnet",
		});
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		expect(seenModel).toBe("claude-sonnet-5");
		expect(runStore.readRunMeta(t.id)?.model).toBe("claude-sonnet-5");
	});

	it("resolves a provider/id-form ref to its concrete id (id-match fallback)", async () => {
		// A ref naming the raw model id (not the short label) still resolves —
		// `findModel` falls back to an id match within the provider group. The
		// canonical `provider/label` form is exercised in models.test.ts.
		let seenModel = "";
		const { deps, store } = makeDeps({
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
			model: "claude/claude-fable-5",
		});
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenModel).toBe("claude-fable-5");
	});
});

describe("runTask resume via lineage", () => {
	function lineageStore(): SessionLineageStore {
		return new SessionLineageStore(
			join(mkdtempSync(join(tmpdir(), "lin-")), "lineage.json"),
		);
	}

	it("pinned task resumes the tip of its pin's lineage", async () => {
		const lineage = lineageStore();
		lineage.recordFork("sess-x", "sess-y");
		let seenResume: string | undefined;
		const { deps, store } = makeDeps({
			lineage,
			executeClaude: async (opts) => {
				seenResume = opts.resumeSessionId;
				return { ...okResult, sessionId: "sess-z" };
			},
		});
		const task = withWorktree(
			store,
			enqueue(store, { resumeSessionId: "sess-x" }).id,
		);
		await runTask(task.id, deps);
		expect(seenResume).toBe("sess-y");
		// The run recorded its own fork: y → z.
		expect(lineage.tip("sess-x")).toBe("sess-z");
	});

	it("fresh task resumes nothing and records no fork", async () => {
		const lineage = lineageStore();
		let seenResume: string | undefined = "sentinel";
		const { deps, store } = makeDeps({
			lineage,
			executeClaude: async (opts) => {
				seenResume = opts.resumeSessionId;
				return { ...okResult, sessionId: "sess-new" };
			},
		});
		const task = withWorktree(store, enqueue(store).id);
		await runTask(task.id, deps);
		expect(seenResume).toBeUndefined();
		expect(lineage.tip("sess-new")).toBe("sess-new");
	});

	it("session:'main' is treated as fresh (deprecated)", async () => {
		const lineage = lineageStore();
		let seenResume: string | undefined = "sentinel";
		const { deps, store } = makeDeps({
			lineage,
			executeClaude: async (opts) => {
				seenResume = opts.resumeSessionId;
				return okResult;
			},
		});
		const task = withWorktree(store, enqueue(store, { session: "main" }).id);
		await runTask(task.id, deps);
		expect(seenResume).toBeUndefined();
	});

	it("two chains in one lane stay isolated (the old lane-pointer hazard)", async () => {
		const lineage = lineageStore();
		const resumes: (string | undefined)[] = [];
		let n = 0;
		const { deps, store } = makeDeps({
			lineage,
			executeClaude: async (opts) => {
				resumes.push(opts.resumeSessionId);
				n += 1;
				return { ...okResult, sessionId: `out-${n}` };
			},
		});
		const a = withWorktree(
			store,
			enqueue(store, { resumeSessionId: "sess-1" }).id,
		);
		const b = withWorktree(
			store,
			enqueue(store, { resumeSessionId: "sess-3" }).id,
		);
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
		const { deps, store } = makeDeps({
			lineage,
			executeClaude: async (opts) => {
				resumes.push(opts.resumeSessionId);
				n += 1;
				return { ...okResult, sessionId: `hop-${n}` };
			},
		});
		const a = withWorktree(
			store,
			enqueue(store, { resumeSessionId: "sess-x" }).id,
		);
		const b = withWorktree(
			store,
			enqueue(store, { resumeSessionId: "sess-x" }).id,
		);
		await runTask(a.id, deps);
		await runTask(b.id, deps);
		expect(resumes).toEqual(["sess-x", "hop-1"]);
		expect(lineage.tip("sess-x")).toBe("hop-2");
	});
});

describe("runTask pinned resume model resolution", () => {
	const enqueuePinned = (store: QueueStore, model?: string) =>
		store.create({
			prompt: "continue\n",
			repo: "platform",
			ref: "temp",
			source: "mcp",
			resumeSessionId: "pin-sess",
			model,
		});

	it("task.model overrides defaults; definition model still wins", async () => {
		let seenModel = "";
		const { deps, store } = makeDeps({
			executeClaude: async (opts) => {
				seenModel = opts.model;
				return okResult;
			},
		});
		const t = enqueuePinned(store, "claude/fable");
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenModel).toBe("claude-fable-5");
	});

	it("def.model beats task.model", async () => {
		const def: TaskDefinition = {
			name: "d",
			repo: "platform",
			discovery: null,
			description: null,
			cron: null,
			args: [],
			dedup: "none",
			worktree: "temp",
			lane: null,
			verify: null,
			preRun: null,
			postRun: null,
			model: "claude/opus",
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
			model: "claude/fable",
		});
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		// def.model "claude/opus" wins over the task's own "claude/fable".
		expect(seenModel).toBe("claude-opus-4-8");
	});
});

describe("runTask timeout precedence", () => {
	it("falls back to deps.defaults.timeoutMs when neither def nor task set one", async () => {
		let seenTimeoutMs = 0;
		const { deps, store } = makeDeps({
			executeClaude: async (opts) => {
				seenTimeoutMs = opts.timeoutMs;
				return okResult;
			},
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenTimeoutMs).toBe(60_000); // deps.defaults.timeoutMs in makeDeps
	});

	it("task.timeout_ms overrides the daemon default; definition timeout still wins", async () => {
		let seenTimeoutMs = 0;
		const { deps, store } = makeDeps({
			executeClaude: async (opts) => {
				seenTimeoutMs = opts.timeoutMs;
				return okResult;
			},
		});
		const t = store.create({
			prompt: "p\n",
			repo: "platform",
			ref: "temp",
			source: "mcp",
			timeoutMs: 900_000,
		});
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenTimeoutMs).toBe(900_000);
	});

	it("def.timeoutMs beats task.timeoutMs", async () => {
		const def: TaskDefinition = {
			name: "d",
			repo: "platform",
			discovery: null,
			description: null,
			cron: null,
			args: [],
			dedup: "none",
			worktree: "temp",
			lane: null,
			verify: null,
			preRun: null,
			postRun: null,
			model: "claude/opus",
			timeoutMs: 45_000,
			priority: "normal",
			prompt: "p",
		};
		let seenTimeoutMs = 0;
		const { deps, store } = makeDeps({
			loadDef: () => def,
			executeClaude: async (opts) => {
				seenTimeoutMs = opts.timeoutMs;
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
			timeoutMs: 900_000,
		});
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenTimeoutMs).toBe(45_000);
	});
});

describe("runTask verify (done-condition)", () => {
	const enqueueVerify = (store: QueueStore, verify: string) =>
		store.create({
			prompt: "do it\n",
			repo: "platform",
			ref: "temp",
			source: "tui",
			verify,
		});

	it("verify passes → task stays done and records verified:true", async () => {
		const calls: string[] = [];
		const { deps, store, runStore } = makeDeps({
			executeVerify: fakeVerify({ exitCode: 0, output: "ok\n" }, calls),
		});
		const t = enqueueVerify(store, "test -f dist/cli.js");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		expect(result.error).toBeNull();
		expect(result.verified).toBe(true);
		expect(result.verifyExitCode).toBe(0);
		expect(result.verify).toBe("test -f dist/cli.js");
		expect(calls).toEqual(["test -f dist/cli.js"]);
		// Persisted to the run-store data.json too.
		const meta = runStore.readRunMeta(t.id);
		expect(meta?.outcome).toBe("done");
		expect(meta?.verified).toBe(true);
	});

	it("verify exits non-zero → verify-failed, output captured, reason set", async () => {
		const { deps, store, runStore } = makeDeps({
			executeVerify: fakeVerify({ exitCode: 2, output: "label missing\n" }),
		});
		const t = enqueueVerify(store, "check-label.sh");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("verify-failed");
		expect(result.error).toBe("verify failed (exit 2)");
		expect(result.verified).toBe(false);
		expect(result.verifyExitCode).toBe(2);
		expect(result.verifyOutput).toBe("label missing\n");
		// The report tab (report.md) surfaces the verdict + output tail.
		const report = readFileSync(
			join(runStore.runsDir, t.id, "report.md"),
			"utf-8",
		);
		expect(report).toContain("## Verify");
		expect(report).toContain("label missing");
		expect(runStore.readRunMeta(t.id)?.outcome).toBe("verify-failed");
	});

	it("verify output is stored ANSI-stripped with \\r overwrites resolved", async () => {
		// Test runners (vitest) emit colored, spinner-overwritten output; stored
		// raw it renders as garbage in the TUI (ratatui drops the ESC byte and
		// prints the `[2m` tail literally). Capture must strip ANSI sequences and
		// keep only the final \r-overwrite segment per line.
		const { deps, store } = makeDeps({
			executeVerify: fakeVerify({
				exitCode: 2,
				output:
					"\x1b[90mstderr\x1b[2m | api.test.ts\x1b[22m gone\nspin\rspun\rfinal\ncrlf line\r\n",
			}),
		});
		const t = enqueueVerify(store, "check.sh");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.verifyOutput).toBe(
			"stderr | api.test.ts gone\nfinal\ncrlf line\n",
		);
	});

	it("verify times out → verify-failed with a timed-out reason and null exit", async () => {
		const { deps, store } = makeDeps({
			executeVerify: fakeVerify({
				exitCode: 1,
				timedOut: true,
				output: "still running...",
			}),
		});
		const t = enqueueVerify(store, "sleep 999");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("verify-failed");
		expect(result.error).toBe("verify timed out");
		expect(result.verified).toBe(false);
		expect(result.verifyExitCode).toBeNull();
	});

	it("no verify command → behavior unchanged, executor never called", async () => {
		const calls: string[] = [];
		const { deps, store } = makeDeps({
			executeVerify: fakeVerify({ exitCode: 1 }, calls),
		});
		const t = enqueue(store); // no verify configured
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		expect(result.verified).toBeNull();
		expect(calls).toEqual([]);
	});

	it("does NOT run verify when the run already failed", async () => {
		const calls: string[] = [];
		const { deps, store } = makeDeps({
			executeClaude: async () => ({ ...okResult, exitCode: 3 }),
			executeVerify: fakeVerify({ exitCode: 0 }, calls),
		});
		const t = enqueueVerify(store, "should-not-run");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("failed");
		expect(result.error).toBe("exit code 3");
		expect(calls).toEqual([]);
		expect(result.verified).toBeNull();
	});

	it("uses the definition's verify (live) and renders worktree context", async () => {
		const def: TaskDefinition = {
			name: "pr-ready",
			repo: "platform",
			discovery: null,
			description: null,
			cron: null,
			args: [],
			dedup: "none",
			worktree: "temp",
			lane: null,
			preRun: null,
			postRun: null,
			verify: "check {{ticket}} {{worktree}}",
			model: "claude/opus",
			timeoutMs: 60_000,
			priority: "normal",
			prompt: "p",
		};
		const calls: string[] = [];
		const exec: Exec = async (cmd, args) => {
			if (cmd === "git" && args.join(" ").includes("rev-parse"))
				return { stdout: "jus-42-x\n", exitCode: 0 };
			return { stdout: "", exitCode: 0 };
		};
		const { deps, store } = makeDeps({
			exec,
			loadDef: () => def,
			executeVerify: fakeVerify({ exitCode: 0 }, calls),
		});
		const t = enqueue(store, "platform/pr-ready");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		// worktree name is "tmp-x" (withWorktree); ticket derived from the branch.
		expect(calls).toEqual(["check JUS-42 tmp-x"]);
		// The definition's command is stamped onto the task record.
		expect(result.verify).toBe("check {{ticket}} {{worktree}}");
	});

	it("redacts secrets in the persisted verify output", async () => {
		const { deps, store } = makeDeps({
			redact: makeRedactor(new Map([["sk-secret-123", "API_TOKEN"]])),
			executeVerify: fakeVerify({
				exitCode: 2,
				output: "auth failed with token sk-secret-123\n",
			}),
		});
		const t = enqueueVerify(store, "deploy-check.sh");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("verify-failed");
		expect(result.verifyOutput).not.toContain("sk-secret-123");
	});
});

describe("startRun / finalizeRun split", () => {
	it("startRun returns a SpawnSpec carrying the rendered prompt + resolved model", async () => {
		const { deps, store } = makeDeps();
		const t = enqueue(store);
		withWorktree(store, t.id);
		const s = await startRun(t.id, deps);
		expect(s.kind).toBe("spawn");
		if (s.kind !== "spawn") throw new Error("expected spawn");
		// No model on the task → `defaultModels` (["claude/sonnet"]) heads it.
		expect(s.spec.model).toBe("claude-sonnet-5");
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
		const settled = await finalizeRun(t.id, { ...okResult, exitCode: 3 }, deps);
		expect(settled.retry).toBe(false);
		expect(settled.task.status).toBe("failed");
		expect(settled.task.error).toBe("exit code 3");
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
