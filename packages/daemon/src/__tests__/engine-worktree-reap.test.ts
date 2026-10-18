import { mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type {
	Exec,
	GlobalConfig,
	ProviderConfig,
	ResolverIO,
	WorktreeInfo,
} from "@queohoh/core";
import {
	BUILTIN_CATALOG,
	createResolverIO,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionLineageStore,
	SessionRegistry,
} from "@queohoh/core";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Engine } from "../engine.js";

afterEach(() => {
	vi.restoreAllMocks();
});

const TEST_PROVIDERS: ProviderConfig[] = [{ name: "claude", enabled: true }];

const DAY = 86_400_000;
/** Fixed wall clock so age assertions never depend on real time. */
const NOW = Date.parse("2026-09-14T12:00:00.000Z");

interface Opts {
	worktrees: WorktreeInfo[];
	/** Worktree paths whose `git status --porcelain` reports changes. */
	dirtyPaths?: string[];
	/** Worktree paths whose `git status` exits non-zero → `dirty` is unknown. */
	statusFailPaths?: string[];
	/** Epoch SECONDS reported by `git log` for every worktree. */
	lastCommitEpoch?: number;
	worktreeRetentionDays?: number | null;
	favorites?: string[];
	protectedWorktrees?: string[];
	/** Branches that `gh pr list --state open` reports an open PR for. */
	openPrBranches?: string[];
	/** Make every `gh` call fail, as on a machine without gh / with bad auth. */
	ghUnavailable?: boolean;
	ghOpenUnavailable?: boolean;
	now?: number;
	/** No durable daemon creation receipt exists for these paths. */
	unownedPaths?: string[];
	ignoredPaths?: string[];
}

function setup(opts: Opts) {
	const base = mkdtempSync(join(tmpdir(), "qo-reap-"));
	const repoPath = join(base, "repo");
	mkdirSync(repoPath, { recursive: true });
	const workspace = join(base, "ws");
	mkdirSync(join(workspace, "platform"), { recursive: true });
	if (opts.protectedWorktrees) {
		writeFileSync(
			join(workspace, "platform", "vars.yaml"),
			`protected_worktrees:\n${opts.protectedWorktrees.map((n) => `  - ${n}\n`).join("")}`,
		);
	}
	const store = new QueueStore(join(base, "state"));
	const config: GlobalConfig = {
		workspace,
		projects: [{ name: "platform", path: repoPath }],
		maxConcurrentTasks: 3,
		purgeAfterDays: 3650, // never age-purge task records inside a test
		archiveAfterDays: 3650,
		worktreeRetentionDays: opts.worktreeRetentionDays ?? null,
		vars: {},
		catalog: BUILTIN_CATALOG,
		defaultModels: ["claude/claude-opus-5"],
		providers: TEST_PROVIDERS,
	};
	const removed: string[] = [];
	const resolverIO: ResolverIO = {
		listWorktrees: async () => opts.worktrees,
		prBranch: async () => null,
		spawnWorktree: async (_r, name) => ({
			name,
			path: join(base, `wt-${name}`),
			branch: name,
		}),
		ownedWorktreeToken: async (_repoPath, wt) =>
			opts.unownedPaths?.includes(wt.path) ? null : `created:${wt.path}`,
		removeOwnedWorktree: async (_repoPath, wt, token, stillAllowed) => {
			if (token !== `created:${wt.path}` || !stillAllowed()) return false;
			removed.push(wt.name);
			opts.worktrees = opts.worktrees.filter((entry) => entry.path !== wt.path);
			return true;
		},
		removeWorktree: async (_repoPath, wt) => {
			removed.push(wt.name);
		},
	};
	const dirty = new Set(opts.dirtyPaths ?? []);
	const statusFail = new Set(opts.statusFailPaths ?? []);
	const epoch = opts.lastCommitEpoch ?? Math.floor(NOW / 1000);
	const openPrs = opts.openPrBranches ?? [];
	const exec = vi.fn<Exec>(async (cmd, args) => {
		if (cmd === "gh") {
			if (
				opts.ghOpenUnavailable &&
				(args.includes("open") || args[0] === "api")
			)
				return { stdout: "", exitCode: 1 };
			if (opts.ghUnavailable) return { stdout: "", exitCode: 127 };
			const open = args.includes("open");
			const headAt = args.indexOf("--head");
			if (headAt >= 0)
				return {
					stdout: JSON.stringify(
						openPrs.includes(args[headAt + 1] ?? "") ? [{ number: 900 }] : [],
					),
					exitCode: 0,
				};
			return {
				stdout: JSON.stringify(
					open
						? openPrs.map((headRefName, i) => ({
								number: 900 + i,
								headRefName,
								baseRefName: "main",
								url: `https://example.test/pr/${900 + i}`,
								state: "OPEN",
								author: { name: "Kevin", login: "koshea" },
								labels: [],
							}))
						: [],
				),
				exitCode: 0,
			};
		}
		const at = args.indexOf("-C");
		const path = at >= 0 ? (args[at + 1] ?? "") : "";
		if (args.includes("status")) {
			if (statusFail.has(path)) return { stdout: "", exitCode: 1 };
			return {
				stdout: dirty.has(path)
					? " M a.ts\n"
					: opts.ignoredPaths?.includes(path) &&
							args.includes("--ignored=matching")
						? "!! secrets.env\n"
						: "",
				exitCode: 0,
			};
		}
		if (args.includes("log")) {
			return {
				stdout: args.includes("--format=%ct")
					? String(epoch)
					: `${epoch}\tKevin\tk@example.com\tabc1234`,
				exitCode: 0,
			};
		}
		return { stdout: "", exitCode: 0 };
	});
	const favorites = new Set(opts.favorites ?? []);
	// The sweep must skip a guarded worktree outright, not discover the guard by
	// calling removeWorktree and catching its throw — that would log an error on
	// every tick forever. Captured so a test can assert nothing was attempted.
	const errors: string[] = [];
	vi.spyOn(console, "error").mockImplementation((...args: unknown[]) => {
		errors.push(args.map(String).join(" "));
	});
	const registry = new SessionRegistry(join(base, "sessions.json"));
	const engine = new Engine({
		store,
		runStore: new RunStore(join(base, "runs")),
		registry,
		config,
		resolverIO,
		exec,
		executeClaude: async () => {
			throw new Error("no runs in these tests");
		},
		executeVerify: async () => ({
			exitCode: 0,
			timedOut: false,
			signal: null,
			output: "",
		}),
		redact: makeRedactor(new Map()),
		lineage: new SessionLineageStore(join(base, "session-lineage.json")),
		now: () => opts.now ?? NOW,
		isWorktreeFavorite: (key) => favorites.has(key),
	});
	return {
		engine,
		store,
		removed,
		errors,
		dirty,
		resolverIO,
		exec,
		registry,
		favorites,
		workspace,
	};
}

