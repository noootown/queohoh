import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import type { TaskDefinition } from "../definition.js";
import { MainSessionStore } from "../main-sessions.js";
import { makeRedactor } from "../redact.js";
import type { Exec } from "../resolver-io.js";
import { RunStore } from "../run-store.js";
import type { RunResult } from "../runner.js";
import { QueueStore } from "../store.js";
import type { WorkerDeps } from "../worker.js";
import { runTask } from "../worker.js";

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
		defaults: { model: "sonnet", timeoutMs: 60_000 },
		...overrides,
	};
	return { deps, store, runStore, hookCalls };
}

const enqueue = (store: QueueStore, definition?: string) =>
	store.create({
		prompt: "do it\n",
		repo: "platform",
		ref: "temp",
		source: "tui",
		definition,
		item: definition ? { number: "1" } : undefined,
		itemKey: definition ? "1" : undefined,
	});

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
		expect(meta?.model).toBe("sonnet");
		expect(runStore.readWorkerPid(t.id)).toBe(process.pid);
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

	it("dirty tree → failed with tree left dirty", async () => {
		const dirtyGit: Exec = async (_c, args) =>
			args.join(" ").includes("status")
				? { stdout: " M src/x.ts\n", exitCode: 0 }
				: { stdout: "", exitCode: 0 };
		const { deps, store } = makeDeps({ exec: dirtyGit });
		const t = enqueue(store);
		withWorktree(store, t.id);
		expect((await runTask(t.id, deps)).error).toBe("tree left dirty");
	});

	it("signal → failed with stopped reason, winning over exit code and a dirty tree", async () => {
		// A stopped run: killed by SIGTERM, non-zero exit, and it left the tree
		// dirty. The signal reason must win over both.
		const dirtyGit: Exec = async (_c, args) =>
			args.join(" ").includes("status")
				? { stdout: " M src/x.ts\n", exitCode: 0 }
				: { stdout: "", exitCode: 0 };
		const { deps, store } = makeDeps({
			exec: dirtyGit,
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
			preRun: "mise run setup",
			postRun: "echo done",
			model: "opus",
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
		expect(claudeModel).toBe("opus");
		expect(hookCalls).toEqual(["mise run setup", "echo done"]);
		expect(runStore.readRunMeta(t.id)?.model).toBe("opus");
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
			preRun: "setup.sh {{number}} {{repo_slug}}",
			postRun: null,
			model: "opus",
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
			preRun: "bad-setup",
			postRun: "cleanup",
			model: "opus",
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
			preRun: "run {{ticket}} {{branch}} {{worktree}}",
			postRun: null,
			model: "opus",
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
			preRun: null,
			postRun: "cleanup",
			model: "opus",
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

describe("runTask model-alias resolution", () => {
	it("resolves the alias against modelTable at spawn (snapshot + spawn)", async () => {
		let seenModel = "";
		const { deps, store, runStore } = makeDeps({
			modelTable: { sonnet: "claude-sonnet-4-6" },
			executeClaude: async (opts) => {
				seenModel = opts.model;
				return okResult;
			},
		});
		// default model is "sonnet" (deps.defaults.model)
		const t = enqueue(store);
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		expect(seenModel).toBe("claude-sonnet-4-6");
		expect(runStore.readRunMeta(t.id)?.model).toBe("claude-sonnet-4-6");
	});

	it("passes an unknown/full model id through untouched", async () => {
		let seenModel = "";
		const { deps, store } = makeDeps({
			modelTable: { sonnet: "claude-sonnet-4-6" },
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
			model: "claude-fable-5",
		});
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenModel).toBe("claude-fable-5");
	});
});

describe("runTask main-session pointer", () => {
	const mainStore = () =>
		new MainSessionStore(
			join(mkdtempSync(join(tmpdir(), "qo-main-")), "main-sessions.json"),
		);

	const enqueueMain = (store: QueueStore) =>
		store.create({
			prompt: "do it\n",
			repo: "platform",
			ref: "temp",
			source: "tui",
			session: "main",
		});

	// laneKey for a withWorktree'd platform task is "platform:tmp-x".
	const LANE = "platform:tmp-x";

	it("main task with pointer set → executor receives resumeSessionId = pointer", async () => {
		const mainSessions = mainStore();
		mainSessions.set(LANE, "prev-session");
		let seenResume: string | undefined;
		const { deps, store } = makeDeps({
			mainSessions,
			executeClaude: async (opts) => {
				seenResume = opts.resumeSessionId;
				return okResult;
			},
		});
		const t = enqueueMain(store);
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenResume).toBe("prev-session");
	});

	it("main task without pointer → no resume, and captured sessionId advances the pointer", async () => {
		const mainSessions = mainStore();
		let seenResume: string | undefined = "sentinel";
		const { deps, store } = makeDeps({
			mainSessions,
			executeClaude: async (opts) => {
				seenResume = opts.resumeSessionId;
				return { ...okResult, sessionId: "s1" };
			},
		});
		const t = enqueueMain(store);
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenResume).toBeUndefined();
		expect(mainSessions.get(LANE)).toBe("s1");
	});

	it("fresh task never reads or writes the store", async () => {
		const mainSessions = mainStore();
		mainSessions.set(LANE, "should-stay");
		let seenResume: string | undefined = "sentinel";
		const { deps, store } = makeDeps({
			mainSessions,
			executeClaude: async (opts) => {
				seenResume = opts.resumeSessionId;
				return { ...okResult, sessionId: "s-fresh" };
			},
		});
		// default session is "fresh"
		const t = enqueue(store);
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(seenResume).toBeUndefined();
		expect(mainSessions.get(LANE)).toBe("should-stay");
	});

	it("failed main run with captured sessionId still advances the pointer", async () => {
		const mainSessions = mainStore();
		const { deps, store } = makeDeps({
			mainSessions,
			executeClaude: async () => ({
				...okResult,
				exitCode: 3,
				sessionId: "s-failed",
			}),
		});
		const t = enqueueMain(store);
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("failed");
		expect(mainSessions.get(LANE)).toBe("s-failed");
	});

	it("main run with null sessionId leaves the pointer unchanged", async () => {
		const mainSessions = mainStore();
		mainSessions.set(LANE, "keep-me");
		const { deps, store } = makeDeps({
			mainSessions,
			executeClaude: async () => ({ ...okResult, sessionId: null }),
		});
		const t = enqueueMain(store);
		withWorktree(store, t.id);
		await runTask(t.id, deps);
		expect(mainSessions.get(LANE)).toBe("keep-me");
	});
});

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
});
