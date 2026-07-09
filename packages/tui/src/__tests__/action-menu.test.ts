import { describe, expect, it } from "vitest";
import { buildActions } from "../action-menu.js";

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
	it("lists the five worktree actions in order", () => {
		const items = buildActions({
			kind: "worktree",
			busy: false,
			insideTmux: true,
		});
		expect(ids(items)).toEqual([
			"task-fresh",
			"task-main",
			"run-def",
			"tmux-open",
			"remove-worktree",
		]);
		expect(enabled(items)).toEqual(ids(items));
	});

	it("busy worktree disables remove", () => {
		const items = buildActions({
			kind: "worktree",
			busy: true,
			insideTmux: true,
		});
		const remove = items.find((i) => i.id === "remove-worktree");
		expect(remove?.disabled).toBe("a task is running here");
	});

	it("outside tmux disables tmux-open", () => {
		const items = buildActions({
			kind: "worktree",
			busy: false,
			insideTmux: false,
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
