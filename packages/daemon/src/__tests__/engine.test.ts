import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type {
	ClaudeExecutor,
	Exec,
	GlobalConfig,
	ResolverIO,
	RunResult,
	VerifyExecutor,
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
import { adoptionDecision, Engine } from "../engine.js";

const okResult: RunResult = {
	exitCode: 0,
	timedOut: false,
	signal: null,
	sessionId: null,
	resultText: "ok",
	stderr: "",
	usage: { costUsd: 0, turns: 1, durationMs: 10, inputTokens: null, outputTokens: null },
};

function setup(
	overrides: {
		resolverIO?: Partial<ResolverIO>;
		config?: Partial<GlobalConfig>;
		claudeResult?: RunResult;
		exec?: Exec;
		executeClaude?: ClaudeExecutor;
		executeVerify?: VerifyExecutor;
		pidAlive?: (pid: number) => boolean;
		isShimPid?: (pid: number) => boolean;
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
		catalog: BUILTIN_CATALOG,
		defaultModels: ["claude/opus", "grok/grok-4.5"],
		providers: DEFAULT_PROVIDERS,
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
	const lineage = new SessionLineageStore(join(base, "session-lineage.json"));
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
		executeVerify:
			overrides.executeVerify ??
			(async () => ({
				exitCode: 0,
				timedOut: false,
				signal: null,
				output: "",
			})),
		redact: makeRedactor(new Map()),
		lineage,
		pidAlive: overrides.pidAlive,
		isShimPid: overrides.isShimPid,
	});
	return { engine, store, base, lineage };
}

/** `setup` with adoption-sweep probes injected, so a test can force the
 * finalize/adopt/orphan branch without a real process. */
