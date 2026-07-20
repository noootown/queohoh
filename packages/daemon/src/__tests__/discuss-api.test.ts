import { mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { GlobalConfig, ProviderAdapter, RunResult } from "@queohoh/core";
import {
	BUILTIN_CATALOG,
	DEFAULT_PROVIDERS,
	DiscussStore,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionLineageStore,
	SessionRegistry,
} from "@queohoh/core";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiServer } from "../api.js";
import { ApiClient } from "../client.js";
import { DiscussService } from "../discuss-service.js";
import { Engine } from "../engine.js";
import { SettingsStore } from "../settings-store.js";

const cleanups: (() => Promise<void> | void)[] = [];
afterEach(async () => {
	while (cleanups.length) await cleanups.pop()?.();
});

function makeWorktree(): string {
	const wt = mkdtempSync(join(tmpdir(), "discuss-api-wt-"));
	mkdirSync(wt, { recursive: true });
	return wt;
}

function okRun(overrides: Partial<RunResult> = {}): RunResult {
	return {
		exitCode: 0,
		timedOut: false,
		signal: null,
		sessionId: "prov-sess-1",
		resultText: "ok",
		stderr: "",
		usage: {
			costUsd: 0,
			turns: 1,
			durationMs: 1,
			inputTokens: null,
			outputTokens: null,
		},
		...overrides,
	};
}

async function setup(opts?: {
	/** Slow/hanging executeRun so concurrent turns can race. */
	executeRun?: (
		adapter: ProviderAdapter,
		opts: { onSpawned?: (pid: number) => void },
	) => Promise<RunResult>;
	activeProvider?: string;
	providers?: GlobalConfig["providers"];
}) {
	const base = mkdtempSync(join(tmpdir(), "qo-discuss-api-"));
	const stateDir = join(base, "state");
	const workspace = join(base, "ws");
	const repoPath = join(base, "repo");
	mkdirSync(repoPath, { recursive: true });
	mkdirSync(workspace, { recursive: true });

	const store = new QueueStore(stateDir);
	const runStore = new RunStore(join(base, "runs"));
	const registry = new SessionRegistry(join(base, "sessions.json"));
	const lineage = new SessionLineageStore(join(base, "session-lineage.json"));
	const config: GlobalConfig = {
		workspace,
		projects: [{ name: "platform", path: repoPath }],
		maxConcurrentTasks: 3,
		archiveAfterDays: 7,
		vars: {},
		catalog: BUILTIN_CATALOG,
		defaultModels: ["claude/claude-opus-4.8", "grok/grok-4.5"],
		providers: opts?.providers ?? DEFAULT_PROVIDERS,
	};
	const settings = new SettingsStore(stateDir, config.providers);
	if (opts?.activeProvider) {
		settings.setActiveProvider(opts.activeProvider, config.providers);
	}

	const discussStore = new DiscussStore(join(base, "discuss"));
	const executeRun =
		opts?.executeRun ??
		(async (
			_adapter: ProviderAdapter,
			runOpts: { onSpawned?: (pid: number) => void },
		) => {
			// Synthetic pid so stop can exercise the kill path without a real child.
			runOpts.onSpawned?.(process.pid);
			return okRun();
		});

	const discuss = new DiscussService({
		store: discussStore,
		lineage,
		settings,
		config,
		redact: makeRedactor(new Map()),
		// Injectable spawn seam — tests never hit a real CLI.
		executeRun: executeRun as DiscussService["executeRun"],
	});

	const engine = new Engine({
		store,
		runStore,
		registry,
		config,
		resolverIO: {
			listWorktrees: async () => [],
			prBranch: async () => null,
			spawnWorktree: async (_r, name) => ({
				name,
				path: `/wt/${name}`,
				branch: name,
			}),
			removeWorktree: async () => {},
		},
		exec: async () => ({ stdout: "", exitCode: 0 }),
		executeClaude: async () => okRun(),
		executeVerify: async () => ({
			exitCode: 0,
			timedOut: false,
			signal: null,
			output: "",
		}),
		redact: makeRedactor(new Map()),
		lineage,
	});

	const server = new ApiServer({
		engine,
		store,
		runStore,
		registry,
		config,
		settings,
		lineage,
		discuss,
		onMutation: () => {},
	});
	const sock = join(base, "d.sock");
	await server.listen(sock);
	const client = new ApiClient();
	await client.connect(sock);
	cleanups.push(() => client.close());
	cleanups.push(() => server.close());

	return { client, discuss, discussStore, settings, config };
}