/** A task already settled on `worktree`, as the sweep will find it. */
function settledTask(
	store: QueueStore,
	worktree: string,
	extra: { ephemeralWorktree?: boolean; status?: "done" | "needs-input" } = {},
) {
	const t = store.create({
		prompt: "p",
		repo: "platform",
		ref: `worktree:${worktree}`,
		source: "tui",
	});
	store.update(t.id, {
		status: extra.status ?? "done",
		// Pinned to the fixed clock: left unset, the store stamps the REAL time,
		// and the retention assertions silently drift as the calendar moves.
		finishedAt: new Date(NOW).toISOString(),
		target: { repo: "platform", ref: `worktree:${worktree}`, worktree },
		ephemeralWorktree: extra.ephemeralWorktree ?? false,
	});
	return t;
}

/** Tick once to fill the worktree cache, land enrichment deterministically
 * (production kicks it fire-and-forget), then tick again to run the sweep. */
async function reapPass(engine: Engine) {
	await engine.tick();
	await engine.refreshGitEnrichment();
	await engine.refreshWorktreeCleanup();
}

const PRIMARY: WorktreeInfo = {
	name: "platform",
	path: "/repo/platform",
	branch: "main",
};

function spawned(name: string): WorktreeInfo {
	return { name, path: `/repo/${name}`, branch: name.replace("platform.", "") };
}

