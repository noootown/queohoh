import {
	appendFileSync,
	existsSync,
	mkdirSync,
	readFileSync,
	realpathSync,
	renameSync,
	writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { ulid } from "ulid";
import type { DiscussMeta, DiscussStatus } from "./types.js";

/** worktree\0provider → sessionId. Null-byte key avoids path/provider collision. */
type Index = Record<string, string>;

/**
 * Canonical index key for a (worktree, provider) pair.
 * realpathSync so symlink aliases collapse to one session; caller must pass
 * an existing path (tests mkdir the temp worktree first).
 */
function indexKey(worktree: string, provider: string): string {
	const abs = realpathSync(worktree);
	return `${abs}\0${provider}`;
}

/**
 * On-disk store for reserved review (discuss) sessions.
 *
 * Layout under `discussDir`:
 *   index.json                 — worktree\0provider → sessionId
 *   sessions/<id>/meta.json    — DiscussMeta (atomic tmp+rename)
 *   sessions/<id>/transcript.md
 *   sessions/<id>/turns/<turnId>/
 *
 * Index repoint on reset leaves old session dirs intact (history preserved).
 */
export class DiscussStore {
	constructor(readonly discussDir: string) {
		mkdirSync(join(discussDir, "sessions"), { recursive: true });
	}

	private indexPath(): string {
		return join(this.discussDir, "index.json");
	}

	private loadIndex(): Index {
		if (!existsSync(this.indexPath())) return {};
		return JSON.parse(readFileSync(this.indexPath(), "utf-8")) as Index;
	}

	private saveIndex(idx: Index): void {
		const tmp = `${this.indexPath()}.tmp`;
		writeFileSync(tmp, JSON.stringify(idx, null, 2));
		renameSync(tmp, this.indexPath());
	}

	/** Path only — does not create directories (safe for get/missing lookups). */
	private metaPath(sessionId: string): string {
		return join(this.discussDir, "sessions", sessionId, "meta.json");
	}

	private writeMeta(meta: DiscussMeta): void {
		// Ensure sessions/<id> exists before the atomic write.
		this.sessionDir(meta.sessionId);
		const path = this.metaPath(meta.sessionId);
		const tmp = `${path}.tmp`;
		writeFileSync(tmp, JSON.stringify(meta, null, 2));
		renameSync(tmp, path);
	}

	private readMeta(sessionId: string): DiscussMeta | null {
		const path = this.metaPath(sessionId);
		if (!existsSync(path)) return null;
		try {
			return JSON.parse(readFileSync(path, "utf-8")) as DiscussMeta;
		} catch {
			return null;
		}
	}

	private now(): string {
		return new Date().toISOString();
	}

	private createSession(worktree: string, provider: string): DiscussMeta {
		const abs = realpathSync(worktree);
		const sessionId = ulid();
		const ts = this.now();
		const meta: DiscussMeta = {
			sessionId,
			worktree: abs,
			provider,
			status: "idle",
			lineageRoot: null,
			createdAt: ts,
			updatedAt: ts,
			lastError: null,
			activeTurnId: null,
		};
		// sessionDir side-effect: ensure sessions/<id> exists before write
		this.writeMeta(meta);
		return meta;
	}

	sessionDir(sessionId: string): string {
		const d = join(this.discussDir, "sessions", sessionId);
		mkdirSync(d, { recursive: true });
		return d;
	}

	transcriptPath(sessionId: string): string {
		return join(this.sessionDir(sessionId), "transcript.md");
	}

	turnDir(sessionId: string, turnId: string): string {
		const d = join(this.sessionDir(sessionId), "turns", turnId);
		mkdirSync(d, { recursive: true });
		return d;
	}

	/**
	 * Return the existing session for (worktree, provider), or mint a new one.
	 * Idempotent for the same pair.
	 */
	ensure(worktree: string, provider: string): DiscussMeta {
		const key = indexKey(worktree, provider);
		const idx = this.loadIndex();
		const existing = idx[key];
		if (existing) {
			const meta = this.readMeta(existing);
			if (meta) return meta;
			// Index points at a missing/corrupt meta — re-mint.
		}
		const meta = this.createSession(worktree, provider);
		idx[key] = meta.sessionId;
		this.saveIndex(idx);
		return meta;
	}

	get(sessionId: string): DiscussMeta | null {
		return this.readMeta(sessionId);
	}

	setStatus(
		sessionId: string,
		status: DiscussStatus,
		lastError: string | null = null,
	): DiscussMeta | null {
		const meta = this.readMeta(sessionId);
		if (!meta) return null;
		meta.status = status;
		meta.lastError = lastError;
		meta.updatedAt = this.now();
		this.writeMeta(meta);
		return meta;
	}

	setLineageRoot(
		sessionId: string,
		lineageRoot: string | null,
	): DiscussMeta | null {
		const meta = this.readMeta(sessionId);
		if (!meta) return null;
		meta.lineageRoot = lineageRoot;
		meta.updatedAt = this.now();
		this.writeMeta(meta);
		return meta;
	}

	setActiveTurn(
		sessionId: string,
		activeTurnId: string | null,
	): DiscussMeta | null {
		const meta = this.readMeta(sessionId);
		if (!meta) return null;
		meta.activeTurnId = activeTurnId;
		meta.updatedAt = this.now();
		this.writeMeta(meta);
		return meta;
	}

	/**
	 * Mint a fresh session for (worktree, provider), repoint the index, leave
	 * the old session dir (and transcript) on disk for history.
	 */
	reset(worktree: string, provider: string): DiscussMeta {
		const key = indexKey(worktree, provider);
		const meta = this.createSession(worktree, provider);
		const idx = this.loadIndex();
		idx[key] = meta.sessionId;
		this.saveIndex(idx);
		return meta;
	}

	appendTranscript(sessionId: string, text: string): void {
		// Ensure session dir exists even if meta was never written.
		appendFileSync(this.transcriptPath(sessionId), text, "utf-8");
	}

	/**
	 * Incremental transcript read from a byte offset. `nextCursor` is the
	 * absolute end offset so clients can poll without re-reading prior bytes.
	 */
	readTranscript(
		sessionId: string,
		cursorBytes: number,
	): { text: string; nextCursor: number } {
		const path = this.transcriptPath(sessionId);
		if (!existsSync(path)) {
			return { text: "", nextCursor: cursorBytes };
		}
		const buf = readFileSync(path);
		const slice = buf.subarray(Math.max(0, cursorBytes));
		return {
			text: slice.toString("utf-8"),
			nextCursor: buf.length,
		};
	}
}
