import { existsSync, mkdtempSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { QueueStore } from "../store.js";

function freshStore(): QueueStore {
	return new QueueStore(mkdtempSync(join(tmpdir(), "queohoh-store-")));
}

describe("QueueStore", () => {
	it("creates a queued task with generated id and lists it", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "fix the flaky test\n",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		expect(t.status).toBe("queued");
		expect(t.id).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);
		expect(store.list()).toEqual([t]);
		expect(store.get(t.id)).toEqual(t);
	});

	it("defaults session to fresh, and persists an explicit main session", () => {
		const store = freshStore();
		const fresh = store.create({
			prompt: "x",
			repo: "r",
			ref: "temp",
			source: "tui",
		});
		expect(fresh.session).toBe("fresh");
		expect(store.get(fresh.id)?.session).toBe("fresh");

		const main = store.create({
			prompt: "y",
			repo: "r",
			ref: "temp",
			source: "tui",
			session: "main",
		});
		expect(main.session).toBe("main");
		expect(store.get(main.id)?.session).toBe("main");
	});

	it("lists in creation order even for same-millisecond ids", () => {
		const store = freshStore();
		// Tight loop: several creations land in the same millisecond. The
		// monotonic ulid factory guarantees they still sort in creation order.
		const created = Array.from({ length: 3 }, (_, i) =>
			store.create({
				prompt: `t${i}`,
				repo: "r",
				ref: "temp",
				source: "tui",
			}),
		);
		const ids = created.map((t) => t.id);
		expect(store.list().map((t) => t.id)).toEqual(ids);
		// Ids are already sorted ascending — monotonicity, not luck.
		expect(ids).toEqual([...ids].sort());
	});

	it("update patches and persists atomically", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "x",
			repo: "r",
			ref: "temp",
			source: "mcp",
		});
		const updated = store.update(t.id, {
			status: "failed",
			error: "boom",
			target: { ...t.target, worktree: "tmp-x-1" },
		});
		expect(updated.status).toBe("failed");
		expect(store.get(t.id)?.error).toBe("boom");
		expect(store.get(t.id)?.target.worktree).toBe("tmp-x-1");
		// no stray tmp files left behind
		const dir = join(store.stateDir, "tasks");
		expect(readdirSync(dir).filter((f) => f.endsWith(".tmp"))).toEqual([]);
	});

	it("update throws for unknown id", () => {
		const store = freshStore();
		expect(() => store.update("01UNKNOWN0000000000000000X", {})).toThrow(
			/task not found/,
		);
	});

	it("archive moves the file out of tasks/", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "x",
			repo: "r",
			ref: "temp",
			source: "tui",
		});
		store.archive(t.id);
		expect(store.list()).toEqual([]);
		expect(existsSync(join(store.stateDir, "archive", `${t.id}.md`))).toBe(
			true,
		);
	});

	it("listArchived returns archived tasks", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "x",
			repo: "r",
			ref: "temp",
			source: "tui",
		});
		store.archive(t.id);
		expect(store.listArchived().map((a) => a.id)).toEqual([t.id]);
	});

	it("create persists resumeSessionId and model", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "mcp",
			resumeSessionId: "sess-1",
			model: "claude-fable-5",
		});
		const reloaded = store.get(t.id);
		expect(reloaded?.resumeSessionId).toBe("sess-1");
		expect(reloaded?.model).toBe("claude-fable-5");
	});

	it("create defaults resumeSessionId and model to null", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "mcp",
		});
		expect(t.resumeSessionId).toBeNull();
		expect(t.model).toBeNull();
	});

	it("skips unparseable files and reports them", () => {
		const store = freshStore();
		store.create({ prompt: "good", repo: "r", ref: "temp", source: "tui" });
		writeFileSync(join(store.stateDir, "tasks", "junk.md"), "not a task");
		expect(store.list()).toHaveLength(1);
		expect(store.invalidFiles).toEqual([
			join(store.stateDir, "tasks", "junk.md"),
		]);
	});
});