function setupWith(overrides: {
	pidAlive?: (pid: number) => boolean;
	isShimPid?: (pid: number) => boolean;
}) {
	return setup(overrides);
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

	it("records a session fork into lineage after a completed pinned run", async () => {
		const { engine, store, lineage } = setup({
			claudeResult: { ...okResult, sessionId: "sess-abc" },
		});
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
			resumeSessionId: "pin-1",
		});
		await engine.tick(); // resolve
		await engine.tick(); // start
		await engine.drain();
		expect(store.list()[0]?.status).toBe("done");
		// The worker resumed the pin's tip and recorded pin-1 → the run's emitted
		// session id, so following the lineage now lands on the new session.
		expect(lineage.tip("pin-1")).toBe("sess-abc");
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

	it("names a def task's temp worktree from its itemKey, not the rendered template", async () => {
		// A definition task's `prompt` is the rendered TEMPLATE — its opening
		// words are identical for every run ("You are in a git worktree of…"),
		// so slugging the prompt names every autofix worktree/branch
		// `qoo-you-are-in-a-git-*`. The itemKey (the rendered args) is the
		// run-specific content and must win when present.
		const { engine, store } = setup();
		store.create({
			prompt: "You are in a git worktree of the repo. Fix the situation below.",
			repo: "platform",
			ref: "temp",
			source: "mcp",
			definition: "platform/autofix",
			item: { situation: "tabbar crash on empty project" },
			itemKey: "tabbar crash on empty project",
		});
		await engine.tick(); // resolve pass
		expect(store.list()[0]?.target.worktree).toMatch(
			/^qoo-tabbar-crash-on-empty-[0-9a-z]{4}$/,
		);
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
			catalog: BUILTIN_CATALOG,
			defaultModels: ["claude/opus", "grok/grok-4.5"],
			providers: DEFAULT_PROVIDERS,
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
			executeVerify: async () => ({
				exitCode: 0,
				timedOut: false,
				signal: null,
				output: "",
			}),
			redact: makeRedactor(new Map()),
			lineage: new SessionLineageStore(join(base, "session-lineage.json")),
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

	it("resolves a task's provider/label model ref to the catalog id in run meta", async () => {
		const { engine, store, base } = setup();
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
			// A `provider/label` ref resolves against the catalog to its concrete
			// provider-specific id (there is no alias table anymore).
			model: "claude/sonnet",
		});
		await engine.tick(); // resolve
		await engine.tick(); // start
		await engine.drain();
		const id = store.list()[0]?.id ?? "";
		expect(store.list()[0]?.status).toBe("done");
		expect(new RunStore(join(base, "runs")).readRunMeta(id)?.model).toBe(
			"claude-sonnet-5",
		);
	});

	it("marks a running task with no result and no live shim as worker died", async () => {
		const { engine, store } = setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, {
			status: "running",
			target: { repo: "platform", ref: "temp", worktree: "JUS-1" },
		});
		await engine.tick();
		expect(store.get(t.id)?.status).toBe("failed");
		expect(store.get(t.id)?.error).toBe("worker died");
	});

	it("finalizes an adopted task whose result.json is already present", async () => {
		const { engine, store, base } = setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, {
			status: "running",
			target: { repo: "platform", ref: "temp", worktree: "JUS-1" },
		});
		const rs = new RunStore(join(base, "runs"));
		rs.writeResultJson(t.id, { ...okResult, resultText: "done" });
		await engine.tick();
		await engine.drain();
		expect(store.get(t.id)?.status).toBe("done");
	});

	it("adopts a live shim (result absent, pid alive & argv is shim) and leaves it running", async () => {
		const { engine, store, base } = setupWith({
			pidAlive: () => true,
			isShimPid: () => true,
		});
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, {
			status: "running",
			target: { repo: "platform", ref: "temp", worktree: "JUS-1" },
		});
		new RunStore(join(base, "runs")).writeWorkerPid(t.id, 999999);
		await engine.tick();
		expect(store.get(t.id)?.status).toBe("running"); // still adopted, not settled
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

	it("applies per-project task_retention_days over the archive_after_days default", async () => {
		const base = mkdtempSync(join(tmpdir(), "qo-engine-trd-"));
		const repoPath = join(base, "repo");
		mkdirSync(repoPath, { recursive: true });
		// platform keeps tasks 15 days; knowledgebase only 3; other has no
		// vars.yaml so it falls back to the archive_after_days default (7).
		for (const [name, days] of [
			["platform", 15],
			["knowledgebase", 3],
		] as const) {
			const wsProject = join(base, "ws", name);
			mkdirSync(wsProject, { recursive: true });
			writeFileSync(
				join(wsProject, "vars.yaml"),
				`task_retention_days: ${days}\n`,
			);
		}
		const store = new QueueStore(join(base, "state"));
		const runStore = new RunStore(join(base, "runs"));
		const registry = new SessionRegistry(join(base, "sessions.json"));
		const config: GlobalConfig = {
			workspace: join(base, "ws"),
			projects: [
				{ name: "platform", path: repoPath },
				{ name: "knowledgebase", path: repoPath },
				{ name: "other", path: repoPath },
			],
			maxConcurrentTasks: 3,
			archiveAfterDays: 7,
			vars: {},
			catalog: BUILTIN_CATALOG,
			defaultModels: ["claude/opus", "grok/grok-4.5"],
			providers: DEFAULT_PROVIDERS,
		};
		const lineage = new SessionLineageStore(join(base, "session-lineage.json"));
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
					path: join(base, `wt-${name}`),
					branch: name,
				}),
				removeWorktree: async () => {},
			},
			exec: async () => ({ stdout: "", exitCode: 0 }),
			executeClaude: async () => okResult,
			executeVerify: async () => ({
				exitCode: 0,
				timedOut: false,
				signal: null,
				output: "",
			}),
			redact: makeRedactor(new Map()),
			lineage,
		});

		const day = 86_400_000;
		const ageDays = (n: number) => new Date(Date.now() - n * day).toISOString();
		// One done task per project at 10 days and one at 2 days.
		const made: { id: string; repo: string; age: number }[] = [];
		for (const repo of ["platform", "knowledgebase", "other"]) {
			for (const age of [10, 2]) {
				const t = store.create({
					prompt: "p",
					repo,
					ref: "temp",
					source: "tui",
				});
				store.update(t.id, { status: "done", created: ageDays(age) });
				made.push({ id: t.id, repo, age });
			}
		}

		await engine.tick();

		const survived = new Set(store.list().map((t) => t.id));
		// At 10 days: platform (15) survives; knowledgebase (3) and other (7) pruned.
		// At 2 days: everyone survives.
		for (const { id, repo, age } of made) {
			const shouldSurvive = age === 2 || (repo === "platform" && age === 10); // 10 < 15 only for platform
			expect(
				survived.has(id),
				`${repo} @ ${age}d should ${shouldSurvive ? "survive" : "be archived"}`,
			).toBe(shouldSurvive);
		}
	});
});

describe("adoptionDecision", () => {
	it("result present → finalize regardless of pid", () => {
		expect(adoptionDecision(true, false, false)).toBe("finalize");
		expect(adoptionDecision(true, true, true)).toBe("finalize");
	});
	it("no result, live shim → adopt", () => {
		expect(adoptionDecision(false, true, true)).toBe("adopt");
	});
	it("no result, live pid but not a shim (reuse) → orphan", () => {
		expect(adoptionDecision(false, true, false)).toBe("orphan");
	});
	it("no result, dead pid → orphan", () => {
		expect(adoptionDecision(false, false, false)).toBe("orphan");
	});
});

