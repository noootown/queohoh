import { mkdirSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { DiscussStore } from "../discuss/store.js";

/** Real temp worktree path so realpathSync in indexKey succeeds. */
function makeWorktree(): string {
	const wt = mkdtempSync(join(tmpdir(), "discuss-wt-"));
	mkdirSync(wt, { recursive: true });
	return wt;
}

describe("DiscussStore", () => {
	it("ensure is idempotent for same worktree+provider", () => {
		const dir = mkdtempSync(join(tmpdir(), "discuss-"));
		const wt = makeWorktree();
		const store = new DiscussStore(dir);
		const a = store.ensure(wt, "grok");
		const b = store.ensure(wt, "grok");
		expect(a.sessionId).toBe(b.sessionId);
		expect(a.provider).toBe("grok");
		expect(a.status).toBe("idle");
	});

	it("different providers get different sessions", () => {
		const dir = mkdtempSync(join(tmpdir(), "discuss-"));
		const wt = makeWorktree();
		const store = new DiscussStore(dir);
		const g = store.ensure(wt, "grok");
		const c = store.ensure(wt, "claude");
		expect(g.sessionId).not.toBe(c.sessionId);
	});

	it("reset repoints index and leaves old session dir", () => {
		const dir = mkdtempSync(join(tmpdir(), "discuss-"));
		const wt = makeWorktree();
		const store = new DiscussStore(dir);
		const old = store.ensure(wt, "grok");
		store.appendTranscript(old.sessionId, "old history\n");
		const neu = store.reset(wt, "grok");
		expect(neu.sessionId).not.toBe(old.sessionId);
		expect(store.get(old.sessionId)?.sessionId).toBe(old.sessionId);
		expect(readFileSync(store.transcriptPath(old.sessionId), "utf-8")).toContain(
			"old history",
		);
		expect(store.ensure(wt, "grok").sessionId).toBe(neu.sessionId);
	});

	it("readTranscript respects byte cursor", () => {
		const dir = mkdtempSync(join(tmpdir(), "discuss-"));
		const wt = makeWorktree();
		const store = new DiscussStore(dir);
		const s = store.ensure(wt, "grok");
		store.appendTranscript(s.sessionId, "hello\nworld\n");
		const first = store.readTranscript(s.sessionId, 0);
		expect(first.text).toBe("hello\nworld\n");
		const mid = store.readTranscript(s.sessionId, "hello\n".length);
		expect(mid.text).toBe("world\n");
		expect(mid.nextCursor).toBe("hello\nworld\n".length);
	});
});
