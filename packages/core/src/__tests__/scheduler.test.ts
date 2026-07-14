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
	runningByRepo: new Map(),
};

describe("schedule", () => {
	it("starts a queued resolved task on a free lane", () => {
		const t = task({});
		expect(schedule([t], idle, { perProjectMax: 3 })).toEqual({
			start: [t],
			resolve: [],
			skip: [],
		});
	});

	it("ignores non-queued statuses", () => {
		const tasks = (["needs-input", "running", "done"] as const).map((status) =>
			task({ status, worktree: `wt-${status}` }),
		);
		expect(schedule(tasks, idle, { perProjectMax: 5 }).start).toEqual([]);
	});

	it("orders by priority band then id", () => {
		const low = task({ priority: "low", worktree: "wt-1" });
		const high = task({ priority: "high", worktree: "wt-2" });
		const normal = task({ priority: "normal", worktree: "wt-3" });
		const { start } = schedule([low, high, normal], idle, {
			perProjectMax: 3,
		});
		expect(start.map((t) => t.id)).toEqual([high.id, normal.id, low.id]);
	});

	it("skips a lane with a task already running", () => {
		const a = task({ worktree: "busy" });
		const live: LiveState = {
			runningLanes: new Set(["platform:busy"]),
			interactiveLanes: new Set(),
			runningByRepo: new Map([["platform", 1]]),
		};
		expect(schedule([a], live, { perProjectMax: 5 }).start).toEqual([]);
	});

	it("starts on a lane held by an interactive session (no longer blocks)", () => {
		const b = task({ worktree: "yours" });
		const live: LiveState = {
			runningLanes: new Set(),
			interactiveLanes: new Set(["platform:yours"]),
			runningByRepo: new Map(),
		};
		expect(schedule([b], live, { perProjectMax: 5 }).start).toEqual([b]);
	});

	it("does not pause a lane containing a failed task", () => {
		const failed = task({ status: "failed", worktree: "wt-a" });
		const queued = task({ worktree: "wt-a" });
		expect(
			schedule([failed, queued], idle, { perProjectMax: 3 }).start,
		).toEqual([queued]);
	});

	it("enforces the per-project cap across start + resolve + running", () => {
		const a = task({ worktree: "wt-1" });
		const b = task({ worktree: null });
		const c = task({ worktree: "wt-3" });
		const live: LiveState = {
			...idle,
			runningByRepo: new Map([["platform", 1]]),
		};
		const decision = schedule([a, b, c], live, { perProjectMax: 2 });
		expect(decision.start).toEqual([a]);
		expect(decision.resolve).toEqual([]);
	});

	it("routes unresolved tasks to resolve", () => {
		const t = task({ worktree: null });
		expect(schedule([t], idle, { perProjectMax: 3 })).toEqual({
			start: [],
			resolve: [t],
			skip: [],
		});
	});

	it("starts at most one task per lane per decision", () => {
		const first = task({ worktree: "wt-a" });
		const second = task({ worktree: "wt-a" });
		const { start } = schedule([first, second], idle, { perProjectMax: 5 });
		expect(start).toEqual([first]);
	});

	it("treats the @repo sentinel as an ordinary lane (serializes primary-checkout tasks)", () => {
		const first = task({ worktree: "@repo" });
		const second = task({ worktree: "@repo" });
		const { start } = schedule([first, second], idle, { perProjectMax: 5 });
		// Both share the `platform:@repo` lane, so only one starts this decision.
		expect(start).toEqual([first]);
	});

	it("does not let one saturated project consume another project's slots", () => {
		// Project A already has perProjectMax(2) running; project B is idle.
		const aQueued = task({ repo: "alpha", worktree: "wt-a-3" });
		const bQueued = task({ repo: "beta", worktree: "wt-b-1" });
		const live: LiveState = {
			runningLanes: new Set(["alpha:wt-a-1", "alpha:wt-a-2"]),
			interactiveLanes: new Set(),
			runningByRepo: new Map([["alpha", 2]]),
		};
		const decision = schedule([aQueued, bQueued], live, { perProjectMax: 2 });
		expect(decision.start).toEqual([bQueued]);
	});

	it("lets each of several projects independently reach perProjectMax", () => {
		const alpha1 = task({ repo: "alpha", worktree: "wt-a-1" });
		const alpha2 = task({ repo: "alpha", worktree: "wt-a-2" });
		const alpha3 = task({ repo: "alpha", worktree: "wt-a-3" });
		const beta1 = task({ repo: "beta", worktree: "wt-b-1" });
		const beta2 = task({ repo: "beta", worktree: "wt-b-2" });
		const decision = schedule([alpha1, alpha2, alpha3, beta1, beta2], idle, {
			perProjectMax: 2,
		});
		expect(decision.start.map((t) => t.id).sort()).toEqual(
			[alpha1.id, alpha2.id, beta1.id, beta2.id].sort(),
		);
	});

	it("still serializes two tasks on the same worktree within one project", () => {
		const first = task({ repo: "alpha", worktree: "wt-shared" });
		const second = task({ repo: "alpha", worktree: "wt-shared" });
		const { start } = schedule([first, second], idle, { perProjectMax: 5 });
		expect(start).toEqual([first]);
	});

	it("runs tasks on different worktrees within one project concurrently, bounded by perProjectMax", () => {
		const w1 = task({ repo: "alpha", worktree: "wt-1" });
		const w2 = task({ repo: "alpha", worktree: "wt-2" });
		const { start } = schedule([w1, w2], idle, { perProjectMax: 5 });
		expect(start.map((t) => t.id).sort()).toEqual([w1.id, w2.id].sort());
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
			runningByRepo: new Map([["platform", 1]]),
		};
		const decision = schedule([head, tail], live, { perProjectMax: 3 });
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
		const { start, skip } = schedule([head, tail], idle, { perProjectMax: 3 });
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
		const decision = schedule([head, tail], idle, { perProjectMax: 3 });
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
		const decision = schedule([head, tail], idle, { perProjectMax: 3 });
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
		const decision = schedule([head, tail], idle, { perProjectMax: 3 });
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
		const decision = schedule([head, tail], idle, { perProjectMax: 3 });
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
		const decision = schedule([a, b, c], idle, { perProjectMax: 3 });
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
		const decision = schedule([head, tail], idle, { perProjectMax: 3 });
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
			perProjectMax: 3,
		});
		expect(decision.start).toEqual([independent]);
		expect(decision.skip.map((s) => s.task.id)).toEqual(["01C2"]);
	});
});