describe("Engine worktree reap", () => {
	it("removes an ephemeral worktree once its task has settled", async () => {
		const wt = spawned("platform.qoo-adhoc-ab12");
		const { engine, store, removed } = setup({ worktrees: [PRIMARY, wt] });
		settledTask(store, wt.name, { ephemeralWorktree: true });

		await reapPass(engine);

		expect(removed).toEqual([wt.name]);
	});

	it("keeps an ephemeral worktree while a live task still targets it", async () => {
		// `needs-input` is live but never scheduled, so this exercises the sweep's
		// live guard rather than the scheduler.
		const wt = spawned("platform.qoo-adhoc-ab12");
		const { engine, store, removed } = setup({ worktrees: [PRIMARY, wt] });
		settledTask(store, wt.name, {
			ephemeralWorktree: true,
			status: "needs-input",
		});

		await reapPass(engine);

		expect(removed).toEqual([]);
	});

	it("removes an idle {repo}.{branch} worktree past the retention window", async () => {
		const wt = spawned("platform.JUS-2763");
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			worktreeRetentionDays: 14,
			lastCommitEpoch: Math.floor((NOW - 30 * DAY) / 1000),
			now: NOW + 30 * DAY, // task settled ~now, so push the clock past both
		});
		settledTask(store, wt.name);

		await reapPass(engine);

		expect(removed).toEqual([wt.name]);
	});

	it("keeps a {repo}.{branch} worktree inside the retention window", async () => {
		const wt = spawned("platform.JUS-2763");
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			worktreeRetentionDays: 14,
			lastCommitEpoch: Math.floor((NOW - 2 * DAY) / 1000),
		});
		settledTask(store, wt.name);

		await reapPass(engine);

		expect(removed).toEqual([]);
	});

	it("never reaps a persistent worktree when retention is unset", async () => {
		const wt = spawned("platform.JUS-2763");
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			worktreeRetentionDays: null,
			lastCommitEpoch: Math.floor((NOW - 900 * DAY) / 1000),
			now: NOW + 900 * DAY,
		});
		settledTask(store, wt.name);

		await reapPass(engine);

		expect(removed).toEqual([]);
	});

	it("never reaps a hand-made worktree outside the {repo}. convention", async () => {
		// platform-dns / platform-hotfix are the operator's own checkouts. Idle,
		// clean, and old is not permission to delete someone's working tree.
		const wt: WorktreeInfo = {
			name: "platform-dns",
			path: "/repo/platform-dns",
			branch: "feature/sourcing-p4-ui",
		};
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			unownedPaths: [wt.path],
			worktreeRetentionDays: 1,
			lastCommitEpoch: Math.floor((NOW - 900 * DAY) / 1000),
			now: NOW + 900 * DAY,
		});
		settledTask(store, wt.name);

		await reapPass(engine);

		expect(removed).toEqual([]);
	});

	it("never reaps a spawned-shape worktree with no task record", async () => {
		const wt = spawned("platform.JUS-9999");
		const { engine, removed } = setup({
			worktrees: [PRIMARY, wt],
			worktreeRetentionDays: 1,
			lastCommitEpoch: Math.floor((NOW - 900 * DAY) / 1000),
			now: NOW + 900 * DAY,
		});

		await reapPass(engine);

		expect(removed).toEqual([]);
	});

	it("never reaps a worktree with uncommitted changes", async () => {
		const wt = spawned("platform.qoo-adhoc-ab12");
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			dirtyPaths: [wt.path],
		});
		settledTask(store, wt.name, { ephemeralWorktree: true });

		await reapPass(engine);

		expect(removed).toEqual([]);
	});

	it("never reaps a worktree whose dirty state is unknown", async () => {
		// git failed, so "clean" was never established. Fail closed: the one
		// thing removal destroys is uncommitted work.
		const wt = spawned("platform.qoo-adhoc-ab12");
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			statusFailPaths: [wt.path],
		});
		settledTask(store, wt.name, { ephemeralWorktree: true });

		await reapPass(engine);

		expect(removed).toEqual([]);
	});

	it("never reaps a worktree whose branch has an open PR", async () => {
		// The branch is still in play: a review comment lands and the tree should
		// be there to answer it.
		const wt = spawned("platform.JUS-2763");
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			worktreeRetentionDays: 1,
			openPrBranches: [wt.branch],
			lastCommitEpoch: Math.floor((NOW - 900 * DAY) / 1000),
			now: NOW + 900 * DAY,
		});
		settledTask(store, wt.name);

		await reapPass(engine);

		expect(removed).toEqual([]);
	});

	it("never reaps a persistent worktree when gh could not report PRs", async () => {
		// gh silence makes every prNumber null, which reads exactly like "no open
		// PR". Refuse to act on that absence rather than reap a live branch.
		const wt = spawned("platform.JUS-2763");
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			worktreeRetentionDays: 1,
			ghUnavailable: true,
			lastCommitEpoch: Math.floor((NOW - 900 * DAY) / 1000),
			now: NOW + 900 * DAY,
		});
		settledTask(store, wt.name);

		await reapPass(engine);

		expect(removed).toEqual([]);
	});

	it("keeps an ephemeral worktree when gh is unavailable", async () => {
		// An unavailable OPEN lookup cannot establish safety for a throwaway tree.
		const wt = spawned("platform.qoo-adhoc-ab12");
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			ghUnavailable: true,
		});
		settledTask(store, wt.name, { ephemeralWorktree: true });

		await reapPass(engine);

		expect(removed).toEqual([]);
	});

	it("skips a protected worktree without attempting removal", async () => {
		const wt = spawned("platform.JUS-2763");
		const { engine, store, removed, errors } = setup({
			worktrees: [PRIMARY, wt],
			worktreeRetentionDays: 1,
			protectedWorktrees: ["JUS-2763"],
			lastCommitEpoch: Math.floor((NOW - 900 * DAY) / 1000),
			now: NOW + 900 * DAY,
		});
		settledTask(store, wt.name);

		await reapPass(engine);

		expect(removed).toEqual([]);
		// removeWorktree would also refuse — but reaching it means logging a
		// failure every tick for as long as the worktree lives.
		expect(errors).toEqual([]);
	});

	it("skips a favorited worktree without attempting removal", async () => {
		const wt = spawned("platform.JUS-2763");
		const { engine, store, removed, errors } = setup({
			worktrees: [PRIMARY, wt],
			worktreeRetentionDays: 1,
			favorites: [`platform/${wt.name}`],
			lastCommitEpoch: Math.floor((NOW - 900 * DAY) / 1000),
			now: NOW + 900 * DAY,
		});
		settledTask(store, wt.name);

		await reapPass(engine);

		expect(removed).toEqual([]);
		expect(errors).toEqual([]);
	});
});
describe("review regression cases", () => {
	it("preserves changes made after cached clean status", async () => {
		const wt = spawned("platform.qoo-adhoc-ab12");
		const { engine, store, removed, dirty } = setup({
			worktrees: [PRIMARY, wt],
		});
		const t = settledTask(store, wt.name, {
			ephemeralWorktree: true,
			status: "needs-input",
		});
		await reapPass(engine);
		dirty.add(wt.path);
		store.update(t.id, { status: "done" });
		await engine.tick();
		await engine.refreshWorktreeCleanup();
		expect(removed).toEqual([]);
	});
	it("does not interpret successful merged lookup as successful open lookup", async () => {
		const wt = spawned("platform.feature");
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			worktreeRetentionDays: 1,
			lastCommitEpoch: Math.floor((NOW - 30 * DAY) / 1000),
			now: NOW + 30 * DAY,
			ghOpenUnavailable: true,
		});
		settledTask(store, wt.name);
		await reapPass(engine);
		expect(removed).toEqual([]);
	});
	it("keeps a manually created convention-named tree used by a worktree-ref task", async () => {
		const wt = spawned("platform.manual-checkout");
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			unownedPaths: [wt.path],
			worktreeRetentionDays: 1,
			lastCommitEpoch: Math.floor((NOW - 30 * DAY) / 1000),
			now: NOW + 30 * DAY,
		});
		settledTask(store, wt.name); // explicit worktree ref: daemon never spawned this tree
		await reapPass(engine);
		expect(removed).toEqual([]);
	});
	it("rejects new work with a retry error while the deletion listing is pending", async () => {
		const wt = spawned("platform.qoo-adhoc-ab12");
		const { engine, store, removed, resolverIO } = setup({
			worktrees: [PRIMARY, wt],
		});
		const t = settledTask(store, wt.name, {
			ephemeralWorktree: true,
			status: "needs-input",
		});
		await reapPass(engine);
		store.update(t.id, { status: "done" });
		let lists = 0;
		const original = resolverIO.listWorktrees;
		resolverIO.listWorktrees = async (path) => {
			lists++;
			if (lists === 2)
				expect(() =>
					settledTask(store, wt.name, { status: "needs-input" }),
				).toThrow(/retry/);
			return original(path);
		};
		await engine.tick();
		await engine.refreshWorktreeCleanup();
		expect(removed).toEqual([wt.name]);
		expect(store.get(t.id)?.status).toBe("done");
	});
});

