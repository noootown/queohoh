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
	id?: string;
	chainId?: string | null;
	chainSeq?: number | null;
}): TaskInstance {
	seq += 1;
	return {
		id: overrides.id ?? `01TEST${String(seq).padStart(20, "0")}`,
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
		finishedAt: null,
		source: "tui",
		ephemeralWorktree: false,
		error: null,
		session: "fresh",
		resumeSessionId: null,
		model: null,
		timeoutMs: null,
		prompt: "p",
		chainId: overrides.chainId ?? null,
		chainSeq: overrides.chainSeq ?? null,
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
			skip: [],
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

	it("skips a lane with a task already running", () => {
		const a = task({ worktree: "busy" });
		const live: LiveState = {
			runningLanes: new Set(["platform:busy"]),
			interactiveLanes: new Set(),
			runningCount: 1,
		};
		expect(schedule([a], live, { maxConcurrent: 5 }).start).toEqual([]);
	});

	it("starts on a lane held by an interactive session (no longer blocks)", () => {
		const b = task({ worktree: "yours" });
		const live: LiveState = {
			runningLanes: new Set(),
			interactiveLanes: new Set(["platform:yours"]),
			runningCount: 0,
		};
		expect(schedule([b], live, { maxConcurrent: 5 }).start).toEqual([b]);
	});

	it("does not pause a lane containing a failed task", () => {
		const failed = task({ status: "failed", worktree: "wt-a" });
		const queued = task({ worktree: "wt-a" });
		expect(
			schedule([failed, queued], idle, { maxConcurrent: 3 }).start,
		).toEqual([queued]);
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
			skip: [],
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

describe("schedule — task chains", () => {
	it("holds a tail while its predecessor is still running (not started, not skipped)", () => {
		const head = task({
			id: "01C1",
			status: "running",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 0,
		});
		const tail = task({
			id: "01C2",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 1,
		});
		const live: LiveState = {
			runningLanes: new Set(["platform:wt-c"]),
			interactiveLanes: new Set(),
			runningCount: 1,
		};
		const decision = schedule([head, tail], live, { maxConcurrent: 3 });
		expect(decision.start).toEqual([]);
		expect(decision.skip).toEqual([]);
	});

	it("starts a tail once its predecessor is done", () => {
		const head = task({
			id: "01C1",
			status: "done",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 0,
		});
		const tail = task({
			id: "01C2",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 1,
		});
		const { start, skip } = schedule([head, tail], idle, { maxConcurrent: 3 });
		expect(start).toEqual([tail]);
		expect(skip).toEqual([]);
	});

	it("skips a tail when its predecessor failed (stop-on-failure inside a chain)", () => {
		const head = task({
			id: "01C1",
			status: "failed",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 0,
		});
		const tail = task({
			id: "01C2",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 1,
		});
		const decision = schedule([head, tail], idle, { maxConcurrent: 3 });
		expect(decision.start).toEqual([]);
		expect(decision.skip.map((s) => s.task.id)).toEqual(["01C2"]);
		expect(decision.skip[0]?.reason).toContain("failed");
	});

	it("skips a tail when its predecessor is verify-failed (the check disagreed)", () => {
		const head = task({
			id: "01C1",
			status: "verify-failed",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 0,
		});
		const tail = task({
			id: "01C2",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 1,
		});
		const decision = schedule([head, tail], idle, { maxConcurrent: 3 });
		expect(decision.start).toEqual([]);
		expect(decision.skip.map((s) => s.task.id)).toEqual(["01C2"]);
		expect(decision.skip[0]?.reason).toContain("verify-failed");
	});

	it("skips a tail when its predecessor was cancelled (user stop/skip)", () => {
		const head = task({
			id: "01C1",
			status: "cancelled",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 0,
		});
		const tail = task({
			id: "01C2",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 1,
		});
		const decision = schedule([head, tail], idle, { maxConcurrent: 3 });
		expect(decision.start).toEqual([]);
		expect(decision.skip.map((s) => s.task.id)).toEqual(["01C2"]);
		expect(decision.skip[0]?.reason).toContain("cancelled");
	});

	it("skips a tail when its predecessor is parked in needs-input", () => {
		const head = task({
			id: "01C1",
			status: "needs-input",
			worktree: null,
			chainId: "c1",
			chainSeq: 0,
		});
		const tail = task({
			id: "01C2",
			worktree: null,
			chainId: "c1",
			chainSeq: 1,
		});
		const decision = schedule([head, tail], idle, { maxConcurrent: 3 });
		expect(decision.skip.map((s) => s.task.id)).toEqual(["01C2"]);
	});

	it("cascades a skip down a 3-step chain in one pass", () => {
		const a = task({
			id: "01C1",
			status: "failed",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 0,
		});
		const b = task({
			id: "01C2",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 1,
		});
		const c = task({
			id: "01C3",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 2,
		});
		const decision = schedule([a, b, c], idle, { maxConcurrent: 3 });
		// b skipped (pred failed) and c skipped (pred b skipped this pass).
		expect(decision.skip.map((s) => s.task.id).sort()).toEqual([
			"01C2",
			"01C3",
		]);
		expect(decision.start).toEqual([]);
	});

	it("never resolves a tail whose worktree is unstamped (no second temp spawn)", () => {
		const head = task({
			id: "01C1",
			status: "done",
			worktree: "wt-c",
			chainId: "c1",
			chainSeq: 0,
		});
		// Tail with worktree still null must NOT be routed to resolve.
		const tail = task({
			id: "01C2",
			worktree: null,
			chainId: "c1",
			chainSeq: 1,
		});
		const decision = schedule([head, tail], idle, { maxConcurrent: 3 });
		expect(decision.resolve).toEqual([]);
		expect(decision.start).toEqual([]);
		expect(decision.skip).toEqual([]);
	});

	it("leaves an independent task on the same lane unaffected by a chain failure", () => {
		const head = task({
			id: "01C1",
			status: "failed",
			worktree: "wt-shared",
			chainId: "c1",
			chainSeq: 0,
		});
		const tail = task({
			id: "01C2",
			worktree: "wt-shared",
			chainId: "c1",
			chainSeq: 1,
		});
		// Not part of the chain, same lane — must still start (failed no longer
		// pauses a lane, and the chain gate is scoped to chain members).
		const independent = task({ id: "01IND", worktree: "wt-shared" });
		const decision = schedule([head, tail, independent], idle, {
			maxConcurrent: 3,
		});
		expect(decision.start).toEqual([independent]);
		expect(decision.skip.map((s) => s.task.id)).toEqual(["01C2"]);
	});
});