describe("Engine.removeWorktree protection", () => {
	function protSetup() {
		const base = mkdtempSync(join(tmpdir(), "qo-eng-rm-prot-"));
		const repoPath = join(base, "repo");
		mkdirSync(repoPath, { recursive: true });
		const wsProject = join(base, "ws", "platform");
		mkdirSync(wsProject, { recursive: true });
		writeFileSync(
			join(wsProject, "vars.yaml"),
			"protected_worktrees:\n  - legal-lake\n  - testing1\n",
		);
		let removed: string | null = null;
		const { engine } = setup({
			config: {
				workspace: join(base, "ws"),
				projects: [{ name: "platform", path: repoPath }],
			},
			resolverIO: {
				listWorktrees: async () => [
					{ name: "platform", path: repoPath, branch: "main" },
					{
						name: "legal-lake",
						path: join(base, "wt-ll"),
						branch: "legal-lake",
					},
					// Real-world shape: the worktree directory (and thus its name)
					// carries the `<repo>.` prefix while vars.yaml lists the
					// stripped display name.
					{
						name: "platform.testing1",
						path: join(base, "wt-t1"),
						branch: "testing1",
					},
					{ name: "JUS-1", path: join(base, "wt-jus1"), branch: "JUS-1" },
				],
				removeWorktree: async (_r, wt) => {
					removed = wt.name;
				},
			},
		});
		return { engine, removed: () => removed };
	}

	it("refuses to remove the main checkout", async () => {
		const { engine, removed } = protSetup();
		await expect(engine.removeWorktree("platform", "platform")).rejects.toThrow(
			/protected/,
		);
		expect(removed()).toBeNull();
	});

	it("refuses to remove a configured protected worktree", async () => {
		const { engine, removed } = protSetup();
		await expect(
			engine.removeWorktree("platform", "legal-lake"),
		).rejects.toThrow(/protected/);
		expect(removed()).toBeNull();
	});

	it("refuses to remove a protected worktree listed by its display name", async () => {
		// vars.yaml says `testing1`; the worktree is `platform.testing1`.
		const { engine, removed } = protSetup();
		await expect(engine.removeWorktree("platform", "testing1")).rejects.toThrow(
			/protected/,
		);
		expect(removed()).toBeNull();
	});

	it("still removes an unprotected worktree", async () => {
		const { engine, removed } = protSetup();
		await engine.removeWorktree("platform", "JUS-1");
		expect(removed()).toBe("JUS-1");
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
		// dirty; the `merge-base --is-ancestor` probe (branch "JUS-1" ≠ default
		// "main") exits 0 → merged true; log "" → epoch parseInt(NaN) → null, empty
		// author/hash → null; and `gh pr list` "" → JSON.parse throws → prNumber/prUrl
		// null (failure-tolerant).
		expect(engine.worktreesByRepo()).toEqual({
			platform: [
				{
					name: "JUS-1",
					path: join(base, "wt-jus1"),
					branch: "JUS-1",
					dirty: false,
					merged: true,
					lastCommitEpoch: null,
					lastCommitAuthor: null,
					lastCommitAuthorEmail: null,
					lastCommitHash: null,
					prNumber: null,
					prUrl: null,
					prAuthor: null,
					prState: null,
					approved: null,
					protected: false,
				},
			],
		});
	});

	it("marks the main checkout and configured names as protected", async () => {
		const base = mkdtempSync(join(tmpdir(), "qo-eng-prot-"));
		const repoPath = join(base, "repo");
		mkdirSync(repoPath, { recursive: true });
		const wsProject = join(base, "ws", "platform");
		mkdirSync(wsProject, { recursive: true });
		writeFileSync(
			join(wsProject, "vars.yaml"),
			"protected_worktrees:\n  - legal-lake\n  - testing1\n",
		);
		const { engine } = setup({
			config: {
				workspace: join(base, "ws"),
				projects: [{ name: "platform", path: repoPath }],
			},
			resolverIO: {
				listWorktrees: async () => [
					{ name: "platform", path: repoPath, branch: "main" },
					{
						name: "legal-lake",
						path: join(base, "wt-ll"),
						branch: "legal-lake",
					},
					// Real-world shape: worktree name carries the `<repo>.` prefix
					// while vars.yaml lists the stripped display name.
					{
						name: "platform.testing1",
						path: join(base, "wt-t1"),
						branch: "testing1",
					},
					{ name: "JUS-1", path: join(base, "wt-jus1"), branch: "JUS-1" },
				],
			},
		});
		await engine.tick();
		const list = engine.worktreesByRepo().platform ?? [];
		const byName = Object.fromEntries(list.map((w) => [w.name, w.protected]));
		expect(byName).toEqual({
			platform: true,
			"legal-lake": true,
			"platform.testing1": true,
			"JUS-1": false,
		});
	});
});

describe("Engine.refreshWorktreeCache failure handling", () => {
	it("keeps the last-known worktrees when a refresh transiently fails", async () => {
		// index.lock contention (e.g. a second daemon or a concurrent git op) must
		// not blank the pane for a tick — the old `catch → set []` clobber did.
		let calls = 0;
		const { engine, base } = setup({
			resolverIO: {
				listWorktrees: async () => {
					calls++;
					if (calls > 1) throw new Error("index.lock contention");
					return [
						{ name: "JUS-1", path: join(base, "wt-jus1"), branch: "JUS-1" },
					];
				},
			},
		});
		await engine.tick(); // first refresh succeeds and fills the cache
		await engine.tick(); // second refresh throws — must keep the last-known list
		expect(calls).toBeGreaterThan(1);
		expect(engine.worktreesByRepo().platform).toHaveLength(1);
	});

	it("records an empty list for a repo that has never listed successfully", async () => {
		const { engine } = setup({
			resolverIO: {
				listWorktrees: async () => {
					throw new Error("boom");
				},
			},
		});
		await engine.tick();
		expect(engine.worktreesByRepo().platform).toEqual([]);
	});
});