describe("private destructive resolver integration", () => {
	it("never issues reset or clean for edits made after the cached clean observation", async () => {
		const wt = spawned("platform.qoo-adhoc-ab12");
		const { engine, store, dirty, resolverIO } = setup({
			worktrees: [PRIMARY, wt],
		});
		const t = settledTask(store, wt.name, {
			ephemeralWorktree: true,
			status: "needs-input",
		});
		await reapPass(engine);
		dirty.add(wt.path);
		store.update(t.id, { status: "done" });
		// Execute CURRENT PRIVATE createResolverIO.removeWorktree, with shell IO fully
		// mocked. The Engine and worktree-ops are real; no git/wt process is launched.
		const commands: string[] = [];
		resolverIO.removeWorktree = createResolverIO(async (cmd, args) => {
			commands.push([cmd, ...args].join(" "));
			return { stdout: "", exitCode: 0 };
		}).removeWorktree;
		await engine.tick();
		await engine.refreshWorktreeCleanup();
		expect(commands).toEqual([]);
	});
	it("does not authorize reap from four-minute-old cold clean data", async () => {
		vi.useFakeTimers();
		vi.setSystemTime(NOW);
		try {
			const wt = spawned("platform.feature");
			const { engine, store, dirty, removed } = setup({
				worktrees: [PRIMARY, wt],
				worktreeRetentionDays: 1,
				lastCommitEpoch: Math.floor((NOW - 30 * DAY) / 1000),
				now: NOW + 30 * DAY,
			});
			// No live/recent task makes this checkout hot. The initial clean sample
			// becomes eligible for the private five-minute cold TTL.
			await reapPass(engine);
			dirty.add(wt.path);
			vi.setSystemTime(NOW + 4 * 60_000);
			const oldTask = settledTask(store, wt.name);
			store.update(oldTask.id, {
				finishedAt: new Date(NOW - 30 * DAY).toISOString(),
			});
			await engine.refreshGitEnrichment();
			expect(
				engine.worktreesByRepo().platform?.find((w) => w.name === wt.name)
					?.dirty,
			).toBe(false);
			await engine.tick();
			await engine.refreshWorktreeCleanup();
			expect(removed).toEqual([]);
		} finally {
			vi.useRealTimers();
		}
	});
});

