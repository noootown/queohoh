import type { Stats } from "node:fs";
import { dirname, join, resolve } from "node:path";
import type { WorktreeInfo } from "./resolver.js";
import type { Exec } from "./resolver-io.js";

/** Metadata lives outside the checkout so task output cannot accidentally erase
 * it. Tests inject this seam along with Exec; unavailable I/O always refuses. */
export interface OwnershipFs {
	realpath(path: string): Promise<string>;
	lstat(path: string): Promise<Stats>;
	readFile(path: string): Promise<string>;
	writeExclusive(path: string, data: string): Promise<void>;
}

const RECEIPT = "queohoh-owned-v1.json";
const unsafePath = (path: string) =>
	!path ||
	[...path].some(
		(char) => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127,
	);

/** A receipt proves creation, never permission. Every deletion independently
 * rechecks registration, identity, cleanliness and the caller's latest policy. */
export function createWorktreeOwnership(exec: Exec, fsio?: OwnershipFs) {
	async function identity(repoPath: string, wt: WorktreeInfo) {
		if (
			!fsio ||
			unsafePath(repoPath) ||
			unsafePath(wt.path) ||
			unsafePath(wt.branch)
		)
			return null;
		const repo = await fsio.realpath(repoPath);
		const path = await fsio.realpath(wt.path);
		if (repo === path || path !== resolve(wt.path)) return null;
		const git = async (cwd: string, args: string[]) => {
			const result = await exec("git", args, { cwd });
			if (result.exitCode !== 0)
				throw new Error("cannot establish worktree identity");
			return result.stdout.trim();
		};
		// core.worktree can redirect status while leaving gitdir and HEAD
		// unchanged. Its cleanliness would say nothing about the directory removed.
		if (
			(await fsio.realpath(
				await git(path, ["rev-parse", "--show-toplevel"]),
			)) !== path
		)
			return null;
		const repoCommon = await fsio.realpath(
			await git(repo, [
				"rev-parse",
				"--path-format=absolute",
				"--git-common-dir",
			]),
		);
		const common = await fsio.realpath(
			await git(path, [
				"rev-parse",
				"--path-format=absolute",
				"--git-common-dir",
			]),
		);
		const gitdir = await fsio.realpath(
			await git(path, ["rev-parse", "--absolute-git-dir"]),
		);
		if (
			repoCommon !== common ||
			gitdir === common ||
			dirname(gitdir) !== join(common, "worktrees")
		)
			return null;
		const listing = await exec(
			"git",
			["worktree", "list", "--porcelain", "-z"],
			{ cwd: repo },
		);
		if (listing.exitCode !== 0) return null;
		const entries = listing.stdout
			.split("\0\0")
			.map((entry) => entry.split("\0"));
		const matches = entries.filter((fields) =>
			fields.includes(`worktree ${path}`),
		);
		const fields = matches[0];
		if (matches.length !== 1 || !fields) return null;
		if (
			!fields.includes(`branch refs/heads/${wt.branch}`) ||
			fields.some((field) => /^(locked|prunable)( |$)/.test(field))
		)
			return null;
		if (
			(await git(path, ["symbolic-ref", "--quiet", "HEAD"])) !==
			`refs/heads/${wt.branch}`
		)
			return null;
		if (
			(await fsio.realpath(
				(await fsio.readFile(join(gitdir, "gitdir"))).trim(),
			)) !== join(path, ".git")
		)
			return null;
		const locations = [repo, path, join(path, ".git"), gitdir, common];
		const ids = [];
		for (const [index, location] of locations.entries()) {
			const stat = await fsio.lstat(location);
			if (
				stat.isSymbolicLink() ||
				(index === 2 ? !stat.isFile() : !stat.isDirectory())
			)
				return null;
			ids.push({ dev: stat.dev, ino: stat.ino, birthtimeMs: stat.birthtimeMs });
		}
		return { repo, path, branch: wt.branch, gitdir, common, ids };
	}

	async function token(
		repoPath: string,
		wt: WorktreeInfo,
	): Promise<string | null> {
		try {
			const current = await identity(repoPath, wt);
			if (!current || !fsio) return null;
			const receiptPath = join(current.gitdir, RECEIPT);
			const stat = await fsio.lstat(receiptPath);
			if (!stat.isFile() || stat.isSymbolicLink()) return null;
			const receipt = await fsio.readFile(receiptPath);
			if (receipt !== JSON.stringify({ version: 1, identity: current }))
				return null;
			// Ownership survives normal commits, but a cleanup decision's age/PR
			// evidence applies only to the HEAD it inspected. Bind that revision
			// to this transient token instead of changing the durable receipt.
			const head = await exec("git", ["rev-parse", "--verify", "HEAD"], {
				cwd: wt.path,
			});
			if (
				head.exitCode !== 0 ||
				!/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(head.stdout.trim())
			)
				return null;
			return JSON.stringify({ receipt, head: head.stdout.trim() });
		} catch {
			return null;
		}
	}

	return {
		token,
		async record(repoPath: string, wt: WorktreeInfo): Promise<void> {
			try {
				const current = await identity(repoPath, wt);
				if (!current || !fsio) return;
				// Never overwrite: even a stale receipt must not be upgraded into
				// ownership of a replacement checkout or a reopened branch.
				await fsio.writeExclusive(
					join(current.gitdir, RECEIPT),
					JSON.stringify({ version: 1, identity: current }),
				);
			} catch {
				// Ownership is optional. Failed recording leaves a usable checkout
				// that automatic cleanup must preserve.
			}
		},
		async remove(
			repoPath: string,
			wt: WorktreeInfo,
			expected: string,
			stillAllowed: () => boolean,
		): Promise<boolean> {
			try {
				const clean = async () => {
					const status = await exec(
						"git",
						[
							"status",
							"--porcelain=v1",
							"-z",
							"--untracked-files=all",
							"--ignored=matching",
							"--ignore-submodules=none",
						],
						{ cwd: wt.path },
					);
					return status.exitCode === 0 && status.stdout === "";
				};
				if (!expected || (await token(repoPath, wt)) !== expected) return false;
				if (!(await clean())) return false;
				// A status command can yield while the checkout is replaced or
				// policy changes. Recheck identity after it, then consult policy
				// after one final status: nonforce git still allows ignored files,
				// so do not leave metadata I/O between that check and removal.
				if (
					(await token(repoPath, wt)) !== expected ||
					!(await clean()) ||
					!stillAllowed()
				)
					return false;
				const result = await exec(
					"git",
					["worktree", "remove", "--", wt.path],
					{ cwd: repoPath },
				);
				return result.exitCode === 0;
			} catch {
				return false;
			}
		},
	};
}
