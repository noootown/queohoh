import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type {
	Exec,
	GlobalConfig,
	ProviderConfig,
	ResolverIO,
} from "@queohoh/core";
import {
	BUILTIN_CATALOG,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionLineageStore,
	SessionRegistry,
} from "@queohoh/core";
import { describe, expect, it, vi } from "vitest";
import { Engine } from "../engine.js";

/** Working provider table for tests that need a multi-provider chain.
 * Production DEFAULT_PROVIDERS ships every provider disabled (opt-in). */
const TEST_PROVIDERS: ProviderConfig[] = [
	{ name: "claude", enabled: true },
	{ name: "grok", enabled: true },
	{ name: "codex", enabled: false },
];

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
		`description: ping\ncron: "${cronExpr}"\nworktree: repo\ndedup: none\nmodel: claude/claude-sonnet-5\n`,
	);
	writeFileSync(join(taskDir, "prompt.md"), "ping\n");
	const config: GlobalConfig = {
		workspace,
		projects: [{ name: "demo", path: repoPath }],
		maxConcurrentTasks: 3,
		purgeAfterDays: 7,
		archiveAfterDays: 7,
		worktreeRetentionDays: null,
		vars: {},
		catalog: BUILTIN_CATALOG,
		defaultModels: ["claude/claude-opus-5", "grok/grok-4.5"],
		providers: TEST_PROVIDERS,
	};
	return { workspace, config };
}