/** Pause the fresh branch-specific lookup, leaving enrichment IO untouched. */
function pauseOpenLookup(exec: ReturnType<typeof setup>["exec"]) {
	const original = exec.getMockImplementation();
	if (!original) throw new Error("missing exec implementation");
	let enter = () => {};
	let resume = () => {};
	const entered = new Promise<void>((resolve) => {
		enter = resolve;
	});
	const released = new Promise<void>((resolve) => {
		resume = resolve;
	});
	exec.mockImplementation(async (cmd, args, options) => {
		if (cmd === "gh" && args.includes("--head")) {
			enter();
			await released;
		}
		return original(cmd, args, options);
	});
	return { entered, resume };
}

async function warmCandidate(engine: Engine) {
	await engine.tick();
	await engine.refreshGitEnrichment();
}

describe("automatic cleanup safety boundary", () => {
	it("preserves ignored files even though cached display status is clean", async () => {
		const wt = spawned("platform.qoo-ignored");
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			ignoredPaths: [wt.path],
		});
		settledTask(store, wt.name, { ephemeralWorktree: true });
		await reapPass(engine);
		expect(
			engine.worktreesByRepo().platform?.find((entry) => entry.path === wt.path)
				?.dirty,
		).toBe(false);
		expect(removed).toEqual([]);
	});

	it("requires a fresh successful exact OPEN query after a cached success", async () => {
		const wt = spawned("platform.qoo-pr-fails");
		const opts: Opts = { worktrees: [PRIMARY, wt] };
		const { engine, store, removed, exec } = setup(opts);
		settledTask(store, wt.name, { ephemeralWorktree: true });
		await warmCandidate(engine);
		opts.ghOpenUnavailable = true;
		await engine.refreshWorktreeCleanup();
		expect(
			exec.mock.calls.some(
				([cmd, args]) =>
					cmd === "gh" &&
					args.join(" ") ===
						`pr list --head ${wt.branch} --state open --json number --limit 1`,
			),
		).toBe(true);
		expect(removed).toEqual([]);
	});

	it("keeps a favorited task's worktree and task record", async () => {
		const wt = spawned("platform.qoo-favorite");
		const { engine, store, removed } = setup({ worktrees: [PRIMARY, wt] });
		const task = settledTask(store, wt.name, { ephemeralWorktree: true });
		store.update(task.id, { favorite: true });
		await reapPass(engine);
		expect(removed).toEqual([]);
		expect(store.get(task.id)?.favorite).toBe(true);
	});

	it("keeps a tree when an interactive session arrives during preflight", async () => {
		const wt = spawned("platform.qoo-session");
		const { engine, store, removed, exec, registry } = setup({
			worktrees: [PRIMARY, wt],
		});
		settledTask(store, wt.name, { ephemeralWorktree: true });
		await warmCandidate(engine);
		const pause = pauseOpenLookup(exec);
		const cleanup = engine.refreshWorktreeCleanup();
		await pause.entered;
		registry.upsertInteractive(`${wt.path}/src`, process.pid);
		pause.resume();
		await cleanup;
		expect(removed).toEqual([]);
	});

	it("rechecks project protection after a slow lookup", async () => {
		const wt = spawned("platform.qoo-protected-later");
		const { engine, store, removed, exec, workspace } = setup({
			worktrees: [PRIMARY, wt],
		});
		settledTask(store, wt.name, { ephemeralWorktree: true });
		await warmCandidate(engine);
		const pause = pauseOpenLookup(exec);
		const cleanup = engine.refreshWorktreeCleanup();
		await pause.entered;
		writeFileSync(
			join(workspace, "platform", "vars.yaml"),
			`protected_worktrees:\n  - ${wt.name}\n`,
		);
		pause.resume();
		await cleanup;
		expect(removed).toEqual([]);
	});

	it("rechecks favorites inside the resolver's last callback", async () => {
		const wt = spawned("platform.qoo-final-favorite");
		const { engine, store, removed, favorites, resolverIO } = setup({
			worktrees: [PRIMARY, wt],
		});
		settledTask(store, wt.name, { ephemeralWorktree: true });
		const original = resolverIO.removeOwnedWorktree;
		resolverIO.removeOwnedWorktree = async (...args) => {
			favorites.add(`platform/${wt.name}`);
			return original?.(...args) ?? false;
		};
		await reapPass(engine);
		expect(removed).toEqual([]);
	});

	it("blocks enqueue and manual name reuse while pending and releases on failure", async () => {
		const wt = spawned("platform.qoo-reservation");
		const opts: Opts = { worktrees: [PRIMARY, wt] };
		const { engine, store, removed, exec } = setup(opts);
		const task = settledTask(store, wt.name, { ephemeralWorktree: true });
		await warmCandidate(engine);
		const pause = pauseOpenLookup(exec);
		const cleanup = engine.refreshWorktreeCleanup();
		await pause.entered;
		expect(() =>
			store.create({
				prompt: "new",
				repo: "platform",
				ref: `worktree:${wt.name}`,
				source: "tui",
			}),
		).toThrow(/retry/);
		await expect(engine.removeWorktree("platform", wt.branch)).rejects.toThrow(
			/retry/,
		);
		await expect(engine.createWorktree("platform", wt.branch)).rejects.toThrow(
			/retry/,
		);
		opts.ghOpenUnavailable = true;
		pause.resume();
		await cleanup;
		expect(removed).toEqual([]);
		expect(store.get(task.id)?.status).toBe("done");
		expect(() =>
			store.create({
				prompt: "retry",
				repo: "platform",
				ref: `worktree:${wt.name}`,
				source: "tui",
			}),
		).not.toThrow();
		// The engine operation token is released as well; manual removal can run.
		await expect(
			engine.removeWorktree("platform", wt.name),
		).resolves.toBeUndefined();
	});

	it("does not block scheduling on a pending fresh PR query and is single-flight", async () => {
		const wt = spawned("platform.qoo-slow");
		const { engine, store, exec } = setup({ worktrees: [PRIMARY, wt] });
		settledTask(store, wt.name, { ephemeralWorktree: true });
		await warmCandidate(engine);
		const pause = pauseOpenLookup(exec);
		const cleanup = engine.refreshWorktreeCleanup();
		await pause.entered;
		expect(engine.refreshWorktreeCleanup()).toBe(cleanup);
		await engine.tick();
		expect(
			exec.mock.calls.filter(
				([cmd, args]) => cmd === "gh" && args.includes("--head"),
			),
		).toHaveLength(1);
		pause.resume();
		await cleanup;
	});

	it("uses a fresh last commit when cached history looks old", async () => {
		const wt = spawned("platform.new-commit");
		const { engine, store, removed, exec } = setup({
			worktrees: [PRIMARY, wt],
			worktreeRetentionDays: 1,
			now: NOW + 30 * DAY,
			lastCommitEpoch: Math.floor((NOW - 30 * DAY) / 1000),
		});
		settledTask(store, wt.name);
		await warmCandidate(engine);
		const original = exec.getMockImplementation();
		if (!original) throw new Error("missing exec implementation");
		exec.mockImplementation(async (cmd, args, options) =>
			args.includes("--format=%ct")
				? { stdout: String((NOW + 30 * DAY) / 1000), exitCode: 0 }
				: original(cmd, args, options),
		);
		await engine.refreshWorktreeCleanup();
		expect(removed).toEqual([]);
	});

	it("detects direct disk task changes immediately before removal", async () => {
		const wt = spawned("platform.qoo-disk-edit");
		const { engine, store, removed, resolverIO } = setup({
			worktrees: [PRIMARY, wt],
		});
		const task = settledTask(store, wt.name, { ephemeralWorktree: true });
		const original = resolverIO.removeOwnedWorktree;
		resolverIO.removeOwnedWorktree = async (...args) => {
			const path = store.taskPath(task.id);
			const { readFileSync } = await import("node:fs");
			writeFileSync(
				path,
				readFileSync(path, "utf8").replace(
					"status: done",
					"status: needs-input",
				),
			);
			return original?.(...args) ?? false;
		};
		await reapPass(engine);
		expect(removed).toEqual([]);
		expect(store.get(task.id)?.status).toBe("needs-input");
	});
});

