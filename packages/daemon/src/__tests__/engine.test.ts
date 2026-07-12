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
		executeVerify?: VerifyExecutor;
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
		models: {},
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
		executeVerify:
			overrides.executeVerify ??
			(async () => ({
				exitCode: 0,
				timedOut: false,
				signal: null,
				output: "",
			})),
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
			models: {},
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

	it("resolves the task model alias through the project vars.yaml models block", async () => {
		const { engine, store, base } = setup();
		mkdirSync(join(base, "ws", "platform"), { recursive: true });
		writeFileSync(
			join(base, "ws", "platform", "vars.yaml"),
			"models:\n  sonnet: claude-sonnet-4-6\n",
		);
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
			model: "sonnet",
		});
		await engine.tick(); // resolve
		await engine.tick(); // start
		await engine.drain();
		const id = store.list()[0]?.id ?? "";
		expect(store.list()[0]?.status).toBe("done");
		expect(new RunStore(join(base, "runs")).readRunMeta(id)?.model).toBe(
			"claude-sonnet-4-6",
		);
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
		// dirty; log "" → epoch parseInt(NaN) → null, empty author/hash → null; and
		// `gh pr list` "" → JSON.parse throws → prNumber/prUrl null (failure-tolerant).
		expect(engine.worktreesByRepo()).toEqual({
			platform: [
				{
					name: "JUS-1",
					path: join(base, "wt-jus1"),
					branch: "JUS-1",
					dirty: false,
					lastCommitEpoch: null,
					lastCommitAuthor: null,
					lastCommitAuthorEmail: null,
					lastCommitHash: null,
					prNumber: null,
					prUrl: null,
				},
			],
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
			// git: args = ["-C", path, <subcommand>, ...rest]; gh: command === "gh".
			const key = command === "gh" ? "gh" : (args[2] ?? "");
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

	it("shells gh at most once per repo per sweep, not once per worktree", async () => {
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
		// Two worktrees, one repo → exactly one gh call; git log ran per worktree.
		expect(counts.gh).toBe(1);
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