function engineWith(
	config: GlobalConfig,
	now: () => number,
	isCronDisabled?: (key: string) => boolean,
	execOverride?: Exec,
) {
	const stateDir = mkdtempSync(join(tmpdir(), "qoo-state-"));
	const store = new QueueStore(join(stateDir, "queue"));
	const engine = new Engine({
		store,
		runStore: new RunStore(join(stateDir, "runs")),
		registry: new SessionRegistry(join(stateDir, "sessions.json")),
		config,
		resolverIO: noopResolverIO,
		exec: execOverride ?? noopExec,
		isCronDisabled,
		executeClaude: async () => ({
			exitCode: 0,
			timedOut: false,
			signal: null,
			sessionId: null,
			resultText: "",
			stderr: "",
			usage: {
				costUsd: null,
				turns: null,
				durationMs: null,
				inputTokens: null,
				outputTokens: null,
			},
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

	it("does not fire a due slot while the def is cron-disabled (paused)", async () => {
		const { config } = workspaceWith("30 15 * * *");
		let clock = at(2026, 7, 14, 15, 29);
		const { engine, store } = engineWith(
			config,
			() => clock,
			() => true,
		);
		await engine.tick(); // seed at 15:29
		clock = at(2026, 7, 14, 15, 30); // slot crosses — but the def is paused
		await engine.tick();
		await settle();
		expect(store.list().filter((t) => t.source === "cron")).toHaveLength(0);
	});

	it("resuming a paused cron does not replay the slots missed while paused", async () => {
		const { config } = workspaceWith("30 15 * * *");
		let clock = at(2026, 7, 14, 15, 29);
		let paused = true;
		const { engine, store } = engineWith(
			config,
			() => clock,
			() => paused,
		);
		await engine.tick(); // seed at 15:29
		clock = at(2026, 7, 14, 15, 30); // due slot passes while paused → cursor advances
		await engine.tick();
		await settle();
		expect(store.list().filter((t) => t.source === "cron")).toHaveLength(0);
		// Resume AFTER the 15:30 slot already elapsed: no catch-up backlog fires.
		paused = false;
		clock = at(2026, 7, 14, 15, 31);
		await engine.tick();
		await settle();
		expect(store.list().filter((t) => t.source === "cron")).toHaveLength(0);
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

/**
 * A hung `fireCron` must not retire a definition's cron.
 *
 * `evaluateCrons` guards each key with `cronInFlight` and only a SETTLED fire
 * releases it. Discovery shells out to operator code, so a network hang used to
 * leave the key in-flight forever: that definition never fired again, silently,
 * while every other one kept going. pr-hygiene sat dead for 12 hours this way
 * and only a daemon restart brought it back.
 */
describe("Engine cron watchdog", () => {
	/** A workspace whose def runs discovery, so `fireCron` shells out. */
	function workspaceWithDiscovery(cronExpr: string) {
		const workspace = mkdtempSync(join(tmpdir(), "qoo-ws-"));
		const repoPath = mkdtempSync(join(tmpdir(), "qoo-repo-"));
		const taskDir = join(workspace, "demo", "tasks", "ping");
		mkdirSync(taskDir, { recursive: true });
		writeFileSync(
			join(taskDir, "config.yaml"),
			`description: ping\ncron: "${cronExpr}"\nworktree: repo\ndedup: none\nmodel: claude/claude-sonnet-5\ndiscovery:\n  command: echo '[]'\n  item_key: "{{id}}"\n`,
		);
		writeFileSync(join(taskDir, "prompt.md"), "ping\n");
		const config: GlobalConfig = {
			workspace,
			projects: [{ name: "demo", path: repoPath }],
			maxConcurrentTasks: 3,
			purgeAfterDays: 7,
			archiveAfterDays: 7,
			worktreeRetentionDays: null,
			vars: {},
			catalog: BUILTIN_CATALOG,
			defaultModels: ["claude/claude-opus-5", "grok/grok-4.5"],
			providers: TEST_PROVIDERS,
		};
		return { config };
	}

	it("an expired fire must not release its replacement's guard", async () => {
		const { config } = workspaceWithDiscovery("* * * * *");
		let clock = at(2026, 7, 14, 15, 19);
		const finish: Array<() => void> = [];
		const hangingExec: Exec = async (command) => {
			if (command !== "/bin/bash") return { stdout: "", exitCode: 0 };
			return new Promise((resolve) => {
				finish.push(() => resolve({ stdout: "[]", exitCode: 0 }));
			});
		};
		const { engine } = engineWith(config, () => clock, undefined, hangingExec);
		vi.useFakeTimers();
		try {
			await engine.tick(); // seed at 15:19
			clock = at(2026, 7, 14, 15, 20);
			await engine.tick(); // first fire hangs
			expect(finish).toHaveLength(1);
			await vi.advanceTimersByTimeAsync(13 * 60_000);
			clock = at(2026, 7, 14, 15, 33);
			await engine.tick(); // watchdog permits a replacement
			expect(finish).toHaveLength(2);
			finish[0]?.(); // old fire finally settles while replacement is pending
			await vi.advanceTimersByTimeAsync(60_000);
			clock = at(2026, 7, 14, 15, 34);
			await engine.tick();
			expect(finish).toHaveLength(2);
		} finally {
			for (const resolve of finish) resolve();
			await vi.advanceTimersByTimeAsync(0);
			vi.useRealTimers();
		}
	});

	it("releases the in-flight guard so a later slot still fires after a hung discovery", async () => {
		const { config } = workspaceWithDiscovery("*/20 * * * *");
		let clock = at(2026, 7, 14, 15, 19);
		let discoveries = 0;
		// Discovery never resolves — the exact shape of the wedge.
		const hangingExec: Exec = async (command) => {
			if (command === "/bin/bash") {
				discoveries += 1;
				return new Promise(() => {}); // never settles
			}
			return { stdout: "", exitCode: 0 };
		};
		const { engine } = engineWith(config, () => clock, undefined, hangingExec);

		vi.useFakeTimers();
		try {
			await engine.tick(); // seed
			clock = at(2026, 7, 14, 15, 20);
			await engine.tick(); // due → fire starts and hangs
			await vi.advanceTimersByTimeAsync(20);
			expect(discoveries).toBe(1);

			// Next due slot while still in flight: correctly suppressed (no double-fire).
			clock = at(2026, 7, 14, 15, 40);
			await engine.tick();
			await vi.advanceTimersByTimeAsync(20);
			expect(discoveries).toBe(1);

			// Past the watchdog: the guard is released even though the fire is STILL
			// hung, which is the whole point — recovery cannot depend on it settling.
			await vi.advanceTimersByTimeAsync(13 * 60_000);

			clock = at(2026, 7, 14, 16, 0);
			await engine.tick();
			await vi.advanceTimersByTimeAsync(20);
			// The cron is alive again rather than silently retired forever.
			expect(discoveries).toBe(2);
		} finally {
			vi.useRealTimers();
		}
	});
});
