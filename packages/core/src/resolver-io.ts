import { execFile } from "node:child_process";
import { basename } from "node:path";
import {
	findWorktree,
	type ResolverIO,
	type WorktreeInfo,
} from "./resolver.js";

export type Exec = (
	command: string,
	args: string[],
	opts: { cwd: string },
) => Promise<{ stdout: string; stderr?: string; exitCode: number }>;

export const defaultExec: Exec = (command, args, opts) =>
	new Promise((resolve) => {
		execFile(command, args, { cwd: opts.cwd }, (error, stdout, stderr) => {
			const exitCode =
				error && typeof error.code === "number" ? error.code : error ? 1 : 0;
			resolve({
				stdout: stdout ?? "",
				stderr: stderr ?? "",
				exitCode,
			});
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
		// Fail loud so Engine.refreshWorktreeCache can KEEP the last-known list.
		// Returning [] on error (with listingOk still true) made every worktree
		// look deleted and hard-purged terminal tasks for still-existing WTs
		// (e.g. long-lived platform.TICK-1946 while cleaning up other branches).
		if (exitCode !== 0) {
			throw new Error(
				`git worktree list failed in ${repoPath} (exit ${exitCode})`,
			);
		}
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
			// `--yes` / `--no-cd` are SWITCH subcommand flags, not global wt
			// options (`wt --yes switch …` fails with "unexpected argument
			// '--yes'"; tip: `switch --yes`). Without `--yes` Worktrunk refuses
			// project post-start hooks in a non-TTY daemon ("Cannot prompt for
			// approval"). `--no-cd`: create only — never change the daemon cwd.
			const createArgs = branch
				? (["switch", "--yes", "--no-cd", branch] as string[])
				: (["switch", "--yes", "--no-cd", "-c", name] as string[]);
			let result = await exec("wt", createArgs, { cwd: repoPath });
			let detail = [result.stderr, result.stdout]
				.filter((s) => s && s.trim())
				.join("\n")
				.trim();

			// Ticket/temp create (-c): a prior partial spawn often left the
			// branch on disk. wt then prints "Branch X already exists" and
			// exits non-zero — open the existing branch without -c.
			if (result.exitCode !== 0 && !branch) {
				const reopen = await exec(
					"wt",
					["switch", "--yes", "--no-cd", name],
					{ cwd: repoPath },
				);
				const reopenDetail = [reopen.stderr, reopen.stdout]
					.filter((s) => s && s.trim())
					.join("\n")
					.trim();
				if (reopen.exitCode === 0) {
					result = reopen;
					detail = reopenDetail;
				} else if (reopenDetail) {
					detail = detail
						? `${detail}\n${reopenDetail}`
						: reopenDetail;
				}
			}

			// Always re-list: post-create hooks can exit non-zero AFTER the
			// worktree directory exists (mise sync / docker-setup). Adopt it so
			// the task runs instead of failing with a generic spawn error while
			// leaving an orphan worktree the ticket matcher used to miss.
			const after = await listWorktrees(repoPath);
			const spawned = findWorktree(after, name, branch);
			if (spawned) return spawned;

			const suffix = detail ? `: ${detail.slice(0, 500)}` : "";
			throw new Error(`failed to spawn worktree: ${name}${suffix}`);
		},

		async removeWorktree(repoPath, worktree) {
			// Force the worktree clean so `wt remove` can proceed (mirrors
			// a cleanup-worktree script — this deliberately discards
			// uncommitted changes). `exec` never rejects, so reset/clean are
			// inherently best-effort; only `wt remove`'s exit code is load-bearing.
			await exec("git", ["reset", "--hard", "HEAD"], { cwd: worktree.path });
			await exec("git", ["clean", "-fd"], { cwd: worktree.path });
			// Same flag placement as switch: `remove --yes`, not `--yes remove`.
			const { exitCode } = await exec(
				"wt",
				["remove", "--yes", worktree.branch],
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
