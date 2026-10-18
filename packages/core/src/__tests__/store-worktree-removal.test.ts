import { mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { QueueStore } from "../store.js";

const dirs: string[] = [];
afterEach(() => {
	for (const dir of dirs.splice(0))
		rmSync(dir, { recursive: true, force: true });
});
function freshStore() {
	const dir = mkdtempSync(join(tmpdir(), "qoo-removal-store-"));
	dirs.push(dir);
	return new QueueStore(dir);
}
const input = {
	repo: "r",
	ref: "worktree:feature",
	source: "tui" as const,
	prompt: "test",
};
const names = new Set(["feature", "r.feature"]);

describe("worktree removal reservation", () => {
	it.each([
		"ticket:TICK-77",
		"TICK-77",
		"https://linear.app/team/issue/TICK-77/fix",
		" worktree:feature ",
		"pr:9",
		"#9",
		"https://github.com/org/repo/pull/9",
		"temp",
	])("guards resolver-equivalent or unresolved refs: %s", (ref) => {
		const store = freshStore();
		const release = store.reserveWorktreeRemoval(
			"r",
			new Set([...names, "TICK-77"]),
		);
		expect(() => store.create({ ...input, ref })).toThrow(
			/cleanup in progress/,
		);
		expect(() => store.create({ ...input, ref, repo: "other" })).not.toThrow();
		release();
		expect(() => store.create({ ...input, ref })).not.toThrow();
	});
	it("allows tasks already resolved to a different lane even if their original ref was a PR", () => {
		const store = freshStore();
		const task = store.create({ ...input, ref: "pr:9" });
		store.update(task.id, { target: { ...task.target, worktree: "other" } });
		store.reserveWorktreeRemoval("r", names);
		expect(() => store.update(task.id, { priority: "high" })).not.toThrow();
	});
	it("reports unreadable archived history so cleanup cannot overlook a favorite", () => {
		const store = freshStore();
		const path = join(store.archiveDir, "broken.md");
		writeFileSync(path, "not a task");
		expect(store.listArchived()).toEqual([]);
		expect(store.invalidArchiveFiles).toEqual([path]);
		rmSync(path);
		store.reload();
		store.listArchived();
		expect(store.invalidArchiveFiles).toEqual([]);
	});
	it("rejects both aliases without creating task files, but allows other lanes", () => {
		const store = freshStore();
		const release = store.reserveWorktreeRemoval("r", names);
		for (const name of names) {
			expect(() => store.create({ ...input, ref: `worktree:${name}` })).toThrow(
				/cleanup in progress/,
			);
		}
		expect(readdirSync(store.tasksDir)).toEqual([]);
		expect(() => store.create({ ...input, repo: "other" })).not.toThrow();
		expect(() =>
			store.create({ ...input, ref: "worktree:other" }),
		).not.toThrow();
		release();
		expect(() => store.create(input)).not.toThrow();
	});
	it("rejects requeue and target resolution without changing the stored task", () => {
		const store = freshStore();
		const task = store.create({ ...input, ref: "temp" });
		store.reserveWorktreeRemoval("r", names);
		expect(() =>
			store.update(task.id, {
				target: { ...task.target, worktree: "r.feature" },
			}),
		).toThrow(/cleanup in progress/);
		expect(store.get(task.id)).toEqual(task);
	});
	it("does not partially enqueue a chain into a reserved lane", () => {
		const store = freshStore();
		store.reserveWorktreeRemoval("r", names);
		expect(() =>
			store.createChain([{ prompt: "a" }, { prompt: "b" }], input),
		).toThrow(/cleanup in progress/);
		expect(readdirSync(store.tasksDir)).toEqual([]);
	});
	it("blocks archived mutations and restoring tasks until removal finishes", () => {
		const store = freshStore();
		const task = store.create(input);
		store.update(task.id, { status: "done" });
		store.archive(task.id);
		const release = store.reserveWorktreeRemoval("r", names);
		expect(() => store.updateAny(task.id, { favorite: true })).toThrow(
			/cleanup in progress/,
		);
		expect(() => store.unarchive(task.id)).toThrow(/cleanup in progress/);
		expect(store.list()).toEqual([]);
		expect(store.getAny(task.id)?.favorite).toBe(false);
		release();
		expect(() => store.unarchive(task.id)).not.toThrow();
	});
	it("overlapping reservations fail atomically and old releases cannot unlock a new reservation", () => {
		const store = freshStore();
		const release = store.reserveWorktreeRemoval("r", names);
		expect(() =>
			store.reserveWorktreeRemoval("r", new Set(["other", "feature"])),
		).toThrow(/cleanup in progress/);
		expect(() =>
			store.create({ ...input, ref: "worktree:other" }),
		).not.toThrow();
		release();
		const releaseNext = store.reserveWorktreeRemoval("r", names);
		release();
		expect(() => store.create(input)).toThrow(/cleanup in progress/);
		releaseNext();
		expect(() => store.create(input)).not.toThrow();
	});
});