describe("Engine git enrichment", () => {
	// Route git subcommands by their args[2] (the subcommand after `-C <path>`),
	// and `gh` calls by the "gh" key. A handler may throw to simulate a missing
	// binary (spawn rejection); the throw propagates as a rejected exec promise.
	function gitExec(
		handlers: Partial<
			Record<string, () => { stdout: string; exitCode: number }>
		>,
		onCall?: (sub: string) => void,
	): Exec {
		return async (command, args) => {
			// git: args = ["-C", path, <subcommand>, ...rest]. gh is routed by its
			// `--state <state>` flag so the OPEN and MERGED `gh pr list` calls can be
			// stubbed independently ("gh:open" / "gh:merged"); a plain "gh" handler
			// still catches both (the existing single-call tests keep working).
			let key: string;
			if (command === "gh") {
				const stateIdx = args.indexOf("--state");
				const state = stateIdx >= 0 ? (args[stateIdx + 1] ?? "") : "";
				key = handlers[`gh:${state}`] ? `gh:${state}` : "gh";
			} else {
				key = args[2] ?? "";
			}
			onCall?.(key);
			const h = handlers[key];
			return h ? h() : { stdout: "", exitCode: 0 };
		};
	}

	it("populates dirty, epoch, author, email and hash from the 4-field git output", async () => {
		const exec = gitExec({
			status: () => ({ stdout: " M src/a.ts\n", exitCode: 0 }),
			// one call: "<epoch>\t<author>\t<email>\t<hash>"
			log: () => ({
				stdout:
					"1700000000\tKevin O'Shea\t12345+koshea@users.noreply.github.com\t9f3ac1d\n",
				exitCode: 0,
			}),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		const wt = engine.worktreesByRepo().platform?.[0];
		expect(wt).toMatchObject({
			dirty: true,
			lastCommitEpoch: 1700000000,
			lastCommitAuthor: "Kevin O'Shea",
			lastCommitAuthorEmail: "12345+koshea@users.noreply.github.com",
			lastCommitHash: "9f3ac1d",
		});
	});

	it("nulls the hash (and email) when git returns the old 3-field line", async () => {
		const exec = gitExec({
			status: () => ({ stdout: "", exitCode: 0 }),
			// Back-compat: the pre-%h "<epoch>\t<author>\t<email>" line still parses;
			// the absent trailing hash field yields null, others intact.
			log: () => ({
				stdout: "1700000000\tAda Lovelace\tada@example.com\n",
				exitCode: 0,
			}),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		const wt = engine.worktreesByRepo().platform?.[0];
		expect(wt).toMatchObject({
			lastCommitEpoch: 1700000000,
			lastCommitAuthor: "Ada Lovelace",
			lastCommitAuthorEmail: "ada@example.com",
			lastCommitHash: null,
		});
	});

	it("nulls the email when the log line predates the %ae field (2-field output)", async () => {
		const exec = gitExec({
			status: () => ({ stdout: "", exitCode: 0 }),
			// Back-compat: a 2-field "<epoch>\t<author>" line still parses; email null.
			log: () => ({ stdout: "1700000000\tAda Lovelace\n", exitCode: 0 }),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		const wt = engine.worktreesByRepo().platform?.[0];
		expect(wt).toMatchObject({
			lastCommitEpoch: 1700000000,
			lastCommitAuthor: "Ada Lovelace",
			lastCommitAuthorEmail: null,
		});
	});

	it("nulls the email when the log line carries epoch and author but an empty email", async () => {
		const exec = gitExec({
			status: () => ({ stdout: "", exitCode: 0 }),
			log: () => ({ stdout: "1700000000\tGrace Hopper\t\n", exitCode: 0 }),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		const wt = engine.worktreesByRepo().platform?.[0];
		expect(wt).toMatchObject({
			lastCommitEpoch: 1700000000,
			lastCommitAuthor: "Grace Hopper",
			lastCommitAuthorEmail: null,
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

	it("stamps prNumber and prUrl when an open PR matches the worktree branch", async () => {
		const exec = gitExec({
			log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }),
			gh: () => ({
				stdout: JSON.stringify([
					{
						number: 99,
						headRefName: "other",
						url: "https://github.com/o/r/pull/99",
					},
					{
						number: 42,
						headRefName: "JUS-1",
						url: "https://github.com/o/r/pull/42",
					},
				]),
				exitCode: 0,
			}),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]).toMatchObject({
			prNumber: 42,
			prUrl: "https://github.com/o/r/pull/42",
		});
	});

	it("stamps prUrl null when gh omits the url field but sends the number", async () => {
		// Defensive: gh always sends `url`, but a forward-compat / malformed row
		// with a non-string url keeps its number and stamps prUrl null.
		const exec = gitExec({
			log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }),
			gh: () => ({
				stdout: JSON.stringify([{ number: 42, headRefName: "JUS-1" }]),
				exitCode: 0,
			}),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]).toMatchObject({
			prNumber: 42,
			prUrl: null,
		});
	});

	it("leaves prNumber null when no open PR matches the branch", async () => {
		const exec = gitExec({
			log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }),
			gh: () => ({
				stdout: JSON.stringify([{ number: 99, headRefName: "some-other" }]),
				exitCode: 0,
			}),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]).toMatchObject({
			prNumber: null,
			prUrl: null,
		});
	});

	it("folds merged=true and stamps prAuthor/prState from a squash-merged PR the open list omits", async () => {
		// The core bug: a squash-merged branch reads NOT merged by local ancestry
		// (merge-base exit 1), and its local HEAD author is an automation merge
		// commit — but the merged-PR list still carries the true state + author.
		const exec = gitExec({
			log: () => ({ stdout: "1\tIan Chiu\ti@x\tabc123\n", exitCode: 0 }),
			"merge-base": () => ({ stdout: "", exitCode: 1 }), // local: NOT an ancestor
			"gh:open": () => ({ stdout: "[]", exitCode: 0 }),
			"gh:merged": () => ({
				stdout: JSON.stringify([
					{
						number: 55,
						headRefName: "JUS-1",
						url: "https://github.com/o/r/pull/55",
						state: "MERGED",
						author: { name: "Tim Kuminecz", login: "tkuminecz" },
					},
				]),
				exitCode: 0,
			}),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]).toMatchObject({
			// Local ancestry said false, but the PR state supplements it → true.
			merged: true,
			prNumber: 55,
			prUrl: "https://github.com/o/r/pull/55",
			prState: "MERGED",
			// prAuthor is the PR author (Tim), NOT the local merge-commit author (Ian).
			prAuthor: "Tim Kuminecz",
			// The row carries no reviewDecision → a PR exists but isn't approved.
			approved: false,
		});
	});

	it("prefers the OPEN PR on a branch-name collision (open wins over merged)", async () => {
		// A reused branch name can appear in BOTH lists; the currently-open PR is
		// the live one, so it wins the number/url/author/state.
		const exec = gitExec({
			log: () => ({ stdout: "1\tIan Chiu\ti@x\tabc123\n", exitCode: 0 }),
			"merge-base": () => ({ stdout: "", exitCode: 1 }),
			"gh:open": () => ({
				stdout: JSON.stringify([
					{
						number: 10,
						headRefName: "JUS-1",
						url: "https://github.com/o/r/pull/10",
						state: "OPEN",
						author: { name: "Ian Chiu", login: "noootown" },
					},
				]),
				exitCode: 0,
			}),
			"gh:merged": () => ({
				stdout: JSON.stringify([
					{
						number: 9,
						headRefName: "JUS-1",
						url: "https://github.com/o/r/pull/9",
						state: "MERGED",
						author: { name: "Tim Kuminecz", login: "tkuminecz" },
					},
				]),
				exitCode: 0,
			}),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]).toMatchObject({
			prNumber: 10,
			prUrl: "https://github.com/o/r/pull/10",
			prState: "OPEN",
			prAuthor: "Ian Chiu",
			// Open PR + local-not-ancestor → not merged (the OPEN state does not fold true).
			merged: false,
		});
	});

	it("falls back to the PR author login when the author name is empty", async () => {
		const exec = gitExec({
			log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }),
			"merge-base": () => ({ stdout: "", exitCode: 1 }),
			"gh:merged": () => ({
				stdout: JSON.stringify([
					{
						number: 7,
						headRefName: "JUS-1",
						url: "https://github.com/o/r/pull/7",
						state: "MERGED",
						author: { name: "", login: "octocat" },
					},
				]),
				exitCode: 0,
			}),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]).toMatchObject({
			prAuthor: "octocat",
			prState: "MERGED",
		});
	});

	it("leaves prAuthor/prState null when no PR (open or merged) matches the branch", async () => {
		const exec = gitExec({
			log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }),
			"merge-base": () => ({ stdout: "", exitCode: 1 }),
			"gh:open": () => ({ stdout: "[]", exitCode: 0 }),
			"gh:merged": () => ({
				stdout: JSON.stringify([
					{ number: 3, headRefName: "some-other", state: "MERGED" },
				]),
				exitCode: 0,
			}),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]).toMatchObject({
			prAuthor: null,
			prState: null,
			prNumber: null,
			// No PR data for this branch; the merged marker is the local verdict (false).
			merged: false,
			// No PR → approved is unknown (null), not false.
			approved: null,
		});
	});

	it("stamps approved=true from an OPEN PR whose reviewDecision is APPROVED (green marker, not merged)", async () => {
		// The approved-but-not-yet-merged case the green marker exists for.
		const exec = gitExec({
			log: () => ({ stdout: "1\tIan Chiu\ti@x\tabc123\n", exitCode: 0 }),
			"merge-base": () => ({ stdout: "", exitCode: 1 }), // not merged locally
			"gh:open": () => ({
				stdout: JSON.stringify([
					{
						number: 42,
						headRefName: "JUS-1",
						url: "https://github.com/o/r/pull/42",
						state: "OPEN",
						author: { name: "Ian Chiu", login: "noootown" },
						reviewDecision: "APPROVED",
					},
				]),
				exitCode: 0,
			}),
			"gh:merged": () => ({ stdout: "[]", exitCode: 0 }),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]).toMatchObject({
			approved: true,
			prState: "OPEN",
			// Approved does NOT imply merged — the TUI's merged marker yields to the
			// approved marker here.
			merged: false,
		});
	});

	it("stamps approved=false when the PR's reviewDecision is CHANGES_REQUESTED", async () => {
		const exec = gitExec({
			log: () => ({ stdout: "1\tIan Chiu\ti@x\tabc123\n", exitCode: 0 }),
			"merge-base": () => ({ stdout: "", exitCode: 1 }),
			"gh:open": () => ({
				stdout: JSON.stringify([
					{
						number: 43,
						headRefName: "JUS-1",
						url: "https://github.com/o/r/pull/43",
						state: "OPEN",
						author: { name: "Ian Chiu", login: "noootown" },
						reviewDecision: "CHANGES_REQUESTED",
					},
				]),
				exitCode: 0,
			}),
			"gh:merged": () => ({ stdout: "[]", exitCode: 0 }),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]).toMatchObject({
			approved: false,
			prState: "OPEN",
		});
	});

	it("treats a missing gh binary as no data (prNumber null, no throw)", async () => {
		const exec = gitExec({
			log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }),
			gh: () => {
				throw new Error("spawn gh ENOENT");
			},
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]).toMatchObject({
			prNumber: null,
			lastCommitHash: "abc123", // git still enriched
		});
	});

	it("treats a gh error (non-zero exit, e.g. unauthenticated) as no data", async () => {
		const exec = gitExec({
			log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }),
			gh: () => ({ stdout: "gh: not logged in\n", exitCode: 1 }),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]).toMatchObject({
			prNumber: null,
			prUrl: null,
		});
	});

	it("shells gh at most twice (open + merged) per repo per sweep, not once per worktree", async () => {
		const counts: Record<string, number> = {};
		const exec = gitExec(
			{
				log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }),
				gh: () => ({
					stdout: JSON.stringify([
						{
							number: 5,
							headRefName: "JUS-1",
							url: "https://github.com/o/r/pull/5",
						},
					]),
					exitCode: 0,
				}),
			},
			(key) => {
				counts[key] = (counts[key] ?? 0) + 1;
			},
		);
		const { engine, base } = setup({
			exec,
			resolverIO: {
				listWorktrees: async () => [
					{ name: "JUS-1", path: join(base, "wt-jus1"), branch: "JUS-1" },
					{ name: "JUS-2", path: join(base, "wt-jus2"), branch: "JUS-2" },
				],
			},
		});
		await engine.tick();
		await engine.refreshGitEnrichment();
		// Two worktrees, one repo → exactly two gh calls (one open list + one
		// merged list per repo, NOT per worktree); git log ran per worktree.
		expect(counts.gh).toBe(2);
		expect(counts.log).toBe(2);
		// The matching branch got the PR; the other stayed null.
		const list = engine.worktreesByRepo().platform ?? [];
		expect(list.find((w) => w.branch === "JUS-1")?.prNumber).toBe(5);
		expect(list.find((w) => w.branch === "JUS-1")?.prUrl).toBe(
			"https://github.com/o/r/pull/5",
		);
		expect(list.find((w) => w.branch === "JUS-2")?.prNumber).toBeNull();
		expect(list.find((w) => w.branch === "JUS-2")?.prUrl).toBeNull();
	});

	it("marks a branch merged when its HEAD is an ancestor of the default branch (merge-base exit 0)", async () => {
		const exec = gitExec({
			log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }),
			"merge-base": () => ({ stdout: "", exitCode: 0 }),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]?.merged).toBe(true);
	});

	it("marks a branch not merged when merge-base --is-ancestor exits 1", async () => {
		const exec = gitExec({
			log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }),
			"merge-base": () => ({ stdout: "", exitCode: 1 }),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]?.merged).toBe(false);
	});

	it("nulls merged when merge-base fails with an unexpected exit code (e.g. 128)", async () => {
		// 128 = unknown ref / not a repo: a real failure, distinct from the
		// 0/1 ancestor verdict, so the marker is unknown rather than false.
		const exec = gitExec({
			log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }),
			"merge-base": () => ({ stdout: "fatal: bad revision\n", exitCode: 128 }),
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]?.merged).toBeNull();
	});

	it("nulls merged when the merge-base probe throws (e.g. missing git binary)", async () => {
		const exec = gitExec({
			log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }),
			"merge-base": () => {
				throw new Error("spawn git ENOENT");
			},
		});
		const { engine } = setup({ exec });
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]?.merged).toBeNull();
	});

	it("nulls merged for the default-branch checkout itself, without a merge-base call", async () => {
		// A worktree whose branch IS the default branch would be its own ancestor
		// (always "merged"), so the marker is meaningless there — computed as null
		// with no git shell-out at all.
		const calls: string[] = [];
		const { engine, base } = setup({
			resolverIO: {
				listWorktrees: async () => [
					{ name: "platform", path: join(base, "repo"), branch: "main" },
				],
			},
			exec: gitExec(
				{ log: () => ({ stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 }) },
				(key) => calls.push(key),
			),
		});
		await engine.tick();
		await engine.refreshGitEnrichment();
		expect(engine.worktreesByRepo().platform?.[0]?.merged).toBeNull();
		expect(calls).not.toContain("merge-base");
	});

	it("passes the vars.yaml default_branch to the merge-base probe", async () => {
		// The merged marker compares HEAD against the project's configured default
		// branch, not a hard-coded "main".
		let mergeBaseTarget: string | null = null;
		const exec: Exec = async (command, args) => {
			if (command === "git" && args[2] === "merge-base") {
				mergeBaseTarget = args[5] ?? null;
				return { stdout: "", exitCode: 0 };
			}
			if (command === "git" && args[2] === "log") {
				return { stdout: "1\tHopper\th@x\tabc123\n", exitCode: 0 };
			}
			return { stdout: "", exitCode: 0 };
		};
		const { engine, base } = setup({ exec });
		mkdirSync(join(base, "ws", "platform"), { recursive: true });
		writeFileSync(
			join(base, "ws", "platform", "vars.yaml"),
			"default_branch: develop\n",
		);
		await engine.tick();
		await engine.refreshGitEnrichment();
		// JUS-1 ≠ develop, so the probe ran and targeted the configured branch.
		expect(mergeBaseTarget).toBe("develop");
		expect(engine.worktreesByRepo().platform?.[0]?.merged).toBe(true);
	});
});

