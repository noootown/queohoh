import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";

// Maps a resumed (parent) session id to the session id its run produced.
// Headless `claude -p --resume X` mints a NEW session id Y for the run, so a
// queued follow-up pinned to X must actually resume Y to see the earlier
// run's conversation. Following parent→child links resolves any pin to the
// tip of its own chain — unlike the old per-lane pointer, a task pinned to a
// different session in the same lane can never be hijacked onto this chain.
export class SessionLineageStore {
	private forks: Record<string, string> = Object.create(null);

	constructor(readonly filePath: string) {
		if (existsSync(filePath)) {
			try {
				const parsed = JSON.parse(readFileSync(filePath, "utf-8"));
				if (parsed && typeof parsed.forks === "object" && parsed.forks) {
					for (const [parent, child] of Object.entries(parsed.forks)) {
						if (typeof child === "string") this.forks[parent] = child;
					}
				}
			} catch {
				this.forks = Object.create(null);
			}
		}
	}

	private persist(): void {
		const tmp = `${this.filePath}.tmp`;
		writeFileSync(tmp, JSON.stringify({ forks: this.forks }, null, 2));
		renameSync(tmp, this.filePath);
	}

	recordFork(parent: string, child: string): void {
		if (parent === child) return;
		this.forks[parent] = child;
		this.persist();
	}

	/** Newest descendant of `sessionId` (itself when no fork recorded). */
	tip(sessionId: string): string {
		let current = sessionId;
		const seen = new Set<string>([current]);
		for (;;) {
			const next = this.forks[current];
			if (next === undefined || seen.has(next)) return current;
			seen.add(next);
			current = next;
		}
	}
}
