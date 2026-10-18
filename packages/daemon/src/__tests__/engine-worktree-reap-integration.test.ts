import { mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
	BUILTIN_CATALOG,
	createResolverIO,
	defaultExec,
	type Exec,
	type GlobalConfig,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionLineageStore,
	SessionRegistry,
} from "@queohoh/core";
import { afterEach, describe, expect, it } from "vitest";
import { Engine } from "../engine.js";

const roots: string[] = [];
afterEach(async () => {
	for (const root of roots.splice(0))
		await rm(root, { recursive: true, force: true });
});

/** Exercise the production engine, creation receipt, metadata checks and actual
 * Git deletion together. Only network PR lookup and the wt CLI are substituted;
 * all git and filesystem operations stay inside a disposable repository. */
async function setup() {
	const root = await realpath(
		await mkdtemp(join(tmpdir(), "qoo-reap-integration-")),
	);
	roots.push(root);
	const repo = join(root, "repo");
	const calls: string[][] = [];
	const exec: Exec = async (command, args, opts) => {
		calls.push([command, ...args]);
		if (command === "gh") return { stdout: "[]", exitCode: 0 };
		if (command === "wt") {
			if (args[0] !== "switch" || !args.includes("-c")) {
				throw new Error("automatic removal must never invoke wt hooks");
			}
			const branch = args.at(-1);
			if (!branch) throw new Error("missing fixture branch");
			return defaultExec(
				"git",
				["worktree", "add", "-b", branch, join(root, branch)],
				opts,
			);
		}
		return defaultExec(command, args, opts);
	};
	const git = (args: string[]) => defaultExec("git", args, { cwd: repo });
	expect(
		(await defaultExec("git", ["init", repo], { cwd: root })).exitCode,
	).toBe(0);
	await writeFile(join(repo, "tracked.txt"), "original\n");
	expect((await git(["add", "tracked.txt"])).exitCode).toBe(0);
	expect(
		(
			await git([
				"-c",
				"user.name=Reviewer",
				"-c",
				"user.email=review@example.test",
				"commit",
				"-m",
				"init",
			])
		).exitCode,
	).toBe(0);
	const resolverIO = createResolverIO(exec);
	const wt = await resolverIO.spawnWorktree(repo, "owned");
	expect(await resolverIO.ownedWorktreeToken?.(repo, wt)).toEqual(
		expect.any(String),
	);
	const store = new QueueStore(join(root, "state"));
	const config: GlobalConfig = {
		workspace: join(root, "workspace"),
		projects: [{ name: "repo", path: repo }],
		maxConcurrentTasks: 1,
		purgeAfterDays: 14,
		archiveAfterDays: 14,
		worktreeRetentionDays: null,
		vars: {},
		catalog: BUILTIN_CATALOG,
		defaultModels: ["claude/claude-opus-5"],
		providers: [{ name: "claude", enabled: true }],
	};
	const engine = new Engine({
		store,
		config,
		resolverIO,
		exec,
		runStore: new RunStore(join(root, "runs")),
		registry: new SessionRegistry(join(root, "sessions.json")),
		lineage: new SessionLineageStore(join(root, "lineage.json")),
		redact: makeRedactor(new Map()),
		executeClaude: async () => {
			throw new Error("these settled tasks must never execute");
		},
		executeVerify: async () => ({
			exitCode: 0,
			timedOut: false,
			signal: null,
			output: "",
		}),
	});
	const task = store.create({
		repo: "repo",
		ref: "worktree:owned",
		source: "tui",
		prompt: "fixture",
	});
	store.update(task.id, {
		status: "needs-input",
		target: { ...task.target, worktree: wt.name },
		ephemeralWorktree: true,
	});
	// Establish a genuinely clean display cache before every safety mutation.
	await engine.tick();
	await engine.refreshGitEnrichment();
	await engine.refreshWorktreeCleanup();
	expect(
		engine.worktreesByRepo().repo?.find((entry) => entry.path === wt.path)
			?.dirty,
	).toBe(false);
	const clean = async () => {
		store.update(task.id, { status: "done" });
		await engine.refreshWorktreeCleanup();
	};
	return { repo, wt, calls, git, resolverIO, clean, config, store };
}

describe("Engine cleanup with real resolver ownership and Git", () => {
	it("removes a genuinely created clean checkout and retains its local branch", async () => {
		const f = await setup();
		await f.clean();
		expect(await f.resolverIO.listWorktrees(f.repo)).toHaveLength(1);
		expect(
			(await f.git(["show-ref", "--verify", "refs/heads/owned"])).exitCode,
		).toBe(0);
		expect(
			f.calls.filter(
				(call) =>
					call[0] === "git" && call[1] === "worktree" && call[2] === "remove",
			),
		).toEqual([["git", "worktree", "remove", "--", f.wt.path]]);
		expect(
			f.calls.some(
				(call) =>
					call.includes("reset") ||
					call.includes("clean") ||
					call.includes("--force") ||
					call.includes("-D"),
			),
		).toBe(false);
	});

	it.each([
		"tracked",
		"untracked",
		"ignored",
	])("preserves %s edits made after the clean cache observation", async (kind) => {
		const f = await setup();
		if (kind === "ignored")
			await writeFile(join(f.repo, ".git", "info", "exclude"), "valuable\n");
		const file = join(
			f.wt.path,
			kind === "tracked" ? "tracked.txt" : "valuable",
		);
		await writeFile(file, "must survive\n");
		await f.clean();
		expect(await readFile(file, "utf8")).toBe("must survive\n");
		expect(await f.resolverIO.listWorktrees(f.repo)).toHaveLength(2);
		expect(
			f.calls.some(
				(call) =>
					call[0] === "git" && call[1] === "worktree" && call[2] === "remove",
			),
		).toBe(false);
	});

	it("retains an owned checkout when Git metadata refuses cleanup", async () => {
		const f = await setup();
		expect((await f.git(["worktree", "lock", f.wt.path])).exitCode).toBe(0);
		await f.clean();
		expect(await f.resolverIO.listWorktrees(f.repo)).toHaveLength(2);
		expect(await readFile(join(f.wt.path, "tracked.txt"), "utf8")).toBe(
			"original\n",
		);
		expect(
			(await f.git(["show-ref", "--verify", "refs/heads/owned"])).exitCode,
		).toBe(0);
	});

	it.each([
		"same path",
		"linked checkout",
	])("retains a tree when another project label registers the %s repository", async (kind) => {
		const f = await setup();
		let path = f.repo;
		if (kind === "linked checkout") {
			path = join(f.repo, "..", "alias-checkout");
			expect(
				(await f.git(["worktree", "add", "-b", "alias-checkout", path]))
					.exitCode,
			).toBe(0);
		}
		f.config.projects.push({ name: "alias", path });
		const task = f.store.create({
			repo: "alias",
			ref: "worktree:owned",
			source: "tui",
			prompt: "still using this checkout",
		});
		f.store.update(task.id, {
			status: "needs-input",
			target: { ...task.target, worktree: f.wt.name },
		});
		await f.clean();
		expect(await readFile(join(f.wt.path, "tracked.txt"), "utf8")).toBe(
			"original\n",
		);
		expect(
			f.calls.some(
				(call) =>
					call[0] === "git" && call[1] === "worktree" && call[2] === "remove",
			),
		).toBe(false);
	});
});
