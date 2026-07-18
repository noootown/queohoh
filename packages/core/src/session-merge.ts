/**
 * One row in the worktree session picker: a session id tagged with its
 * originating provider, a recency timestamp, and a display label — merged
 * from whichever source(s) know about it (see `mergeSessionSources`).
 */
export interface SessionRow {
	sessionId: string;
	mtimeMs: number;
	provider: string;
	label: string;
	model?: string;
}

/**
 * Union two session sources into one deduped, recency-sorted, per-provider-
 * capped list for the worktree session picker (design spec: list sessions
 * from ALL providers, not just claude).
 *
 * `diskRows` come from Claude Code's own on-disk transcripts
 * (`listClaudeSessions`) — the ONLY way to see a claude session started
 * OUTSIDE the daemon (a manual `claude` run in that worktree).
 *
 * `runStoreRows` come from the daemon's own run store — every provider
 * (claude, codex, grok, ...) the daemon has launched a run for, in this
 * worktree.
 *
 * Rows are unioned by `sessionId`. On a conflict (the same session recorded
 * by both sources — e.g. a daemon-launched claude session that also has an
 * on-disk transcript) the LATER-processed row's metadata (provider/model/
 * label) wins: `runStoreRows` is processed after `diskRows`, so the daemon's
 * own bookkeeping is preferred over the on-disk scrape. `mtimeMs` always
 * keeps whichever value is larger, so a session never appears staler than
 * either source reports.
 *
 * The deduped set is grouped by provider, each group capped to the
 * `perProviderLimit` most recent sessions, and the survivors from every
 * provider are merged back into one list sorted by recency — interleaved
 * across providers, not grouped by provider.
 */
export function mergeSessionSources(
	diskRows: SessionRow[],
	runStoreRows: SessionRow[],
	perProviderLimit: number,
): SessionRow[] {
	const bySessionId = new Map<string, SessionRow>();
	const upsert = (row: SessionRow): void => {
		const existing = bySessionId.get(row.sessionId);
		bySessionId.set(
			row.sessionId,
			existing === undefined
				? row
				: { ...row, mtimeMs: Math.max(existing.mtimeMs, row.mtimeMs) },
		);
	};
	for (const row of diskRows) upsert(row);
	for (const row of runStoreRows) upsert(row);

	const byProvider = new Map<string, SessionRow[]>();
	for (const row of bySessionId.values()) {
		const list = byProvider.get(row.provider);
		if (list) list.push(row);
		else byProvider.set(row.provider, [row]);
	}

	const capped: SessionRow[] = [];
	for (const list of byProvider.values()) {
		list.sort((a, b) => b.mtimeMs - a.mtimeMs);
		capped.push(...list.slice(0, perProviderLimit));
	}
	capped.sort((a, b) => b.mtimeMs - a.mtimeMs);
	return capped;
}
