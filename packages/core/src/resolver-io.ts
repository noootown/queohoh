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
			const { exitCode } = await exec("wt", ["switch", "-c", branch ?? name], {
				cwd: repoPath,
			});
			if (exitCode === 0) {
				const after = await listWorktrees(repoPath);
				const spawned =
					after.find((w) => w.name === name) ??
					after.find((w) => w.branch === (branch ?? name));
				if (spawned) return spawned;
			}
			throw new Error(`failed to spawn worktree: ${name}`);
		},
	};
}
