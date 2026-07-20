/** Lifecycle of a reserved review (discuss) session. */
export type DiscussStatus = "idle" | "running" | "error";

/**
 * A code-anchor pointing into an old/new side of a review diff.
 * Side + line are load-bearing for the juice review UI; keep fields stable.
 */
export type DiscussAnchor = {
	path: string;
	side: "old" | "new";
	line: number;
	snippet?: string;
};

/**
 * On-disk meta for one discuss session (`sessions/<id>/meta.json`).
 * Reserved review sessions live here (not TaskStore) so the daemon can
 * own long-lived chat state independent of the queue.
 */
export type DiscussMeta = {
	sessionId: string;
	worktree: string;
	provider: string;
	status: DiscussStatus;
	/** Provider lineage root session id, once the first turn mints one. */
	lineageRoot: string | null;
	createdAt: string;
	updatedAt: string;
	lastError: string | null;
	/** In-flight turn dir name under `turns/`, if any. */
	activeTurnId: string | null;
};
