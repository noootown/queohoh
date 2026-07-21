import { execFile } from "node:child_process";
import { basename } from "node:path";
import type { ResolverIO, WorktreeInfo } from "./resolver.js";

export type Exec = (
	command: string,
	args: string[],
	opts: { cwd: string },
) => Promise<{ stdout: string; exitCode: number }>;

export const defaultExec: Exec = (command, args, opts) =>
	new Promise((resolve) => {
		execFile(command, args, { cwd: opts.cwd }, (error, stdout) => {
			const exitCode =
				error && typeof error.code === "number" ? error.code : error ? 1 : 0;
			resolve({ stdout: stdout ?? "", exitCode });
		});
	});

export function parseWorktreePorcelain(output: string): WorktreeInfo[] {
	const result: WorktreeInfo[] = [];
	let path: string | undefined;
	let branch: string | undefined;
	const flush = () => {
		// Only emit entries that have both a path and a branch; detached/bare
		// entries (no `branch` line) are skipped.
		if (path && branch) result.push({ name: basename(path), path, branch });
		path = undefined;
		branch = undefined;
	};
	// A new `worktree` line starts a fresh entry, so entries are separated by
	// their attribute lines rather than relying on blank-line terminators.
	for (const line of output.split("\n")) {
		if (line.startsWith("worktree ")) {
			flush();
			path = line.slice("worktree ".length);
		} else if (line.startsWith("branch ")) {
			branch = line.slice("branch ".length).replace(/^refs\/heads\//, "");
		}
	}
	flush();
	return result;
}

export function createResolverIO(exec: Exec): ResolverIO {
	async function listWorktrees(repoPath: string): Promise<WorktreeInfo[]> {
		const { stdout, exitCode } = await exec(
			"git",
			["worktree", "list", "--porcelain"],
			{ cwd: repoPath },
		);
		if (exitCode !== 0) return [];
		return parseWorktreePorcelain(stdout);
	}

	return {
		listWorktrees,

		async prBranch(repoPath, number) {
			const { stdout, exitCode } = await exec(
				"gh",
				["pr", "view", String(number), "--json", "headRefName"],
				{ cwd: repoPath },
			);
			if (exitCode !== 0) return null;
			try {
				const parsed = JSON.parse(stdout) as { headRefName?: string };
				return parsed.headRefName ?? null;
			} catch {
				return null;
			}
		},

		async spawnWorktree(repoPath, name, branch) {
			// `branch` given (the PR flow) means "check out this EXISTING branch":
			// fetch it and switch WITHOUT -c — `wt switch -c` would mint a brand-new
			// branch of the same name off HEAD, silently landing the worktree on
			// main's tip instead of the PR. No branch (ticket/temp flows) keeps the
			// create-new-branch semantics.
			if (branch) {
				// Both best-effort: fetch may be offline, --track fails when the
				// local branch already exists. `wt switch` is the load-bearing step.
				await exec("git", ["fetch", "origin", branch], { cwd: repoPath });
				await exec("git", ["branch", "--track", branch, `origin/${branch}`], {
					cwd: repoPath,
				});
			}
			// `--yes`: the daemon is non-interactive — without it Worktrunk
			// refuses to run project post-start hooks (platform's mise/uv/docker
			// setup) and spawn fails with "Cannot prompt for approval".
			// `--no-cd`: we only need the worktree created; the daemon never
			// changes its own cwd into it.
			const args = branch
				? ["--yes", "switch", "--no-cd", branch]
				: ["--yes", "switch", "--no-cd", "-c", name];
			const { exitCode } = await exec("wt", args, { cwd: repoPath });
			if (exitCode === 0) {
				const after = await listWorktrees(repoPath);
				// Prefer branch match: Worktrunk path templates are often
				// `{repo}.{branch}` (slashes folded), not the slash→dash name
				// we pass for PR refs — basename equality alone misses them.
				const spawned =
					after.find((w) => w.branch === (branch ?? name)) ??
					after.find((w) => w.name === name);
				if (spawned) return spawned;
			}
			throw new Error(`failed to spawn worktree: ${name}`);
		},

		async removeWorktree(repoPath, worktree) {
			// Force the worktree clean so `wt remove` can proceed (mirrors
			// agent247's cleanup-worktree.sh — this deliberately discards
			// uncommitted changes). `exec` never rejects, so reset/clean are
			// inherently best-effort; only `wt remove`'s exit code is load-bearing.
			await exec("git", ["reset", "--hard", "HEAD"], { cwd: worktree.path });
			await exec("git", ["clean", "-fd"], { cwd: worktree.path });
			const { exitCode } = await exec(
				"wt",
				["--yes", "remove", worktree.branch],
				{ cwd: repoPath },
			);
			if (exitCode !== 0) {
				throw new Error(`failed to remove worktree: ${worktree.name}`);
			}
			// Best-effort: wt may have already deleted the branch.
			await exec("git", ["branch", "-D", worktree.branch], { cwd: repoPath });
		},
	};
}
