import { execFile } from "node:child_process";
import { lstat } from "node:fs/promises";
import { homedir } from "node:os";
import { basename } from "node:path";
import {
	findWorktree,
	type ResolverIO,
	type WorktreeInfo,
} from "./resolver.js";
import { executeBoundedCommand } from "./runner.js";

export type Exec = (
	command: string,
	args: string[],
	/** Opt-in hard deadline for shell-outs such as discovery; omitted means no limit. */
	opts: { cwd: string; timeoutMs?: number },
) => Promise<{ stdout: string; stderr?: string; exitCode: number }>;

export const defaultExec: Exec = (command, args, opts) => {
	if (opts.timeoutMs !== undefined && opts.timeoutMs > 0) {
		return executeBoundedCommand(command, args, {
			cwd: opts.cwd,
			timeoutMs: opts.timeoutMs,
		});
	}
	return new Promise((resolve) => {
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
};

/**
 * Filesystem seam used only by the post-`wt remove` settle (below). Injected so
 * the removal contract is unit-testable without touching a real tree.
 */
export interface ResolverFs {
	exists(path: string): Promise<boolean>;
	/** Legacy injection seam; recursive recovery is intentionally never called. */
	remove?(path: string): Promise<void>;
}

export const defaultResolverFs: ResolverFs = {
	async exists(path) {
		try {
			await lstat(path);
			return true;
		} catch (error) {
			// Only ENOENT proves absence. Permission/I/O errors leave removal
			// unknown and must stop teardown before branch deletion.
			if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
			throw error;
		}
	},
};

/** How long to wait for the worktree path to actually be free before reporting
 * that manual inspection is required: 60 × 250ms = 15s. Bounded because a hung delete must not
 * stall the engine pass that is reaping.
 *
 * On wt >= 0.79 this costs one `stat` and returns immediately (see
 * `settleRemoval`); it is kept as version-agnostic insurance, not as the
 * primary mechanism. */
const REMOVE_SETTLE_ATTEMPTS = 60;
const REMOVE_SETTLE_INTERVAL_MS = 250;

const delay = (ms: number): Promise<void> =>
	new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Pull the offending path out of wt's refusal, e.g.
 * `✗ Directory already exists: ~/dev/platform.feature-x`.
 *
 * Taken from wt's own message rather than rebuilt from repo + name because the
 * worktree layout is configurable (`wt config create`), so guessing the path
 * could point the husk cleanup at the wrong directory. `~` is expanded because
 * wt prints the home-relative form. Returns null when the output is any other
 * failure — the caller must then not touch the filesystem at all.
 */
export function parseExistingDirPath(
	output: string,
	home: string = homedir(),
): string | null {
	// A diagnostic spanning lines cannot distinguish a newline in a filename
	// from unrelated output. Refuse it rather than truncate the candidate.
	const line = output.endsWith("\n") ? output.slice(0, -1) : output;
	if (
		[...line].some(
			(char) => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127,
		)
	)
		return null;
	const match = /^(?:✗ )?Directory already exists: (.+)$/.exec(line);
	const raw = match?.[1];
	if (!raw || raw.trim() !== raw) return null;
	if (!raw.startsWith("/") && !raw.startsWith("~/")) return null;
	if (raw.split("/").some((part) => part === "." || part === "..")) return null;
	return raw.startsWith("~/") ? `${home}/${raw.slice(2)}` : raw;
}

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

export function createResolverIO(
	exec: Exec,
	fsio: ResolverFs = defaultResolverFs,
	/** Settle tuning — tests shrink these so the linger path doesn't wait 15s. */
	opts: { settleAttempts?: number; settleIntervalMs?: number } = {},
): ResolverIO {
	const settleAttempts = opts.settleAttempts ?? REMOVE_SETTLE_ATTEMPTS;
	const settleIntervalMs = opts.settleIntervalMs ?? REMOVE_SETTLE_INTERVAL_MS;
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

	/** Wait for the provider's background delete, but never take over deletion.
	 * An unregistered leftover might have been replaced by user data while we
	 * waited. Provider success alone cannot establish ownership of that path. */
	async function settleRemoval(path: string, repoPath: string): Promise<void> {
		for (let i = 0; i < settleAttempts; i++) {
			if (!(await fsio.exists(path))) return;
			await delay(settleIntervalMs);
		}
		if (!(await fsio.exists(path))) return;
		let stillRegistered = false;
		try {
			stillRegistered = (await listWorktrees(repoPath)).some(
				(wt) => wt.path === path,
			);
		} catch {
			// Listing failed (git contention): treat as "unknown", which must not
			// read as "safe to delete" — same conservatism as the engine's reaper.
			stillRegistered = true;
		}
		if (stillRegistered) {
			throw new Error(
				`worktree ${path} still registered after wt remove reported success`,
			);
		}
		throw new Error(
			`worktree path ${path} remains after wt remove; manual inspection required`,
		);
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
				.filter((s) => s?.trim())
				.join("\n")
				.trim();

			// Ticket/temp create (-c): a prior partial spawn often left the
			// branch on disk. wt then prints "Branch X already exists" and
			// exits non-zero — open the existing branch without -c.
			if (result.exitCode !== 0 && !branch) {
				const reopen = await exec("wt", ["switch", "--yes", "--no-cd", name], {
					cwd: repoPath,
				});
				const reopenDetail = [reopen.stderr, reopen.stdout]
					.filter((s) => s?.trim())
					.join("\n")
					.trim();
				if (reopen.exitCode === 0) {
					result = reopen;
					detail = reopenDetail;
				} else if (reopenDetail) {
					detail = detail ? `${detail}\n${reopenDetail}` : reopenDetail;
				}
			}

			// Always re-list: post-create hooks can exit non-zero AFTER the
			// worktree directory exists (mise sync / docker-setup). Adopt it so
			// the task runs instead of failing with a generic spawn error while
			// leaving an orphan worktree the ticket matcher used to miss.
			const after = await listWorktrees(repoPath);
			const spawned = findWorktree(after, name, branch);
			if (spawned) return spawned;

			// A refusal, missing registration or missing .git is never proof that
			// a directory belongs to us. Leave every leftover for manual inspection.

			const suffix = detail ? `: ${detail.slice(0, 500)}` : "";
			throw new Error(`failed to spawn worktree: ${name}${suffix}`);
		},

		async removeWorktree(repoPath, worktree) {
			// Force the worktree clean so `wt remove` can proceed (mirrors
			// a cleanup-worktree script — this deliberately discards
			// uncommitted changes). `exec` never rejects, so reset/clean are
			// inherently best-effort; only `wt remove`'s exit code is load-bearing.
			await exec("git", ["reset", "--hard", "HEAD"], { cwd: worktree.path });
			// Explicit manual removal retains its historical untracked cleanup;
			// ignored output must not be preemptively erased if removal later fails.
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
			// Some provider versions return before deletion ends. Wait for the
			// original path to disappear and surface leftovers without erasing them.
			await settleRemoval(worktree.path, repoPath);
			// Best-effort: wt may have already deleted the branch.
			await exec("git", ["branch", "-D", worktree.branch], { cwd: repoPath });
		},
	};
}
