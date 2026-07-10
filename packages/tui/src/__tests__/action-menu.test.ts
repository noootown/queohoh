import { describe, expect, it } from "vitest";
import { buildActions, buildBulkActions } from "../action-menu.js";

const ids = (items: { id: string }[]) => items.map((i) => i.id);
const enabled = (items: { id: string; disabled?: string }[]) =>
	items.filter((i) => i.disabled === undefined).map((i) => i.id);

describe("buildActions — queue context", () => {
	it("always lists rerun/skip/assign-worktree in stable order", () => {
		const items = buildActions({
			kind: "queue",
			status: "running",
			archived: false,
		});
		expect(ids(items)).toEqual(["rerun", "skip", "assign-worktree"]);
	});

	it("failed task: rerun+skip enabled, assign-worktree disabled", () => {
		const items = buildActions({
			kind: "queue",
			status: "failed",
			archived: false,
		});
		expect(enabled(items)).toEqual(["rerun", "skip"]);
	});

	it("needs-input task: everything enabled", () => {
		const items = buildActions({
			kind: "queue",
			status: "needs-input",
			archived: false,
		});
		expect(enabled(items)).toEqual(["rerun", "skip", "assign-worktree"]);
	});

	it("done task: only skip enabled", () => {
		const items = buildActions({
			kind: "queue",
			status: "done",
			archived: false,
		});
		expect(enabled(items)).toEqual(["skip"]);
	});

	it("running/queued task: nothing enabled", () => {
		for (const status of ["running", "queued"] as const) {
			expect(
				enabled(buildActions({ kind: "queue", status, archived: false })),
			).toEqual([]);
		}
	});

	it("archived row: all disabled with 'archived' reason", () => {
		const items = buildActions({
			kind: "queue",
			status: "done",
			archived: true,
		});
		expect(items.every((i) => i.disabled === "archived")).toBe(true);
	});
});

describe("buildActions — task context", () => {
	it("offers Run", () => {
		expect(buildActions({ kind: "task" })).toEqual([
			{ id: "run", label: "Run" },
		]);
	});
});

describe("buildActions — worktree context", () => {
	it("lists the seven worktree actions in order (squash above remove)", () => {
		const items = buildActions({
			kind: "worktree",
			busy: false,
			insideTmux: true,
			hasBranch: true,
		});
		expect(ids(items)).toEqual([
			"task-fresh",
			"task-main",
			"run-def",
			"tmux-open",
			"squash-merge",
			"remove-worktree",
			"create-worktree",
		]);
		expect(enabled(items)).toEqual(ids(items));
	});

	it("keeps Create worktree… enabled even on a busy worktree", () => {
		const items = buildActions({
			kind: "worktree",
			busy: true,
			insideTmux: true,
			hasBranch: true,
		});
		const create = items.find((i) => i.id === "create-worktree");
		expect(create).toEqual({
			id: "create-worktree",
			label: "Create worktree…",
		});
	});

	it("busy worktree disables remove and squash-merge", () => {
		const items = buildActions({
			kind: "worktree",
			busy: true,
			insideTmux: true,
			hasBranch: true,
		});
		const remove = items.find((i) => i.id === "remove-worktree");
		expect(remove?.disabled).toBe("a task is running here");
		const squash = items.find((i) => i.id === "squash-merge");
		expect(squash?.disabled).toBe("a task is running here");
	});

	it("branchless worktree disables squash-merge with a branch reason", () => {
		const items = buildActions({
			kind: "worktree",
			busy: false,
			insideTmux: true,
			hasBranch: false,
		});
		const squash = items.find((i) => i.id === "squash-merge");
		expect(squash).toEqual({
			id: "squash-merge",
			label: "Squash merge into…",
			disabled: "worktree has no branch",
		});
		// remove stays enabled — it only cares about busy, not branch.
		const remove = items.find((i) => i.id === "remove-worktree");
		expect(remove?.disabled).toBeUndefined();
	});

	it("outside tmux disables tmux-open", () => {
		const items = buildActions({
			kind: "worktree",
			busy: false,
			insideTmux: false,
			hasBranch: true,
		});
		const open = items.find((i) => i.id === "tmux-open");
		expect(open?.disabled).toBe("not inside tmux");
	});
});

describe("buildActions — session context", () => {
	it("offers only tmux-open", () => {
		expect(buildActions({ kind: "session", insideTmux: true })).toEqual([
			{ id: "tmux-open", label: "Open in tmux window" },
		]);
	});
});

describe("buildBulkActions", () => {
	it("bulk-queue: rerun and skip with eligible-of-total labels", () => {
		expect(
			buildBulkActions({ kind: "bulk-queue", rerun: 2, skip: 3, total: 5 }),
		).toEqual([
			{ id: "rerun", label: "Rerun (2 of 5)" },
			{ id: "skip", label: "Skip (3 of 5)" },
		]);
	});

	it("disables an action with zero eligible rows", () => {
		expect(
			buildBulkActions({ kind: "bulk-queue", rerun: 0, skip: 1, total: 4 }),
		).toEqual([
			{ id: "rerun", label: "Rerun (0 of 4)", disabled: "no eligible rows" },
			{ id: "skip", label: "Skip (1 of 4)" },
		]);
	});

	it("bulk-tasks: run only", () => {
		expect(buildBulkActions({ kind: "bulk-tasks", run: 1, total: 3 })).toEqual([
			{ id: "run", label: "Run (1 of 3)" },
		]);
	});

	it("bulk-worktrees: remove only", () => {
		expect(
			buildBulkActions({ kind: "bulk-worktrees", remove: 2, total: 4 }),
		).toEqual([{ id: "remove-worktree", label: "Remove worktrees… (2 of 4)" }]);
	});
});
