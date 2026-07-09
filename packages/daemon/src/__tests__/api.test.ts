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
import { afterEach, describe, expect, it } from "vitest";
import { ApiServer } from "../api.js";
import { ApiClient } from "../client.js";
import { Engine } from "../engine.js";

const cleanups: (() => Promise<void> | void)[] = [];
afterEach(async () => {
	while (cleanups.length) await cleanups.pop()?.();
});

async function setup() {
	const base = mkdtempSync(join(tmpdir(), "qo-api-"));
	const repoPath = join(base, "repo");
	const workspace = join(base, "ws");
	// definition fixture
	const defDir = join(workspace, "platform", "tasks", "greet");
	mkdirSync(defDir, { recursive: true });
	writeFileSync(join(defDir, "config.yaml"), "args: [name]\ndedup: none\n");
	writeFileSync(join(defDir, "prompt.md"), "Say hi to {{name}}.\n");

	const store = new QueueStore(join(base, "state"));
	const runStore = new RunStore(join(base, "runs"));
	const registry = new SessionRegistry(join(base, "sessions.json"));
	const config: GlobalConfig = {
		workspace,
		projects: [{ name: "platform", path: repoPath }],
		maxConcurrentTasks: 3,
		archiveAfterDays: 7,
		vars: {},
	};
	const okResult: RunResult = {
		exitCode: 0,
		timedOut: false,
		sessionId: null,
		resultText: "ok",
		stderr: "",
		usage: { costUsd: 0, turns: 1, durationMs: 1 },
	};
	const exec: Exec = async () => ({ stdout: "", exitCode: 0 });
	const resolverIO: ResolverIO = {
		listWorktrees: async () => [],
		prBranch: async () => null,
		spawnWorktree: async (_r, name) => ({
			name,
			path: `/wt/${name}`,
			branch: name,
		}),
	};
	const mainSessions = new MainSessionStore(join(base, "main-sessions.json"));
	const engine = new Engine({
		store,
		runStore,
		registry,
		config,
		resolverIO,
		exec,
		executeClaude: async () => okResult,
		redact: makeRedactor(new Map()),
		mainSessions,
	});
	let mutations = 0;
	const server = new ApiServer({
		engine,
		store,
		runStore,
		registry,
		config,
		mainSessions,
		onMutation: () => {
			mutations += 1;
		},
	});
	const sock = join(base, "d.sock");
	await server.listen(sock);
	const client = new ApiClient();
	await client.connect(sock);
	cleanups.push(() => client.close());
	cleanups.push(() => server.close());
	return {
		server,
		client,
		store,
		engine,
		mainSessions,
		mutations: () => mutations,
	};
}

