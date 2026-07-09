import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";

export class MainSessionStore {
	private sessions: Record<string, string> = Object.create(null);

	constructor(readonly filePath: string) {
		if (existsSync(filePath)) {
			try {
				const parsed = JSON.parse(readFileSync(filePath, "utf-8"));
				if (parsed && typeof parsed.sessions === "object" && parsed.sessions) {
					this.sessions = Object.assign(Object.create(null), parsed.sessions);
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
		return this.sessions[lane] ?? null;
	}

	set(lane: string, sessionId: string): void {
		this.sessions[lane] = sessionId;
		this.persist();
	}

	all(): Record<string, string> {
		return { ...this.sessions };
	}
}
