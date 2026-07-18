import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type {
	Exec,
	GlobalConfig,
	ResolverIO,
	RunResult,
	SpawnSpec,
} from "@queohoh/core";
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
import type { ShimSpawner } from "../shim-host.js";

const okResult: RunResult = {
	exitCode: 0,
	timedOut: false,
	signal: null,
	sessionId: "s",
	resultText: "ok",
	stderr: "",
	usage: { costUsd: 0, turns: 1, durationMs: 1, inputTokens: null, outputTokens: null },
};

describe("Engine default_models []→global fallback", () => {
	it("treats an explicitly-empty project default_models as unset and falls back to the global list", async () => {
		const base = mkdtempSync(join(tmpdir(), "qo-engine-dm-"));
		const repoPath = join(base, "repo");
		mkdirSync(repoPath, { recursive: true });
		const workspace = join(base, "ws");
		// Project vars.yaml with an EXPLICITLY EMPTY default_models list —
		// loadProjectDefaultModels returns `[]`, and the engine owns the []→global
		// fallback decision (an empty override reads as "unset").
		const projDir = join(workspace, "platform");
		mkdirSync(projDir, { recursive: true });
		writeFileSync(join(projDir, "vars.yaml"), "default_models: []\n");

		const store = new QueueStore(join(base, "state"));
		const runStore = new RunStore(join(base, "runs"));
		const registry = new SessionRegistry(join(base, "sessions.json"));
		const config: GlobalConfig = {
			workspace,
			projects: [{ name: "platform", path: repoPath }],
			maxConcurrentTasks: 3,
			archiveAfterDays: 7,
			vars: {},
			catalog: BUILTIN_CATALOG,
			// Global fallback names ONLY grok. Because the injected activeProvider
			// (codex) is DISABLED, the "head the chain onto the active provider"
			// branch does NOT fire, so the resolved chain comes purely from
			// default_models: fall back to the global list → grok runs; use the
			// project's literal empty list → an empty chain → the task fails.
			defaultModels: ["grok/grok-4.5"],
			providers: DEFAULT_PROVIDERS,
		};

		const specs: SpawnSpec[] = [];
		const spawnShim: ShimSpawner = async (_id, spec, onPid) => {
			specs.push(spec);
			onPid(1);
			return okResult;
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
		};
		const exec: Exec = async () => ({ stdout: "", exitCode: 0 });
		const lineage = new SessionLineageStore(join(base, "session-lineage.json"));
		const engine = new Engine({
			store,
			runStore,
			registry,
			config,
			resolverIO,
			exec,
			executeClaude: async () => okResult,
			executeVerify: async () => ({
				exitCode: 0,
				timedOut: false,
				signal: null,
				output: "",
			}),
			redact: makeRedactor(new Map()),
			lineage,
			spawnShim,
			// Disabled provider → no active-provider heading, so default_models
			// alone drives the chain (isolates the []→global decision).
			activeProvider: () => "codex",
		});

		// A model-less task (no `model:` of its own) resolves against default_models.
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});

		await engine.tick(); // resolve pass: stamps target.worktree
		await engine.tick(); // start pass: resolve chain → grok
		await engine.drain();

		const task = store.list()[0];
		expect(task?.status).toBe("done");
		expect(specs).toHaveLength(1);
		expect(specs[0]?.provider).toBe("grok");
	});
});