describe("cleanup resolver aliases and unknown targets", () => {
	it.each([
		["worktree:feature/topic", "feature/topic"],
		["worktree:nested.checkout", "feature/topic"],
		["worktree:checkout", "feature/topic"],
		["ticket:JUS-123", "JUS-123"],
		["JUS-123", "JUS-123"],
		["pr:123", "feature/topic"],
		["temp", "feature/topic"],
		["unknown", "feature/topic"],
	])("keeps a checkout with live unresolved %s", async (ref, branch) => {
		const wt: WorktreeInfo = {
			name: "custom.nested.checkout",
			path: "/repo/custom.nested.checkout",
			branch,
		};
		const { engine, store, removed } = setup({ worktrees: [PRIMARY, wt] });
		settledTask(store, wt.name, { ephemeralWorktree: true });
		const live = store.create({
			prompt: "live",
			repo: "platform",
			ref,
			source: "tui",
		});
		store.update(live.id, { status: "needs-input" });
		await reapPass(engine);
		expect(removed).toEqual([]);
		expect(store.get(live.id)?.status).toBe("needs-input");
	});

	it("reserves branch and suffix aliases during asynchronous removal", async () => {
		const wt: WorktreeInfo = {
			name: "custom.nested.checkout",
			path: "/repo/custom.nested.checkout",
			branch: "JUS-123",
		};
		const { engine, store, exec, removed } = setup({
			worktrees: [PRIMARY, wt],
		});
		settledTask(store, wt.name, { ephemeralWorktree: true });
		await warmCandidate(engine);
		const pause = pauseOpenLookup(exec);
		const cleanup = engine.refreshWorktreeCleanup();
		await pause.entered;
		for (const ref of [
			"worktree:JUS-123",
			"worktree:nested.checkout",
			"worktree:checkout",
			"ticket:JUS-123",
			"pr:123",
		]) {
			expect(() =>
				store.create({ prompt: "live", repo: "platform", ref, source: "tui" }),
			).toThrow(/retry/);
		}
		await expect(engine.createWorktree("platform", "JUS-123")).rejects.toThrow(
			/retry/,
		);
		pause.resume();
		await cleanup;
		expect(removed).toEqual([wt.name]);
	});

	it.each([
		"{}",
		"null",
		"",
		"[",
		'[{"number":1}]',
	])("does not authorize removal with incomplete PR output %s", async (stdout) => {
		const wt = spawned("platform.qoo-bad-json");
		const { engine, store, exec, removed } = setup({
			worktrees: [PRIMARY, wt],
		});
		settledTask(store, wt.name, { ephemeralWorktree: true });
		await warmCandidate(engine);
		const original = exec.getMockImplementation();
		if (!original) throw new Error("missing exec implementation");
		exec.mockImplementation(async (cmd, args, options) =>
			cmd === "gh" && args.includes("--head")
				? { stdout, exitCode: 0 }
				: original(cmd, args, options),
		);
		await engine.refreshWorktreeCleanup();
		expect(removed).toEqual([]);
	});

	it("keeps a replacement listed at the same name with a different branch", async () => {
		const wt = spawned("platform.qoo-replaced");
		const { engine, store, removed, resolverIO } = setup({
			worktrees: [PRIMARY, wt],
		});
		settledTask(store, wt.name, { ephemeralWorktree: true });
		await warmCandidate(engine);
		resolverIO.listWorktrees = async () => [
			PRIMARY,
			{ ...wt, branch: "other" },
		];
		await engine.refreshWorktreeCleanup();
		expect(removed).toEqual([]);
	});

	it("requires the safe resolver methods even when naming and task history match", async () => {
		const wt = spawned("platform.qoo-old-resolver");
		const { engine, store, removed, resolverIO } = setup({
			worktrees: [PRIMARY, wt],
		});
		settledTask(store, wt.name, { ephemeralWorktree: true });
		delete resolverIO.removeOwnedWorktree;
		delete resolverIO.ownedWorktreeToken;
		await reapPass(engine);
		expect(removed).toEqual([]);
	});

	it("retains worktree and history when live task files are malformed", async () => {
		const wt = spawned("platform.qoo-invalid-task");
		const { engine, store, removed } = setup({ worktrees: [PRIMARY, wt] });
		const task = settledTask(store, wt.name, { ephemeralWorktree: true });
		writeFileSync(join(store.tasksDir, "broken.md"), "invalid frontmatter");
		await reapPass(engine);
		expect(removed).toEqual([]);
		expect(store.get(task.id)?.status).toBe("done");
	});
});

