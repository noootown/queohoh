import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";

export interface MainSessionEntry {
	sessionId: string;
	updatedAt: string;
}

// Legacy bare-string entries get the epoch so an old pointer never outranks
// a task's pinned session (worker compares updatedAt > task.created).
const LEGACY_UPDATED_AT = "1970-01-01T00:00:00.000Z";

function parseEntry(value: unknown): MainSessionEntry | null {
	if (typeof value === "string") {
		return { sessionId: value, updatedAt: LEGACY_UPDATED_AT };
	}
	if (value !== null && typeof value === "object") {
		const v = value as Record<string, unknown>;
		if (typeof v.sessionId === "string" && typeof v.updatedAt === "string") {
			return { sessionId: v.sessionId, updatedAt: v.updatedAt };
		}
	}
	return null;
}

export class MainSessionStore {
	private sessions: Record<string, MainSessionEntry> = Object.create(null);

	constructor(readonly filePath: string) {
		if (existsSync(filePath)) {
			try {
				const parsed = JSON.parse(readFileSync(filePath, "utf-8"));
				if (parsed && typeof parsed.sessions === "object" && parsed.sessions) {
					for (const [lane, value] of Object.entries(parsed.sessions)) {
						const entry = parseEntry(value);
						if (entry) this.sessions[lane] = entry;
					}
				}
			} catch {
				this.sessions = Object.create(null);
			}
		}
	}

	private persist(): void {
		const tmp = `${this.filePath}.tmp`;
		writeFileSync(tmp, JSON.stringify({ sessions: this.sessions }, null, 2));
		renameSync(tmp, this.filePath);
	}

	get(lane: string): string | null {
		return this.sessions[lane]?.sessionId ?? null;
	}

	entry(lane: string): MainSessionEntry | null {
		return this.sessions[lane] ?? null;
	}

	set(lane: string, sessionId: string): void {
		this.sessions[lane] = { sessionId, updatedAt: new Date().toISOString() };
		this.persist();
	}

	/** lane -> sessionId snapshot; timestamps omitted (TUI/API shape). */
	all(): Record<string, string> {
		return Object.fromEntries(
			Object.entries(this.sessions).map(([lane, e]) => [lane, e.sessionId]),
		);
	}
}
