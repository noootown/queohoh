import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type {
	ClaudeExecutor,
	Exec,
	GlobalConfig,
	ResolverIO,
	RunResult,
} from "@queohoh/core";
import {
	MainSessionStore,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionRegistry,
} from "@queohoh/core";
import { describe, expect, it } from "vitest";
import { Engine } from "../engine.js";

const okResult: RunResult = {
	exitCode: 0,
	timedOut: false,
	signal: null,
	sessionId: null,
	resultText: "ok",
	stderr: "",
	usage: { costUsd: 0, turns: 1, durationMs: 10 },
};

function setup(
	overrides: {
		resolverIO?: Partial<ResolverIO>;
		config?: Partial<GlobalConfig>;
		claudeResult?: RunResult;
		exec?: Exec;
		executeClaude?: ClaudeExecutor;
	} = {},
) {
	const base = mkdtempSync(join(tmpdir(), "qo-engine-"));
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
		...overrides.resolverIO,
	};
	const exec: Exec =
		overrides.exec ?? (async () => ({ stdout: "", exitCode: 0 }));
	const mainSessions = new MainSessionStore(join(base, "main-sessions.json"));
	const engine = new Engine({
		store,
		runStore,
		registry,
		config,
		resolverIO,
		exec,
		executeClaude:
			overrides.executeClaude ??
			(async () => overrides.claudeResult ?? okResult),
		redact: makeRedactor(new Map()),
		mainSessions,
	});
	return { engine, store, base, mainSessions };
}

describe("Engine.tick", () => {
	it("resolves an unresolved task then runs it to done across ticks", async () => {
		const { engine, store } = setup();
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		await engine.tick(); // resolve pass
		expect(store.list()[0]?.target.worktree).toBe("JUS-1");
		await engine.tick(); // start pass
		await engine.drain();
		expect(store.list()[0]?.status).toBe("done");
		// The terminal transition stamps finishedAt, which the state snapshot
		// then carries verbatim (camelCase) to the TUI.
		expect(store.list()[0]?.finishedAt).toMatch(/^\d{4}-\d{2}-\d{2}T.*Z$/);
	});

	it("advances the main-session pointer after a completed main run", async () => {
		const { engine, store, mainSessions } = setup({
			claudeResult: { ...okResult, sessionId: "sess-abc" },
		});
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
			session: "main",
		});
		await engine.tick(); // resolve
		await engine.tick(); // start
		await engine.drain();
		expect(store.list()[0]?.status).toBe("done");
		expect(mainSessions.get("platform:JUS-1")).toBe("sess-abc");
	});

	it("runs a `repo` ref in the primary checkout via the @repo sentinel", async () => {
		const { engine, store } = setup();
		store.create({ prompt: "p", repo: "platform", ref: "repo", source: "tui" });
		await engine.tick(); // resolve → @repo (no spawn)
		expect(store.list()[0]?.target.worktree).toBe("@repo");
		expect(store.list()[0]?.ephemeralWorktree).toBe(false);
		await engine.tick(); // start
		await engine.drain();
		// No worktree is named "@repo", so reaching "done" proves the engine's
		// name→path lookup special-cased the sentinel to the project's path
		// (otherwise the worker would fail with "worktree path not found").
		expect(store.list()[0]?.status).toBe("done");
	});

	it("routes unknown repo to needs-input", async () => {
		const { engine, store } = setup();
		store.create({ prompt: "p", repo: "ghost", ref: "temp", source: "tui" });
		await engine.tick();
		const t = store.list()[0];
		expect(t?.status).toBe("needs-input");
		expect(t?.error).toContain("unknown repo");
	});

	it("maps resolver needs-input outcome onto the task", async () => {
		const { engine, store } = setup();
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:missing",
			source: "tui",
		});
		await engine.tick();
		expect(store.list()[0]?.status).toBe("needs-input");
	});

	it("names ephemeral temp worktrees from the prompt with a qoo- prefix", async () => {
		const { engine, store } = setup();
		store.create({
			prompt: "fix the login redirect",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		await engine.tick(); // resolve pass
		expect(store.list()[0]?.target.worktree).toMatch(
			/^qoo-fix-the-login-redirect-[0-9a-z]{4}$/,
		);
		expect(store.list()[0]?.ephemeralWorktree).toBe(true);
	});

	it("maps thrown spawn errors to failed", async () => {
		const { engine, store } = setup({
			resolverIO: {
				spawnWorktree: async () => {
					throw new Error("wt exploded");
				},
			},
		});
		store.create({ prompt: "p", repo: "platform", ref: "temp", source: "tui" });
		await engine.tick();
		const t = store.list()[0];
		expect(t?.status).toBe("failed");
		expect(t?.error).toContain("wt exploded");
	});

	it("fails the task (never runs claude) when project vars.yaml is malformed", async () => {
		const base = mkdtempSync(join(tmpdir(), "qo-engine-vars-"));
		const repoPath = join(base, "repo");
		mkdirSync(repoPath, { recursive: true });
		mkdirSync(join(base, "ws", "platform"), { recursive: true });
		writeFileSync(
			join(base, "ws", "platform", "vars.yaml"),
			"nested:\n  a: 1\n",
		);
		const store = new QueueStore(join(base, "state"));
		const runStore = new RunStore(join(base, "runs"));
		const registry = new SessionRegistry(join(base, "sessions.json"));
		const config: GlobalConfig = {
			workspace: join(base, "ws"),
			projects: [{ name: "platform", path: repoPath }],
			maxConcurrentTasks: 3,
			archiveAfterDays: 7,
			vars: {},
		};
		let claudeRan = false;
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
		const engine = new Engine({
			store,
			runStore,
			registry,
			config,
			resolverIO,
			exec: async () => ({ stdout: "", exitCode: 0 }),
			executeClaude: async () => {
				claudeRan = true;
				return okResult;
			},
			redact: makeRedactor(new Map()),
			mainSessions: new MainSessionStore(join(base, "main-sessions.json")),
		});
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		await engine.tick(); // resolve
		await engine.tick(); // start (should fail on vars load)
		await engine.drain();
		const t = store.list()[0];
		expect(t?.status).toBe("failed");
		expect(t?.error).toContain("non-scalar var");
		expect(claudeRan).toBe(false);
	});

	it("marks running tasks with no live worker as orphaned", async () => {
		const { engine, store } = setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, {
			status: "running",
			target: { repo: "platform", ref: "temp", worktree: "x" },
		});
		await engine.tick();
		expect(store.get(t.id)?.status).toBe("failed");
		expect(store.get(t.id)?.error).toBe("orphaned by daemon restart");
	});

	it("archives old done tasks", async () => {
		const { engine, store } = setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, {
			status: "done",
			created: "2020-01-01T00:00:00.000Z",
		});
		await engine.tick();
		expect(store.list()).toEqual([]);
		expect(store.listArchived().map((a) => a.id)).toEqual([t.id]);
	});
});