describe("cleanup keeps its last activity evidence", () => {
	it.each([
		14, 30,
	])("retains one task with purge14 while worktree retention is %i", async (retention) => {
		const wt = spawned("platform.long-retention");
		const opts: Opts = {
			worktrees: [PRIMARY, wt],
			worktreeRetentionDays: retention,
			now: NOW + 20 * DAY,
			lastCommitEpoch: Math.floor((NOW - 30 * DAY) / 1000),
		};
		const { engine, store, removed } = setup(opts);
		const task = settledTask(store, wt.name);
		store.update(task.id, { purgeAfterDays: 14 });
		await reapPass(engine);
		if (retention === 30) {
			expect(removed).toEqual([]);
			expect(store.get(task.id)?.status).toBe("done");
			opts.now = NOW + 31 * DAY;
			await engine.tick();
			await engine.refreshWorktreeCleanup();
		}
		expect(removed).toEqual([wt.name]);
	});

	it("purges terminal history normally when worktree retention is null", async () => {
		const wt = spawned("platform.no-retention");
		const { engine, store, removed } = setup({
			worktrees: [PRIMARY, wt],
			now: NOW + 20 * DAY,
		});
		const task = settledTask(store, wt.name);
		store.update(task.id, { purgeAfterDays: 14 });
		await reapPass(engine);
		expect(store.get(task.id)).toBeUndefined();
		expect(removed).toEqual([]);
	});
});

it("protects an interactive session reached through a symlink", async () => {
	const base = mkdtempSync(join(tmpdir(), "qo-reap-session-"));
	const ownedPath = join(base, "platform.qoo-owned");
	const aliasPath = join(base, "interactive-alias");
	mkdirSync(ownedPath);
	symlinkSync(ownedPath, aliasPath);
	const wt: WorktreeInfo = {
		name: "platform.qoo-owned",
		path: ownedPath,
		branch: "qoo-owned",
	};
	const { engine, store, registry, removed } = setup({
		worktrees: [PRIMARY, wt],
	});
	settledTask(store, wt.name, { ephemeralWorktree: true });
	registry.upsertInteractive(aliasPath, process.pid);
	await reapPass(engine);
	expect(removed).toEqual([]);
});
