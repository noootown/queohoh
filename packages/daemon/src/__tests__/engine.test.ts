import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Exec, GlobalConfig, ResolverIO, RunResult } from "@queohoh/core";
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
		...overrides.resolverIO,
	};
	const exec: Exec = async () => ({ stdout: "", exitCode: 0 });
	const mainSessions = new MainSessionStore(join(base, "main-sessions.json"));
	const engine = new Engine({
		store,
		runStore,
		registry,
		config,
		resolverIO,
		exec,
		executeClaude: async () => overrides.claudeResult ?? okResult,
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

describe("Engine.worktreesByRepo", () => {
	it("exposes the cached worktrees per repo after a tick", async () => {
		const { engine, base } = setup();
		await engine.tick();
		expect(engine.worktreesByRepo()).toEqual({
			platform: [
				{ name: "JUS-1", path: join(base, "wt-jus1"), branch: "JUS-1" },
			],
		});
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
