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

	it("stamps finishedAt when a task transitions to done or failed", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "x",
			repo: "r",
			ref: "temp",
			source: "tui",
		});
		expect(t.finishedAt).toBeNull();

		const running = store.update(t.id, { status: "running" });
		expect(running.finishedAt).toBeNull();

		const done = store.update(t.id, { status: "done" });
		expect(done.finishedAt).toMatch(/^\d{4}-\d{2}-\d{2}T.*Z$/);
		expect(store.get(t.id)?.finishedAt).toBe(done.finishedAt);

		const failed = store.update(
			store.create({ prompt: "y", repo: "r", ref: "temp", source: "tui" }).id,
			{ status: "failed", error: "boom" },
		);
		expect(failed.finishedAt).toMatch(/^\d{4}-\d{2}-\d{2}T.*Z$/);
	});

	it("stamps finishedAt for the cancelled and skipped terminal statuses too", () => {
		const store = freshStore();
		const cancelled = store.update(
			store.create({ prompt: "a", repo: "r", ref: "temp", source: "tui" }).id,
			{ status: "cancelled", error: "cancelled by user" },
		);
		expect(cancelled.finishedAt).toMatch(/^\d{4}-\d{2}-\d{2}T.*Z$/);
		const skipped = store.update(
			store.create({ prompt: "b", repo: "r", ref: "temp", source: "tui" }).id,
			{ status: "skipped", error: "skipped: chain predecessor failed" },
		);
		expect(skipped.finishedAt).toMatch(/^\d{4}-\d{2}-\d{2}T.*Z$/);
	});

	it("keeps finishedAt stable across a re-set of the same terminal status", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "x",
			repo: "r",
			ref: "temp",
			source: "tui",
		});
		const first = store.update(t.id, { status: "failed", error: "a" });
		const second = store.update(t.id, { status: "failed", error: "b" });
		expect(second.finishedAt).toBe(first.finishedAt);
	});

	it("clears finishedAt when a finished task is re-run", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "x",
			repo: "r",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, { status: "done" });
		const rerun = store.update(t.id, { status: "running" });
		expect(rerun.finishedAt).toBeNull();
	});

	it("leaves finishedAt untouched on a non-status patch", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "x",
			repo: "r",
			ref: "temp",
			source: "tui",
		});
		const done = store.update(t.id, { status: "done" });
		const patched = store.update(t.id, {
			target: { ...t.target, worktree: "wt-x" },
		});
		expect(patched.finishedAt).toBe(done.finishedAt);
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

	it("createChain links members with a shared chainId, ascending chainSeq, one head", () => {
		const store = freshStore();
		const members = store.createChain(
			[
				{ prompt: "autofix\n", definition: "platform/autofix" },
				{ prompt: "pr-ready full\n" },
			],
			{
				repo: "platform",
				ref: "temp",
				source: "mcp",
				priority: "high",
				resumeSessionId: "sess-1",
			},
		);
		expect(members).toHaveLength(2);
		const [head, tail] = members;
		// Shared chain id, ascending 0-based seq.
		expect(head?.chainId).toBeTruthy();
		expect(head?.chainId).toBe(tail?.chainId);
		expect(head?.chainSeq).toBe(0);
		expect(tail?.chainSeq).toBe(1);
		// Same target + priority; both start queued, unresolved.
		expect(members.map((m) => m.target)).toEqual([
			{ repo: "platform", ref: "temp", worktree: null },
			{ repo: "platform", ref: "temp", worktree: null },
		]);
		expect(members.every((m) => m.status === "queued")).toBe(true);
		expect(members.every((m) => m.priority === "high")).toBe(true);
		// resume applies to the head only; the tail is always fresh.
		expect(head?.resumeSessionId).toBe("sess-1");
		expect(tail?.resumeSessionId).toBeNull();
		// Step fields carried through; ids ascend in creation order.
		expect(head?.definition).toBe("platform/autofix");
		expect(tail?.definition).toBeNull();
		const ids = members.map((m) => m.id);
		expect(ids).toEqual([...ids].sort());
		// Persisted and reloadable with the chain fields intact.
		expect(store.get(tail?.id ?? "")?.chainSeq).toBe(1);
	});

	it("create leaves chainId and chainSeq null for a standalone task", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "x",
			repo: "r",
			ref: "temp",
			source: "tui",
		});
		expect(t.chainId).toBeNull();
		expect(t.chainSeq).toBeNull();
	});

	it("persists a lane override through create and reload", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "x",
			repo: "platform",
			ref: "temp",
			source: "mcp",
			lane: "testing1-stack",
		});
		expect(t.lane).toBe("testing1-stack");
		expect(store.get(t.id)?.lane).toBe("testing1-stack");
		const plain = store.create({
			prompt: "y",
			repo: "platform",
			ref: "temp",
			source: "mcp",
		});
		expect(plain.lane).toBeNull();
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
