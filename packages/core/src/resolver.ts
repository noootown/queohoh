import type { TargetRef } from "./ref.js";
import { extractTicketId, parseRef } from "./ref.js";
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
			const ticket = extractTicketId(branch);
			if (ticket === null) {
				return {
					outcome: "needs-input",
					reason: `no ticket id in branch: ${branch}`,
				};
			}
			const byName = existing.find((w) => w.name === ticket);
			if (byName) {
				return { outcome: "resolved", worktree: byName.name, ephemeral: false };
			}
			const spawned = await io.spawnWorktree(ctx.repoPath, ticket, branch);
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
