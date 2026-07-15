import type { TargetRef } from "./ref.js";
import { parseRef } from "./ref.js";
import { qooTempName } from "./slug.js";

/**
 * Sentinel worktree name for a `repo` ref (the project's primary checkout). The
 * engine's name→path lookup special-cases this → `config.projects[].path`; no
 * worktree by this name is ever spawned. The leading `@` keeps it distinct from
 * any real worktree name.
 */
export const REPO_SENTINEL = "@repo";

export interface WorktreeInfo {
	name: string;
	path: string;
	branch: string;
	/** Working tree has uncommitted changes (git status --porcelain non-empty). null = unknown. */
	dirty?: boolean | null;
	/** Worktree HEAD is an ancestor of the project's default branch (vars.yaml
	 * `default_branch`, fallback `main`) — its committed work has been merged
	 * back. null/absent = unknown, or the default-branch checkout itself.
	 * Drives the TUI's `↣` front-column marker. */
	merged?: boolean | null;
	/** Unix epoch SECONDS of the last commit (git log -1 --format=%ct). null = unknown. */
	lastCommitEpoch?: number | null;
	/** Author name of the last commit (git log -1 --format=%an). null = unknown. */
	lastCommitAuthor?: string | null;
	/** Author email of the last commit (git log -1 --format=%ae). null = unknown.
	 * The TUI matches this against the project's githubId (GitHub noreply emails
	 * embed the login) to sort "my" worktrees first. */
	lastCommitAuthorEmail?: string | null;
	/** Short hash of the last commit (git log -1 --format=%h). null = unknown. */
	lastCommitHash?: string | null;
	/** Open PR number for this worktree's branch (via `gh pr list`). null =
	 * unknown / no open PR / gh unavailable. */
	prNumber?: number | null;
	/** Web URL of that open PR (via `gh pr list`'s `url` field). null =
	 * unknown / no open PR / gh unavailable. Paired with prNumber so the TUI can
	 * open the PR in a browser on a click. */
	prUrl?: string | null;
	/** True when queohoh must never delete this worktree — the project's main
	 * checkout (path-equality) or a name in the project's `protected_worktrees`.
	 * Computed by the daemon and carried to the TUI. Absent/undefined = not
	 * protected (an old daemon that predates the field). */
	protected?: boolean;
}

/**
 * Whether `wt` is protected from deletion: it is the project's main checkout
 * (its path equals the project's registered checkout path) OR its name is in the
 * project's configured `protected_worktrees`. Path-equality — not name equality —
 * identifies the main checkout, because a project's name is a user label while a
 * worktree's name is `basename(path)`; the two can differ. `repoPath` is null for
 * an unknown repo, in which case only the name list applies.
 *
 * A `protected_worktrees` entry may be written either as the raw worktree name
 * (the directory basename, e.g. `platform.legal-lake`) or as the TUI's display
 * name with the `<repo>.` prefix stripped (`legal-lake`) — the same dual-form
 * convention `removeWorktree` accepts for its name lookup.
 */
export function isProtectedWorktree(
	repoPath: string | null,
	repo: string,
	protectedNames: string[],
	wt: WorktreeInfo,
): boolean {
	if (repoPath !== null && wt.path === repoPath) return true;
	return protectedNames.some(
		(n) => wt.name === n || wt.name === `${repo}.${n}`,
	);
}

export interface ResolverIO {
	listWorktrees(repoPath: string): Promise<WorktreeInfo[]>;
	prBranch(repoPath: string, number: number): Promise<string | null>;
	spawnWorktree(
		repoPath: string,
		name: string,
		branch?: string,
	): Promise<WorktreeInfo>;
	removeWorktree(repoPath: string, worktree: WorktreeInfo): Promise<void>;
}

export type Resolution =
	| { outcome: "resolved"; worktree: string; ephemeral: boolean }
	| { outcome: "needs-input"; reason: string };

function defaultTempName(): string {
	return qooTempName("");
}

export async function resolveTarget(
	rawRef: string,
	ctx: { repoPath: string; tempName?: () => string },
	io: ResolverIO,
): Promise<Resolution> {
	let ref: TargetRef;
	try {
		ref = parseRef(rawRef);
	} catch {
		return { outcome: "needs-input", reason: `unrecognized ref: ${rawRef}` };
	}

	switch (ref.kind) {
		case "worktree": {
			const existing = await io.listWorktrees(ctx.repoPath);
			const match = existing.find((w) => w.name === ref.name);
			if (match) {
				return { outcome: "resolved", worktree: match.name, ephemeral: false };
			}
			return {
				outcome: "needs-input",
				reason: `worktree not found: ${ref.name}`,
			};
		}
		case "pr": {
			const branch = await io.prBranch(ctx.repoPath, ref.number);
			if (branch === null) {
				return {
					outcome: "needs-input",
					reason: `PR not found: #${ref.number}`,
				};
			}
			const existing = await io.listWorktrees(ctx.repoPath);
			const byBranch = existing.find((w) => w.branch === branch);
			if (byBranch) {
				return {
					outcome: "resolved",
					worktree: byBranch.name,
					ephemeral: false,
				};
			}
			// A PR always has a branch, so the branch itself names the worktree.
			// Conventional branches (branch = ticket id) keep their old names;
			// off-convention ones (dependabot/…) resolve instead of parking as
			// needs-input. Only `/` needs folding — the one path-hostile character
			// a valid git branch name can contain. No truncation: a lossy cut
			// (slugify caps at 24) would collide two dependabot branches.
			const name = branch.replace(/\//g, "-");
			const byName = existing.find((w) => w.name === name);
			if (byName) {
				return { outcome: "resolved", worktree: byName.name, ephemeral: false };
			}
			const spawned = await io.spawnWorktree(ctx.repoPath, name, branch);
			return { outcome: "resolved", worktree: spawned.name, ephemeral: false };
		}
		case "ticket": {
			const existing = await io.listWorktrees(ctx.repoPath);
			const match = existing.find((w) => w.name === ref.id);
			if (match) {
				return { outcome: "resolved", worktree: match.name, ephemeral: false };
			}
			const spawned = await io.spawnWorktree(ctx.repoPath, ref.id);
			return { outcome: "resolved", worktree: spawned.name, ephemeral: false };
		}
		case "temp": {
			const name = (ctx.tempName ?? defaultTempName)();
			const spawned = await io.spawnWorktree(ctx.repoPath, name);
			return { outcome: "resolved", worktree: spawned.name, ephemeral: true };
		}
		case "repo": {
			// Primary checkout: never spawns, never ephemeral. The engine's
			// name→path lookup special-cases this sentinel → the project's path.
			return { outcome: "resolved", worktree: REPO_SENTINEL, ephemeral: false };
		}
	}
}
