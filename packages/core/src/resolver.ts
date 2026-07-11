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