describe("Engine.createWorktree", () => {
	it("delegates to spawnWorktree with the resolved repo path", async () => {
		const spawned: { repoPath: string; name: string }[] = [];
		const { engine, base } = setup({
			resolverIO: {
				listWorktrees: async () => [],
				spawnWorktree: async (repoPath, name) => {
					spawned.push({ repoPath, name });
					return { name, path: join(base, `wt-${name}`), branch: name };
				},
			},
		});
		await engine.createWorktree("platform", "feature-x");
		expect(spawned).toEqual([
			{ repoPath: join(base, "repo"), name: "feature-x" },
		]);
	});

	it("rejects an unknown repo", async () => {
		const { engine } = setup();
		await expect(engine.createWorktree("ghost", "feature-x")).rejects.toThrow(
			/unknown repo/,
		);
	});

	it("rejects when a worktree with that branch already exists", async () => {
		const { engine } = setup({
			resolverIO: {
				listWorktrees: async () => [
					{ name: "platform.feature-x", path: "/wt/x", branch: "feature-x" },
				],
			},
		});
		await expect(
			engine.createWorktree("platform", "feature-x"),
		).rejects.toThrow(/already exists/);
	});
});

describe("Engine.worktreesByRepo", () => {
	it("exposes the cached worktrees per repo, merged with git enrichment, after a tick", async () => {
		const { engine, base } = setup();
		await engine.tick();
		await engine.refreshGitEnrichment();
		// With the default all-zero-exit / empty-stdout exec stub: status "" → not
		// dirty; log "" → epoch parseInt(NaN) → null and empty author → null.
		expect(engine.worktreesByRepo()).toEqual({
			platform: [
				{
					name: "JUS-1",
					path: join(base, "wt-jus1"),
					branch: "JUS-1",
					dirty: false,
					lastCommitEpoch: null,
					lastCommitAuthor: null,
				},
			],
		});
	});
});