describe("discuss RPCs", () => {
	it("discuss_ensure returns snake_case meta for active provider", async () => {
		const wt = makeWorktree();
		const { client } = await setup({ activeProvider: "grok" });
		const meta = (await client.call("discuss_ensure", { worktree: wt })) as {
			session_id: string;
			provider: string;
			status: string;
			updated_at: string;
			worktree: string;
		};
		expect(meta.session_id).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/i);
		expect(meta.provider).toBe("grok");
		expect(meta.status).toBe("idle");
		expect(typeof meta.updated_at).toBe("string");
		expect(meta.worktree).toBeTruthy();

		// Idempotent for the same worktree + active provider.
		const again = (await client.call("discuss_ensure", { worktree: wt })) as {
			session_id: string;
		};
		expect(again.session_id).toBe(meta.session_id);
	});

	it("discuss_ensure then discuss_turn rejects second concurrent turn", async () => {
		const wt = makeWorktree();
		let release!: () => void;
		const gate = new Promise<void>((r) => {
			release = r;
		});
		const executeRun = vi.fn(async () => {
			await gate;
			return okRun();
		});
		const { client } = await setup({ executeRun, activeProvider: "claude" });

		await client.call("discuss_ensure", { worktree: wt });
		const first = (await client.call("discuss_turn", {
			worktree: wt,
			prompt: "hi",
		})) as { session_id: string; turn_id: string; status: string };
		expect(first.status).toBe("running");
		expect(first.turn_id).toBeTruthy();

		await expect(
			client.call("discuss_turn", { worktree: wt, prompt: "again" }),
		).rejects.toThrow(/busy/i);

		release();
		// Drain the in-flight turn so cleanup is clean.
		await vi.waitFor(async () => {
			const tail = (await client.call("discuss_tail", {
				session_id: first.session_id,
			})) as { status: string };
			expect(tail.status).not.toBe("running");
		});
	});

	it("discuss_tail streams transcript bytes and reflects status", async () => {
		const wt = makeWorktree();
		const executeRun = vi.fn(
			async (
				_a: ProviderAdapter,
				opts: {
					transcriptPath: string;
					onSpawned?: (pid: number) => void;
				},
			) => {
				opts.onSpawned?.(process.pid);
				const { writeFileSync } = await import("node:fs");
				writeFileSync(opts.transcriptPath, "Assistant reply body.");
				return okRun({ sessionId: "prov-1" });
			},
		);
		const { client } = await setup({
			executeRun: executeRun as never,
			activeProvider: "claude",
		});

		const ensured = (await client.call("discuss_ensure", {
			worktree: wt,
		})) as { session_id: string };
		const turn = (await client.call("discuss_turn", {
			worktree: wt,
			prompt: "what is this?",
			anchor: {
				path: "src/foo.ts",
				side: "new",
				line: 10,
				snippet: "const x = 1;",
			},
		})) as { session_id: string; turn_id: string };

		await vi.waitFor(async () => {
			const tail = (await client.call("discuss_tail", {
				session_id: turn.session_id,
				cursor: 0,
			})) as {
				text: string;
				next_cursor: number;
				status: string;
				turn_id: string | null;
				error: string | null;
			};
			expect(tail.status).toBe("idle");
			expect(tail.text).toContain("what is this?");
			expect(tail.text).toContain("Assistant reply body.");
			expect(tail.next_cursor).toBeGreaterThan(0);
			expect(tail.error).toBeNull();
		});

		// Cursor advance returns empty suffix when already at EOF.
		const full = (await client.call("discuss_tail", {
			session_id: ensured.session_id,
			cursor: 0,
		})) as { next_cursor: number; text: string };
		const mid = (await client.call("discuss_tail", {
			session_id: ensured.session_id,
			cursor: full.next_cursor,
		})) as { text: string; next_cursor: number };
		expect(mid.text).toBe("");
		expect(mid.next_cursor).toBe(full.next_cursor);
	});

	it("discuss_stop kills an in-flight turn and settles idle", async () => {
		const wt = makeWorktree();
		let release!: () => void;
		const gate = new Promise<void>((r) => {
			release = r;
		});
		const executeRun = vi.fn(async () => {
			await gate;
			// Simulate stop-killed child (non-zero / signal).
			return okRun({ exitCode: 1, signal: "SIGTERM", stderr: "killed" });
		});
		const { client } = await setup({ executeRun, activeProvider: "claude" });

		await client.call("discuss_ensure", { worktree: wt });
		const turn = (await client.call("discuss_turn", {
			worktree: wt,
			prompt: "long",
		})) as { session_id: string };

		const stopped = (await client.call("discuss_stop", {
			session_id: turn.session_id,
		})) as { status: string };
		// Status may still be running until the async turn finally settles, or
		// idle if stop cleared it — either is fine as long as we don't error.
		expect(["running", "idle"]).toContain(stopped.status);

		release();
		await vi.waitFor(async () => {
			const tail = (await client.call("discuss_tail", {
				session_id: turn.session_id,
			})) as { status: string; error: string | null };
			// Cancelled stops settle idle (not sticky error).
			expect(tail.status).toBe("idle");
		});
	});

	it("discuss_reset mints a new session and leaves the old transcript", async () => {
		const wt = makeWorktree();
		const { client, discussStore } = await setup({ activeProvider: "grok" });

		const first = (await client.call("discuss_ensure", { worktree: wt })) as {
			session_id: string;
		};
		discussStore.appendTranscript(first.session_id, "old history\n");

		const reset = (await client.call("discuss_reset", { worktree: wt })) as {
			session_id: string;
			provider: string;
			status: string;
		};
		expect(reset.session_id).not.toBe(first.session_id);
		expect(reset.provider).toBe("grok");
		expect(reset.status).toBe("idle");

		// Old session dir retained with its transcript.
		const old = discussStore.readTranscript(first.session_id, 0);
		expect(old.text).toContain("old history");

		const ensured = (await client.call("discuss_ensure", { worktree: wt })) as {
			session_id: string;
		};
		expect(ensured.session_id).toBe(reset.session_id);
	});

	it("discuss_ensure requires worktree", async () => {
		const { client } = await setup();
		await expect(client.call("discuss_ensure", {})).rejects.toThrow(
			/worktree/i,
		);
	});
});
