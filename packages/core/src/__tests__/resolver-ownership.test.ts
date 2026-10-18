import {
	mkdtemp,
	readFile,
	realpath,
	rename,
	rm,
	symlink,
	writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { createResolverIO, defaultExec, type Exec } from "../resolver-io.js";

const roots: string[] = [];
afterEach(async () => {
	for (const root of roots.splice(0))
		await rm(root, { recursive: true, force: true });
});

/** Real git owns every checkout; only the external wt provider is replaced. */
async function fixture() {
	const root = await realpath(await mkdtemp(join(tmpdir(), "qoo-ownership-")));
	roots.push(root);
	const repo = join(root, "repo");
	await defaultExec("git", ["init", repo], { cwd: root });
	await defaultExec(
		"git",
		[
			"-c",
			"user.name=Test",
			"-c",
			"user.email=test@example.com",
			"commit",
			"--allow-empty",
			"-m",
			"init",
		],
		{ cwd: repo },
	);
	const calls: string[][] = [];
	let failAfterCreate = false;
	const exec: Exec = async (command, args, opts) => {
		calls.push([command, ...args]);
		if (command !== "wt") return defaultExec(command, args, opts);
		if (args[0] !== "switch")
			throw new Error("automatic cleanup must not run wt hooks");
		const branch = args.at(-1);
		if (!branch) throw new Error("missing branch");
		const result = await defaultExec(
			"git",
			[
				"worktree",
				"add",
				...(args.includes("-c") ? ["-b", branch] : []),
				join(root, branch),
				...(args.includes("-c") ? [] : [branch]),
			],
			opts,
		);
		return failAfterCreate ? { ...result, exitCode: 1 } : result;
	};
	return {
		root,
		repo,
		exec,
		calls,
		failCreateHook: () => {
			failAfterCreate = true;
		},
	};
}

describe("durable worktree ownership", () => {
	it("records a new creation across resolver restarts and removes only its clean checkout", async () => {
		const f = await fixture();
		const wt = await createResolverIO(f.exec).spawnWorktree(f.repo, "owned");
		const io = createResolverIO(f.exec);
		const token = await io.ownedWorktreeToken?.(f.repo, wt);
		expect(token).toEqual(expect.any(String));
		expect(
			await io.removeOwnedWorktree?.(f.repo, wt, token ?? "", () => true),
		).toBe(true);
		expect(await io.listWorktrees(f.repo)).toHaveLength(1);
		expect(
			(
				await defaultExec("git", ["show-ref", "--verify", "refs/heads/owned"], {
					cwd: f.repo,
				})
			).exitCode,
		).toBe(0);
		expect(
			f.calls.filter(
				(c) => c[0] === "git" && c[1] === "worktree" && c[2] === "remove",
			),
		).toEqual([["git", "worktree", "remove", "--", wt.path]]);
	});

	it("never claims a manual checkout or a reused existing branch", async () => {
		const f = await fixture();
		await defaultExec(
			"git",
			["worktree", "add", "-b", "manual", join(f.root, "manual")],
			{ cwd: f.repo },
		);
		const io = createResolverIO(f.exec);
		const manual = (await io.listWorktrees(f.repo)).find(
			(w) => w.branch === "manual",
		);
		if (!manual) throw new Error("manual checkout not found");
		expect(await io.ownedWorktreeToken?.(f.repo, manual)).toBeNull();
		await defaultExec("git", ["branch", "existing"], { cwd: f.repo });
		const reused = await io.spawnWorktree(f.repo, "existing", "existing");
		expect(await io.ownedWorktreeToken?.(f.repo, reused)).toBeNull();
	});

	it("does not turn a failed first creation and reopen into ownership", async () => {
		const f = await fixture();
		f.failCreateHook();
		const io = createResolverIO(f.exec);
		const wt = await io.spawnWorktree(f.repo, "partial");
		expect(await io.ownedWorktreeToken?.(f.repo, wt)).toBeNull();
	});

	it("refuses metadata that redirects git status to a different checkout", async () => {
		const f = await fixture();
		const io = createResolverIO(f.exec);
		const wt = await io.spawnWorktree(f.repo, "redirected");
		expect(await io.ownedWorktreeToken?.(f.repo, wt)).toEqual(
			expect.any(String),
		);
		await defaultExec(
			"git",
			["worktree", "add", "-b", "other", join(f.root, "other")],
			{ cwd: f.repo },
		);
		await defaultExec("git", ["config", "extensions.worktreeConfig", "true"], {
			cwd: f.repo,
		});
		await defaultExec(
			"git",
			["config", "--worktree", "core.worktree", join(f.root, "other")],
			{ cwd: wt.path },
		);
		expect(await io.ownedWorktreeToken?.(f.repo, wt)).toBeNull();
	});

	it.each([
		"untracked",
		"ignored",
		"tracked",
	])("preserves %s data added after the ownership token", async (kind) => {
		const f = await fixture();
		if (kind === "tracked") {
			await writeFile(join(f.repo, "tracked"), "original");
			await defaultExec("git", ["add", "tracked"], { cwd: f.repo });
			await defaultExec(
				"git",
				[
					"-c",
					"user.name=Test",
					"-c",
					"user.email=test@example.com",
					"commit",
					"-m",
					"tracked",
				],
				{ cwd: f.repo },
			);
		}
		const io = createResolverIO(f.exec);
		const wt = await io.spawnWorktree(f.repo, "dirty");
		const token = await io.ownedWorktreeToken?.(f.repo, wt);
		expect(token).toEqual(expect.any(String));
		if (kind === "ignored") {
			const gitdir = (
				await defaultExec("git", ["rev-parse", "--git-common-dir"], {
					cwd: wt.path,
				})
			).stdout.trim();
			await writeFile(join(gitdir, "info", "exclude"), "valuable\n");
		}
		const file = join(wt.path, kind === "tracked" ? "tracked" : "valuable");
		await writeFile(file, "must survive");
		expect(
			await io.removeOwnedWorktree?.(f.repo, wt, token ?? "", () => true),
		).toBe(false);
		expect(await readFile(file, "utf8")).toBe("must survive");
	});

	it("rechecks a revocation immediately before git removal", async () => {
		const f = await fixture();
		const io = createResolverIO(f.exec);
		const wt = await io.spawnWorktree(f.repo, "revoked");
		const token = await io.ownedWorktreeToken?.(f.repo, wt);
		expect(token).toEqual(expect.any(String));
		expect(
			await io.removeOwnedWorktree?.(f.repo, wt, token ?? "", () => false),
		).toBe(false);
		expect(await io.listWorktrees(f.repo)).toHaveLength(2);
	});

	it("checks ignored output again after final identity inspection yields", async () => {
		const f = await fixture();
		const initial = createResolverIO(f.exec);
		const wt = await initial.spawnWorktree(f.repo, "late-output");
		const common = (
			await defaultExec("git", ["rev-parse", "--git-common-dir"], {
				cwd: wt.path,
			})
		).stdout.trim();
		await writeFile(join(common, "info", "exclude"), "valuable\n");
		const token = await initial.ownedWorktreeToken?.(f.repo, wt);
		expect(token).toEqual(expect.any(String));
		let identities = 0;
		const racingExec: Exec = async (command, args, opts) => {
			const result = await f.exec(command, args, opts);
			if (
				command === "git" &&
				args[0] === "symbolic-ref" &&
				++identities === 2
			) {
				await writeFile(join(wt.path, "valuable"), "late ignored output");
			}
			return result;
		};
		expect(
			await createResolverIO(racingExec).removeOwnedWorktree?.(
				f.repo,
				wt,
				token ?? "",
				() => true,
			),
		).toBe(false);
		expect(await readFile(join(wt.path, "valuable"), "utf8")).toBe(
			"late ignored output",
		);
	});

	it("refuses a commit added during cleanup without revoking durable ownership", async () => {
		const f = await fixture();
		const initial = createResolverIO(f.exec);
		const wt = await initial.spawnWorktree(f.repo, "recent-commit");
		const token = await initial.ownedWorktreeToken?.(f.repo, wt);
		expect(token).toEqual(expect.any(String));
		let committed = false;
		const racingExec: Exec = async (command, args, opts) => {
			const result = await f.exec(command, args, opts);
			if (command === "git" && args[0] === "status" && !committed) {
				committed = true;
				const commit = await defaultExec(
					"git",
					[
						"-c",
						"user.name=Test",
						"-c",
						"user.email=test@example.com",
						"commit",
						"--allow-empty",
						"-m",
						"new work during cleanup",
					],
					{ cwd: wt.path },
				);
				expect(commit.exitCode).toBe(0);
			}
			return result;
		};
		expect(
			await createResolverIO(racingExec).removeOwnedWorktree?.(
				f.repo,
				wt,
				token ?? "",
				() => true,
			),
		).toBe(false);
		expect(await initial.listWorktrees(f.repo)).toHaveLength(2);
		const restarted = createResolverIO(f.exec);
		const current = await restarted.ownedWorktreeToken?.(f.repo, wt);
		expect(current).toEqual(expect.any(String));
		expect(current).not.toBe(token);
		expect(
			await restarted.removeOwnedWorktree?.(
				f.repo,
				wt,
				current ?? "",
				() => true,
			),
		).toBe(true);
	});

	it("refuses a replaced directory even when its .git file was copied", async () => {
		const f = await fixture();
		const io = createResolverIO(f.exec);
		const wt = await io.spawnWorktree(f.repo, "replaced");
		const token = await io.ownedWorktreeToken?.(f.repo, wt);
		expect(token).toEqual(expect.any(String));
		await rename(wt.path, `${wt.path}.original`);
		await defaultExec("mkdir", [wt.path], { cwd: f.root });
		await writeFile(
			join(wt.path, ".git"),
			await readFile(join(`${wt.path}.original`, ".git")),
		);
		await writeFile(join(wt.path, "valuable"), "replacement");
		expect(await io.ownedWorktreeToken?.(f.repo, wt)).toBeNull();
		expect(
			await io.removeOwnedWorktree?.(f.repo, wt, token ?? "", () => true),
		).toBe(false);
		expect(await readFile(join(wt.path, "valuable"), "utf8")).toBe(
			"replacement",
		);
	});

	it("refuses symlinks, changed branches, locked trees and the primary checkout", async () => {
		const f = await fixture();
		const io = createResolverIO(f.exec);
		const wt = await io.spawnWorktree(f.repo, "guarded");
		const token = await io.ownedWorktreeToken?.(f.repo, wt);
		expect(token).toEqual(expect.any(String));
		const primary = (await io.listWorktrees(f.repo))[0];
		if (!primary) throw new Error("primary checkout not found");
		expect(await io.ownedWorktreeToken?.(f.repo, primary)).toBeNull();
		await defaultExec("git", ["worktree", "lock", wt.path], { cwd: f.repo });
		expect(
			await io.removeOwnedWorktree?.(f.repo, wt, token ?? "", () => true),
		).toBe(false);
		await defaultExec("git", ["worktree", "unlock", wt.path], { cwd: f.repo });
		await defaultExec("git", ["checkout", "-b", "changed"], { cwd: wt.path });
		expect(
			await io.removeOwnedWorktree?.(f.repo, wt, token ?? "", () => true),
		).toBe(false);
		await defaultExec("git", ["checkout", "guarded"], { cwd: wt.path });
		await rename(wt.path, `${wt.path}.original`);
		await symlink(`${wt.path}.original`, wt.path);
		expect(
			await io.removeOwnedWorktree?.(f.repo, wt, token ?? "", () => true),
		).toBe(false);
	});
});