describe("ApiServer", () => {
	it("ping/pong", async () => {
		const { client } = await setup();
		expect(await client.call("ping")).toBe("pong");
	});

	it("state snapshot exposes projects and worktrees", async () => {
		const { client, engine } = await setup();
		await engine.tick();
		const state = (await client.call("state")) as {
			projects: { name: string }[];
			worktrees: Record<string, unknown[]>;
			maxConcurrent: number;
		};
		expect(state.projects).toEqual([{ name: "platform" }]);
		expect(state.worktrees).toEqual(engine.worktreesByRepo());
		expect(state.maxConcurrent).toBe(3);
	});

	it("enqueue creates an adhoc task and reports state", async () => {
		const { client } = await setup();
		const task = (await client.call("enqueue", {
			prompt: "fix it",
			repo: "platform",
		})) as { id: string; target: { ref: string } };
		expect(task.target.ref).toBe("temp");
		const state = (await client.call("state")) as { tasks: { id: string }[] };
		expect(state.tasks.map((t) => t.id)).toContain(task.id);
	});

	it("state snapshot exposes mainSessions (empty by default, filled after set)", async () => {
		const { client, mainSessions } = await setup();
		const empty = (await client.call("state")) as {
			mainSessions: Record<string, string>;
		};
		expect(empty.mainSessions).toEqual({});
		mainSessions.set("platform:JUS-1", "sess-1");
		const filled = (await client.call("state")) as {
			mainSessions: Record<string, string>;
		};
		expect(filled.mainSessions).toEqual({ "platform:JUS-1": "sess-1" });
	});

	it("enqueue with worktree sets the target ref", async () => {
		const { client } = await setup();
		const task = (await client.call("enqueue", {
			prompt: "fix it",
			repo: "platform",
			worktree: "wt-a",
		})) as { target: { ref: string } };
		expect(task.target.ref).toBe("worktree:wt-a");
	});

	it("enqueue with session main sets the task session field", async () => {
		const { client } = await setup();
		const task = (await client.call("enqueue", {
			prompt: "fix it",
			repo: "platform",
			session: "main",
		})) as { session: string };
		expect(task.session).toBe("main");
	});

	it("enqueue defaults session to fresh", async () => {
		const { client } = await setup();
		const task = (await client.call("enqueue", {
			prompt: "fix it",
			repo: "platform",
		})) as { session: string };
		expect(task.session).toBe("fresh");
	});

	it("definitions lists per-repo task definitions", async () => {
		const { client } = await setup();
		const defs = (await client.call("definitions")) as {
			repo: string;
			name: string;
			args: string[];
		}[];
		expect(defs).toEqual([
			{ repo: "platform", name: "greet", args: ["name"], hasDiscovery: false },
		]);
	});

	it("runDefinition with args instantiates", async () => {
		const { client } = await setup();
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "greet",
			args: ["world"],
		})) as { prompt: string }[];
		expect(created).toHaveLength(1);
		expect(created[0]?.prompt).toBe("Say hi to world.\n");
	});

	it("runDefinition attributes source: mcp when requested", async () => {
		const { client } = await setup();
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "greet",
			args: ["world"],
			source: "mcp",
		})) as { source: string }[];
		expect(created).toHaveLength(1);
		expect(created[0]?.source).toBe("mcp");
	});

	it("runDefinition defaults source to tui", async () => {
		const { client } = await setup();
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "greet",
			args: ["world"],
		})) as { source: string }[];
		expect(created[0]?.source).toBe("tui");
	});

	it("runDefinition with worktree overrides the task ref", async () => {
		const { client } = await setup();
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "greet",
			args: ["world"],
			worktree: "wt-x",
		})) as { target: { ref: string } }[];
		expect(created).toHaveLength(1);
		expect(created[0]?.target.ref).toBe("worktree:wt-x");
	});

	it("definition returns the full loaded definition", async () => {
		const { client } = await setup();
		const def = (await client.call("definition", {
			repo: "platform",
			name: "greet",
		})) as {
			prompt: string;
			args: string[];
			worktree: string;
			model: string;
		};
		expect(def.prompt).toBe("Say hi to {{name}}.\n");
		expect(def.args).toEqual(["name"]);
		expect(def.worktree).toBe("temp");
		expect(def.model).toBe("sonnet");
	});

	it("definition rejects unknown repo", async () => {
		const { client } = await setup();
		await expect(
			client.call("definition", { repo: "nope", name: "greet" }),
		).rejects.toThrow(/unknown repo: nope/);
	});

	it("definition rejects unknown name in a known repo", async () => {
		const { client } = await setup();
		await expect(
			client.call("definition", { repo: "platform", name: "nope" }),
		).rejects.toThrow();
	});

	it("retry re-queues a failed task; skip archives it", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, { status: "failed", error: "boom" });
		const retried = (await client.call("retry", { id: t.id })) as {
			status: string;
			error: null;
		};
		expect(retried.status).toBe("queued");
		store.update(t.id, { status: "failed", error: "boom again" });
		await client.call("skip", { id: t.id });
		expect(store.list()).toEqual([]);
	});

	it("retry rejects tasks that are not failed/needs-input", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		await expect(client.call("retry", { id: t.id })).rejects.toThrow(
			/cannot retry/,
		);
	});

	it("setWorktree answers needs-input and re-queues", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:gone",
			source: "tui",
		});
		store.update(t.id, { status: "needs-input", error: "not found" });
		const updated = (await client.call("setWorktree", {
			id: t.id,
			worktree: "main",
		})) as { status: string; target: { worktree: string } };
		expect(updated.status).toBe("queued");
		expect(updated.target.worktree).toBe("main");
	});

	it("setWorktree rejects tasks that are not needs-input", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		await expect(
			client.call("setWorktree", { id: t.id, worktree: "main" }),
		).rejects.toThrow(/cannot set worktree/);
	});

	it("unknown method errors", async () => {
		const { client } = await setup();
		await expect(client.call("nope")).rejects.toThrow("unknown method: nope");
	});

	it("subscribe pushes state on broadcast", async () => {
		const { server, client } = await setup();
		const states: unknown[] = [];
		await client.subscribe((s) => states.push(s));
		server.broadcast();
		await new Promise((r) => setTimeout(r, 100));
		expect(states.length).toBeGreaterThanOrEqual(1);
	});

	it("onClose fires when the server destroys the connection", async () => {
		const { server, client } = await setup();
		const closed = new Promise<void>((resolve) => client.onClose(resolve));
		await server.close();
		await closed;
		expect(true).toBe(true);
	});
});
