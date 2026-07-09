import { describe, expect, it } from "vitest";
import type { LiveState } from "../scheduler.js";
import { schedule } from "../scheduler.js";
import type { Priority, TaskInstance, TaskStatus } from "../task.js";

let seq = 0;
function task(overrides: {
	status?: TaskStatus;
	priority?: Priority;
	worktree?: string | null;
	repo?: string;
}): TaskInstance {
	seq += 1;
	return {
		id: `01TEST${String(seq).padStart(20, "0")}`,
		status: overrides.status ?? "queued",
		definition: null,
		item: null,
		itemKey: null,
		target: {
			repo: overrides.repo ?? "platform",
			ref: "temp",
			worktree: overrides.worktree === undefined ? "wt-a" : overrides.worktree,
		},
		priority: overrides.priority ?? "normal",
		created: "2026-07-08T00:00:00.000Z",
		source: "tui",
		ephemeralWorktree: false,
		error: null,
		session: "fresh",
		prompt: "p",
	};
}

const idle: LiveState = {
	runningLanes: new Set(),
	interactiveLanes: new Set(),
	runningCount: 0,
};

describe("schedule", () => {
	it("starts a queued resolved task on a free lane", () => {
		const t = task({});
		expect(schedule([t], idle, { maxConcurrent: 3 })).toEqual({
			start: [t],
			resolve: [],
		});
	});

	it("ignores non-queued statuses", () => {
		const tasks = (["needs-input", "running", "done"] as const).map((status) =>
			task({ status, worktree: `wt-${status}` }),
		);
		expect(schedule(tasks, idle, { maxConcurrent: 5 }).start).toEqual([]);
	});

	it("orders by priority band then id", () => {
		const low = task({ priority: "low", worktree: "wt-1" });
		const high = task({ priority: "high", worktree: "wt-2" });
		const normal = task({ priority: "normal", worktree: "wt-3" });
		const { start } = schedule([low, high, normal], idle, { maxConcurrent: 3 });
		expect(start.map((t) => t.id)).toEqual([high.id, normal.id, low.id]);
	});

	it("skips lanes that are running or interactive", () => {
		const a = task({ worktree: "busy" });
		const b = task({ worktree: "yours" });
		const live: LiveState = {
			runningLanes: new Set(["platform:busy"]),
			interactiveLanes: new Set(["platform:yours"]),
			runningCount: 1,
		};
		expect(schedule([a, b], live, { maxConcurrent: 5 }).start).toEqual([]);
	});

	it("pauses a lane containing a failed task", () => {
		const failed = task({ status: "failed", worktree: "wt-a" });
		const queued = task({ worktree: "wt-a" });
		expect(
			schedule([failed, queued], idle, { maxConcurrent: 3 }).start,
		).toEqual([]);
	});

	it("enforces the global cap across start + resolve + running", () => {
		const a = task({ worktree: "wt-1" });
		const b = task({ worktree: null });
		const c = task({ worktree: "wt-3" });
		const live: LiveState = { ...idle, runningCount: 1 };
		const decision = schedule([a, b, c], live, { maxConcurrent: 2 });
		expect(decision.start).toEqual([a]);
		expect(decision.resolve).toEqual([]);
	});

	it("routes unresolved tasks to resolve", () => {
		const t = task({ worktree: null });
		expect(schedule([t], idle, { maxConcurrent: 3 })).toEqual({
			start: [],
			resolve: [t],
		});
	});

	it("starts at most one task per lane per decision", () => {
		const first = task({ worktree: "wt-a" });
		const second = task({ worktree: "wt-a" });
		const { start } = schedule([first, second], idle, { maxConcurrent: 5 });
		expect(start).toEqual([first]);
	});

	it("treats the @repo sentinel as an ordinary lane (serializes primary-checkout tasks)", () => {
		const first = task({ worktree: "@repo" });
		const second = task({ worktree: "@repo" });
		const { start } = schedule([first, second], idle, { maxConcurrent: 5 });
		// Both share the `platform:@repo` lane, so only one starts this decision.
		expect(start).toEqual([first]);
	});
});