describe("Engine task chains", () => {
	it("resolves a temp chain's worktree exactly ONCE and stamps it onto the tail", async () => {
		let spawns = 0;
		const { engine, store } = setup({
			resolverIO: {
				listWorktrees: async () => [],
				spawnWorktree: async (_r, name) => {
					spawns += 1;
					return { name, path: `/wt/${name}`, branch: name };
				},
			},
		});
		const [head, tail] = store.createChain(
			[{ prompt: "step one\n" }, { prompt: "step two\n" }],
			{ repo: "platform", ref: "temp", source: "mcp" },
		);
		await engine.tick(); // resolve pass: head spawns once, tail is stamped

		expect(spawns).toBe(1); // NOT two — the tail never re-resolves temp
		const h = store.get(head?.id ?? "");
		const t = store.get(tail?.id ?? "");
		expect(h?.target.worktree).toBeTruthy();
		// Tail lands on the head's lane, ref pinned, ownership left with the head.
		expect(t?.target.worktree).toBe(h?.target.worktree);
		expect(t?.target.ref).toBe(`worktree:${h?.target.worktree}`);
		expect(t?.ephemeralWorktree).toBe(false);
		// Tail is still queued (gated on the head completing), not running/spawned.
		expect(t?.status).toBe("queued");
	});

	it("marks the tail skipped once the head has failed", async () => {
		const { engine, store } = setup();
		const [head, tail] = store.createChain(
			[{ prompt: "one\n" }, { prompt: "two\n" }],
			{ repo: "platform", ref: "worktree:JUS-1", source: "mcp" },
		);
		// Simulate the head having failed (e.g. stopped or errored).
		store.update(head?.id ?? "", { status: "failed", error: "boom" });
		await engine.tick();
		const t = store.get(tail?.id ?? "");
		expect(t?.status).toBe("skipped");
		expect(t?.error).toContain("chain predecessor");
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

	it("kills a running task's child and marks it CANCELLED (user stop), not failed", async () => {
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
		// A signal from a user Stop settles as cancelled (distinct from failed),
		// with the reason preserved in the error field for the detail view.
		expect(t?.status).toBe("cancelled");
		expect(t?.error).toBe("stopped by user");
		expect(t?.finishedAt).toMatch(/^\d{4}-\d{2}-\d{2}T.*Z$/);
	}, 15_000);
});

describe("worktree-deletion archive", () => {
	it("archives a terminal task whose worktree was deleted", async () => {
		// Worktree "JUS-1" is gone from the listing.
		const { engine, store } = setup({
			resolverIO: { listWorktrees: async () => [] },
		});
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		store.update(t.id, {
			status: "failed",
			target: { repo: "platform", ref: "worktree:JUS-1", worktree: "JUS-1" },
		});

		await engine.tick();

		expect(store.list()).toHaveLength(0);
		expect(store.listArchived().map((a) => a.id)).toContain(t.id);
	});

	it("keeps a terminal task whose worktree still exists", async () => {
		// Default listWorktrees returns [{ name: "JUS-1", … }] — worktree present.
		const { engine, store } = setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		store.update(t.id, {
			status: "failed",
			target: { repo: "platform", ref: "worktree:JUS-1", worktree: "JUS-1" },
		});

		await engine.tick();

		expect(store.list().map((x) => x.id)).toContain(t.id);
		expect(store.listArchived()).toHaveLength(0);
	});

	it("does not archive a non-terminal task with a deleted worktree", async () => {
		const { engine, store } = setup({
			resolverIO: { listWorktrees: async () => [] },
		});
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		store.update(t.id, {
			status: "needs-input",
			target: { repo: "platform", ref: "worktree:JUS-1", worktree: "JUS-1" },
		});

		await engine.tick();

		expect(store.list()[0]?.status).toBe("needs-input");
		expect(store.listArchived()).toHaveLength(0);
	});

	it("does not archive @repo or null-worktree terminal tasks", async () => {
		const { engine, store } = setup({
			resolverIO: { listWorktrees: async () => [] },
		});
		const sentinel = store.create({
			prompt: "p",
			repo: "platform",
			ref: "repo",
			source: "tui",
		});
		store.update(sentinel.id, {
			status: "failed",
			target: { repo: "platform", ref: "repo", worktree: "@repo" },
		});
		const noWt = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:X",
			source: "tui",
		});
		store.update(noWt.id, {
			status: "failed",
			target: { repo: "platform", ref: "worktree:X", worktree: null },
		});

		await engine.tick();

		expect(
			store
				.list()
				.map((x) => x.id)
				.sort(),
		).toEqual([sentinel.id, noWt.id].sort());
		expect(store.listArchived()).toHaveLength(0);
	});

	it("archives every terminal status on worktree deletion", async () => {
		const statuses = [
			"done",
			"failed",
			"skipped",
			"cancelled",
			"verify-failed",
		] as const;
		for (const status of statuses) {
			const { engine, store } = setup({
				resolverIO: { listWorktrees: async () => [] },
			});
			const t = store.create({
				prompt: "p",
				repo: "platform",
				ref: "worktree:JUS-1",
				source: "tui",
			});
			store.update(t.id, {
				status,
				target: { repo: "platform", ref: "worktree:JUS-1", worktree: "JUS-1" },
			});

			await engine.tick();

			expect(store.list(), `status ${status} should be archived`).toHaveLength(
				0,
			);
			expect(store.listArchived().map((a) => a.id)).toContain(t.id);
		}
	});

	it("does not archive when the repo has never listed successfully", async () => {
		// listWorktrees always throws → refreshWorktreeCache seeds [] for the repo,
		// but the listing never succeeded, so "absent" must NOT count as deleted.
		const { engine, store } = setup({
			resolverIO: {
				listWorktrees: async () => {
					throw new Error("git unavailable");
				},
			},
		});
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		store.update(t.id, {
			status: "failed",
			target: { repo: "platform", ref: "worktree:JUS-1", worktree: "JUS-1" },
		});

		await engine.tick();

		expect(store.list().map((x) => x.id)).toContain(t.id);
		expect(store.listArchived()).toHaveLength(0);
	});

	it("keeps a terminal task when listing fails after a prior success", async () => {
		// First tick: listWorktrees succeeds (worktree present) → cache is
		// seeded and worktreeListingOk is set. Second tick: listWorktrees
		// throws (transient git hiccup) → refreshWorktreeCache's catch branch
		// keeps the last-known list instead of clobbering it with []. The
		// worktree must still be found in that last-known list, so the task
		// must NOT be archived on the second tick.
		let shouldThrow = false;
		const { engine, store } = setup({
			resolverIO: {
				listWorktrees: async (path) => {
					if (shouldThrow) throw new Error("git unavailable");
					return [{ name: "JUS-1", path, branch: "JUS-1" }];
				},
			},
		});
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		store.update(t.id, {
			status: "failed",
			target: { repo: "platform", ref: "worktree:JUS-1", worktree: "JUS-1" },
		});

		await engine.tick();
		expect(store.list().map((x) => x.id)).toContain(t.id);
		expect(store.listArchived()).toHaveLength(0);

		shouldThrow = true;
		await engine.tick();

		expect(store.list().map((x) => x.id)).toContain(t.id);
		expect(store.listArchived()).toHaveLength(0);
	});
});
