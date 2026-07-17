import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Exec, GlobalConfig, ResolverIO } from "@queohoh/core";
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

const noopResolverIO: ResolverIO = {
	listWorktrees: async () => [],
	prBranch: async () => null,
	spawnWorktree: async (_r, name) => ({ name, path: "/tmp/wt", branch: name }),
	removeWorktree: async () => {},
};
const noopExec: Exec = async () => ({ stdout: "", exitCode: 0 });

// Build a workspace with `<workspace>/<project>/tasks/<name>/{config.yaml,prompt.md}`
// and return a GlobalConfig pointing at it. The project `path` is a throwaway repo
// dir (cron firing only enqueues; it never resolves a worktree here).
function workspaceWith(cronExpr: string) {
	const workspace = mkdtempSync(join(tmpdir(), "qoo-ws-"));
	const repoPath = mkdtempSync(join(tmpdir(), "qoo-repo-"));
	const taskDir = join(workspace, "demo", "tasks", "ping");
	mkdirSync(taskDir, { recursive: true });
	writeFileSync(
		join(taskDir, "config.yaml"),
		`description: ping\ncron: "${cronExpr}"\nworktree: repo\ndedup: none\nmodel: sonnet\n`,
	);
	writeFileSync(join(taskDir, "prompt.md"), "ping\n");
	const config: GlobalConfig = {
		workspace,
		projects: [{ name: "demo", path: repoPath }],
		maxConcurrentTasks: 3,
		archiveAfterDays: 7,
		vars: {},
		catalog: BUILTIN_CATALOG,
		defaultModels: ["claude/opus", "grok/grok-4.5"],
		providers: DEFAULT_PROVIDERS,
	};
	return { workspace, config };
}

function engineWith(config: GlobalConfig, now: () => number) {
	const stateDir = mkdtempSync(join(tmpdir(), "qoo-state-"));
	const store = new QueueStore(join(stateDir, "queue"));
	const engine = new Engine({
		store,
		runStore: new RunStore(join(stateDir, "runs")),
		registry: new SessionRegistry(join(stateDir, "sessions.json")),
		config,
		resolverIO: noopResolverIO,
		exec: noopExec,
		executeClaude: async () => ({
			exitCode: 0,
			timedOut: false,
			signal: null,
			sessionId: null,
			resultText: "",
			stderr: "",
			usage: { costUsd: null, turns: null, durationMs: null },
		}),
		executeVerify: async () => ({
			exitCode: 0,
			timedOut: false,
			signal: null,
			output: "",
		}),
		redact: makeRedactor(new Map()),
		lineage: new SessionLineageStore(join(stateDir, "lineage.json")),
		now,
	});
	return { engine, store };
}

// A local-time epoch for the given wall-clock.
const at = (y: number, mo: number, d: number, h: number, mi: number) =>
	new Date(y, mo - 1, d, h, mi, 0, 0).getTime();
const settle = () => new Promise((r) => setTimeout(r, 20));

describe("Engine cron firing", () => {
	it("does not fire on first sight (seeds the cursor)", async () => {
		const { config } = workspaceWith("30 15 * * *");
		const { engine, store } = engineWith(config, () => at(2026, 7, 14, 15, 30));
		await engine.tick(); // first sight: seed only
		await settle();
		expect(store.list().filter((t) => t.source === "cron")).toHaveLength(0);
	});

	it("fires once when a slot comes due after seeding", async () => {
		const { config } = workspaceWith("30 15 * * *");
		let clock = at(2026, 7, 14, 15, 29);
		const { engine, store } = engineWith(config, () => clock);
		await engine.tick(); // seed at 15:29
		clock = at(2026, 7, 14, 15, 30); // slot crosses
		await engine.tick(); // schedules the async fire
		await settle();
		const cronTasks = store.list().filter((t) => t.source === "cron");
		expect(cronTasks).toHaveLength(1);
		expect(cronTasks[0]?.definition).toBe("demo/ping");
	});

	it("does not double-fire the same slot on a later tick", async () => {
		const { config } = workspaceWith("30 15 * * *");
		let clock = at(2026, 7, 14, 15, 29);
		const { engine, store } = engineWith(config, () => clock);
		await engine.tick();
		clock = at(2026, 7, 14, 15, 30);
		await engine.tick();
		await settle();
		clock = at(2026, 7, 14, 15, 31); // same 15:30 slot already fired
		await engine.tick();
		await settle();
		expect(store.list().filter((t) => t.source === "cron")).toHaveLength(1);
	});
});