describe("Engine git enrichment", () => {
	// Route git subcommands by their args[2] (the subcommand after `-C <path>`).
	function gitExec(
		handlers: Partial<
			Record<string, () => { stdout: string; exitCode: number }>
		>,
		onCall?: (sub: string) => void,
	): Exec {
		return async (_command, args) => {
			// args = ["-C", path, <subcommand>, ...rest]
			const sub = args[2] ?? "";
			onCall?.(sub);
			const h = handlers[sub];
			return h ? h() : { stdout: "", exitCode: 0 };
		};
	}

	it("populates dirty, lastCommitEpoch and lastCommitAuthor from git output", async () => {
		const exec = gitExec({
			status: () => ({ stdout: " M src/a.ts\n", exitCode: 0 }),
			// one call: "<epoch>\t<author>"
			log: () => ({ stdout: "1700000000\tKevin O'Shea\n", exitCode: 0 }),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		const wt = engine.worktreesByRepo().platform?.[0];
		expect(wt).toMatchObject({
			dirty: true,
			lastCommitEpoch: 1700000000,
			lastCommitAuthor: "Kevin O'Shea",
		});
	});

	it("yields null for a field whose git subcommand fails, still computing the rest", async () => {
		const exec = gitExec({
			status: () => ({ stdout: "", exitCode: 128 }), // fails → dirty null
			log: () => ({ stdout: "1700000000\tAda Lovelace\n", exitCode: 0 }),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		const wt = engine.worktreesByRepo().platform?.[0];
		expect(wt).toMatchObject({
			dirty: null,
			lastCommitEpoch: 1700000000,
			lastCommitAuthor: "Ada Lovelace",
		});
	});

	it("nulls the author when the log line carries an epoch but no author", async () => {
		const exec = gitExec({
			status: () => ({ stdout: "", exitCode: 0 }),
			log: () => ({ stdout: "1700000000\t\n", exitCode: 0 }),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		const wt = engine.worktreesByRepo().platform?.[0];
		expect(wt).toMatchObject({
			lastCommitEpoch: 1700000000,
			lastCommitAuthor: null,
		});
	});

	it("serves last-known within the TTL without re-shelling git", async () => {
		const counts: Record<string, number> = {};
		const exec = gitExec(
			{
				status: () => ({ stdout: " M x\n", exitCode: 0 }),
				log: () => ({ stdout: "1\tHopper\n", exitCode: 0 }),
			},
			(sub) => {
				counts[sub] = (counts[sub] ?? 0) + 1;
			},
		);
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		const statusAfterFirst = counts.status;
		const logAfterFirst = counts.log;
		await engine.refreshGitEnrichment(); // within TTL → no re-shell for same path
		expect(counts.status).toBe(statusAfterFirst);
		expect(counts.log).toBe(logAfterFirst);
	});
});

describe("Engine.laneOfCwd", () => {
	it("prefix-matches worktree paths after a tick", async () => {
		const { engine, base } = setup();
		await engine.tick();
		expect(engine.laneOfCwd(join(base, "wt-jus1", "src"))).toBe(
			"platform:JUS-1",
		);
		expect(engine.laneOfCwd("/elsewhere")).toBeNull();
	});
});

describe("Engine.stopTask", () => {
	it("throws when no child is tracked for the id", () => {
		const { engine } = setup();
		expect(() => engine.stopTask("nope")).toThrow(/no running child tracked/);
	});

	it("kills a running task's child, failing it with the signal reason", async () => {
		const { spawn } = await import("node:child_process");
		let markSpawned: () => void = () => {};
		const spawned = new Promise<void>((r) => {
			markSpawned = r;
		});
		// A real detached child (own process group) so stopTask's group-kill lands.
		const executeClaude: ClaudeExecutor = (opts) =>
			new Promise((resolve) => {
				const child = spawn("sleep", ["30"], {
					detached: true,
					stdio: "ignore",
				});
				if (child.pid) opts.onSpawned?.(child.pid);
				markSpawned();
				child.on("close", (code, signal) => {
					resolve({ ...okResult, exitCode: code ?? 1, signal: signal ?? null });
				});
			});
		const { engine, store } = setup({ executeClaude });
		const task = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		await engine.tick(); // resolve
		await engine.tick(); // start → spawns the child
		await spawned; // the pid is now tracked
		engine.stopTask(task.id);
		await engine.drain();
		const t = store.list()[0];
		expect(t?.status).toBe("failed");
		expect(t?.error).toBe("stopped (SIGTERM)");
	}, 15_000);
});
