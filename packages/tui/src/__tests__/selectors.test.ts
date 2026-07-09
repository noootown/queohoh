import { describe, expect, it } from "vitest";
import {
	buildProjectTabs,
	buildWorktreeRows,
	computePaneLayout,
	matchesFilter,
	paneTitle,
	queueRowsForProject,
	windowRows,
	worktreeDotColor,
} from "../selectors.js";
import { makeSession, makeSnapshot, makeTask } from "./helpers.js";

const NOW = Date.parse("2026-07-08T10:03:12.000Z");

describe("buildProjectTabs", () => {
	it("lists config projects in order, no synthetic when no orphan repos", () => {
		const snapshot = makeSnapshot({
			projects: [{ name: "platform" }, { name: "web" }],
			tasks: [
				makeTask("running", {
					target: { repo: "platform", ref: "temp", worktree: "wt-a" },
				}),
			],
		});
		expect(buildProjectTabs(snapshot)).toEqual([
			{ name: "platform", synthetic: false },
			{ name: "web", synthetic: false },
		]);
	});

	it("appends synthetic tabs for orphan repos sorted alphabetically", () => {
		const snapshot = makeSnapshot({
			projects: [{ name: "platform" }],
			tasks: [
				makeTask("running", {
					target: { repo: "zeta", ref: "temp", worktree: "wt-a" },
				}),
				makeTask("queued", {
					target: { repo: "platform", ref: "temp", worktree: "wt-a" },
				}),
			],
			archivedRecent: [
				makeTask("done", {
					target: { repo: "alpha", ref: "temp", worktree: "wt-a" },
				}),
			],
		});
		expect(buildProjectTabs(snapshot)).toEqual([
			{ name: "platform", synthetic: false },
			{ name: "alpha", synthetic: true },
			{ name: "zeta", synthetic: true },
		]);
	});

	it("keeps config projects even when they have no tasks", () => {
		const snapshot = makeSnapshot({ projects: [{ name: "platform" }] });
		expect(buildProjectTabs(snapshot)).toEqual([
			{ name: "platform", synthetic: false },
		]);
	});
});

describe("queueRowsForProject", () => {
	it("excludes tasks and archived tasks of other projects", () => {
		const snapshot = makeSnapshot({
			tasks: [
				makeTask("running", {
					id: "01TASKAAA000000000000000000",
					target: { repo: "platform", ref: "temp", worktree: "wt-a" },
					prompt: "platform live\n",
				}),
				makeTask("running", {
					id: "01TASKBBB000000000000000000",
					target: { repo: "web", ref: "temp", worktree: "wt-b" },
					prompt: "web live\n",
				}),
			],
			archivedRecent: [
				makeTask("done", {
					id: "01TASKCCC000000000000000000",
					target: { repo: "platform", ref: "temp", worktree: "wt-a" },
					prompt: "platform archived\n",
				}),
				makeTask("done", {
					id: "01TASKDDD000000000000000000",
					target: { repo: "web", ref: "temp", worktree: "wt-b" },
					prompt: "web archived\n",
				}),
			],
		});
		const rows = queueRowsForProject(snapshot, "platform", NOW, 80);
		expect(rows.map((r) => r.id)).toEqual([
			"01TASKAAA000000000000000000",
			"01TASKCCC000000000000000000",
		]);
		expect(rows.map((r) => r.kind)).toEqual(["live", "archived"]);
	});
});

describe("buildWorktreeRows", () => {
	const worktrees = {
		platform: [
			{ name: "wt-a", path: "/wt/wt-a", branch: "feat/a" },
			{ name: "wt-b", path: "/wt/wt-b", branch: "feat/b" },
			{ name: "wt-c", path: "/wt/wt-c", branch: "feat/c" },
		],
	};

	it("marks a worktree busy when a running task shares its lane", () => {
		const snapshot = makeSnapshot({
			worktrees,
			tasks: [
				makeTask("running", {
					target: { repo: "platform", ref: "temp", worktree: "wt-a" },
				}),
			],
		});
		const rows = buildWorktreeRows(snapshot, "platform");
		expect(rows.find((r) => r.name === "wt-a")?.state).toBe("busy");
	});

	it("marks a worktree failed when the newest lane task failed and none running", () => {
		const snapshot = makeSnapshot({
			worktrees,
			tasks: [
				makeTask("done", {
					id: "01TASKB00000000000000000001",
					target: { repo: "platform", ref: "temp", worktree: "wt-b" },
				}),
				makeTask("failed", {
					id: "01TASKB00000000000000000002",
					target: { repo: "platform", ref: "temp", worktree: "wt-b" },
				}),
			],
		});
		const rows = buildWorktreeRows(snapshot, "platform");
		expect(rows.find((r) => r.name === "wt-b")?.state).toBe("failed");
	});

	it("is free when newest lane task is not failed", () => {
		const snapshot = makeSnapshot({
			worktrees,
			tasks: [
				makeTask("failed", {
					id: "01TASKB00000000000000000001",
					target: { repo: "platform", ref: "temp", worktree: "wt-c" },
				}),
				makeTask("done", {
					id: "01TASKB00000000000000000002",
					target: { repo: "platform", ref: "temp", worktree: "wt-c" },
				}),
			],
		});
		const rows = buildWorktreeRows(snapshot, "platform");
		expect(rows.find((r) => r.name === "wt-c")?.state).toBe("free");
	});

	it("running beats a newer failed task (busy wins)", () => {
		const snapshot = makeSnapshot({
			worktrees,
			tasks: [
				makeTask("running", {
					id: "01TASKB00000000000000000001",
					target: { repo: "platform", ref: "temp", worktree: "wt-a" },
				}),
				makeTask("failed", {
					id: "01TASKB00000000000000000009",
					target: { repo: "platform", ref: "temp", worktree: "wt-a" },
				}),
			],
		});
		const rows = buildWorktreeRows(snapshot, "platform");
		expect(rows.find((r) => r.name === "wt-a")?.state).toBe("busy");
	});

	it("emits worktree rows in order with full fields", () => {
		const snapshot = makeSnapshot({ worktrees });
		const rows = buildWorktreeRows(snapshot, "platform");
		expect(rows).toEqual([
			{
				kind: "worktree",
				name: "wt-a",
				path: "/wt/wt-a",
				branch: "feat/a",
				state: "free",
				hasMainSession: false,
				queued: 0,
			},
			{
				kind: "worktree",
				name: "wt-b",
				path: "/wt/wt-b",
				branch: "feat/b",
				state: "free",
				hasMainSession: false,
				queued: 0,
			},
			{
				kind: "worktree",
				name: "wt-c",
				path: "/wt/wt-c",
				branch: "feat/c",
				state: "free",
				hasMainSession: false,
				queued: 0,
			},
		]);
	});

	it("flags hasMainSession for worktrees whose lane has a stored main session", () => {
		const snapshot = makeSnapshot({
			worktrees,
			mainSessions: { "platform:wt-b": "sess-main" },
		});
		const rows = buildWorktreeRows(snapshot, "platform");
		expect(rows.find((r) => r.name === "wt-a")?.hasMainSession).toBe(false);
		expect(rows.find((r) => r.name === "wt-b")?.hasMainSession).toBe(true);
	});

	it("appends a session row for an interactive session whose cwd is inside a worktree", () => {
		const snapshot = makeSnapshot({
			worktrees,
			sessions: [
				makeSession({ cwd: "/wt/wt-b/packages/tui" }),
				makeSession({ key: "sess-outside", cwd: "/elsewhere/repo" }),
				makeSession({ key: "sess-worker", kind: "worker", cwd: "/wt/wt-a" }),
			],
		});
		const rows = buildWorktreeRows(snapshot, "platform");
		const sessionRows = rows.filter((r) => r.kind === "session");
		expect(sessionRows).toEqual([
			{
				kind: "session",
				name: "tui",
				path: "/wt/wt-b/packages/tui",
				branch: null,
				state: "you",
				hasMainSession: false,
				queued: 0,
			},
		]);
	});

	it("matches a session whose cwd equals a worktree path exactly", () => {
		const snapshot = makeSnapshot({
			worktrees,
			sessions: [makeSession({ cwd: "/wt/wt-a" })],
		});
		const sessionRows = buildWorktreeRows(snapshot, "platform").filter(
			(r) => r.kind === "session",
		);
		expect(sessionRows).toEqual([
			{
				kind: "session",
				name: "wt-a",
				path: "/wt/wt-a",
				branch: null,
				state: "you",
				hasMainSession: false,
				queued: 0,
			},
		]);
	});

	it("does not match a session whose cwd is a sibling sharing a path prefix", () => {
		const snapshot = makeSnapshot({
			worktrees,
			sessions: [makeSession({ cwd: "/wt/wt-a-sibling" })],
		});
		const sessionRows = buildWorktreeRows(snapshot, "platform").filter(
			(r) => r.kind === "session",
		);
		expect(sessionRows).toEqual([]);
	});

	it("returns no rows for a project with no worktrees", () => {
		const snapshot = makeSnapshot({ worktrees });
		expect(buildWorktreeRows(snapshot, "web")).toEqual([]);
	});

	it("strips the redundant <repo>. prefix from displayed worktree names", () => {
		const snapshot = makeSnapshot({
			worktrees: {
				platform: [
					{ name: "platform", path: "/wt/platform", branch: "main" },
					{
						name: "platform.dedup-dependabot-run",
						path: "/wt/platform.dedup-dependabot-run",
						branch: "dedup-dependabot-run",
					},
				],
			},
		});
		const rows = buildWorktreeRows(snapshot, "platform");
		// bare repo (name === repo) is kept; prefixed worktree is stripped
		expect(rows.map((r) => r.name)).toEqual([
			"platform",
			"dedup-dependabot-run",
		]);
		// underlying path (the identifier) is untouched
		expect(rows[1]?.path).toBe("/wt/platform.dedup-dependabot-run");
	});

	it("strips the <repo>. prefix from a session row's displayed name", () => {
		const snapshot = makeSnapshot({
			worktrees: {
				platform: [
					{
						name: "platform.feat-x",
						path: "/wt/platform.feat-x",
						branch: "feat-x",
					},
				],
			},
			sessions: [makeSession({ cwd: "/wt/platform.feat-x" })],
		});
		const sessionRows = buildWorktreeRows(snapshot, "platform").filter(
			(r) => r.kind === "session",
		);
		expect(sessionRows).toEqual([
			{
				kind: "session",
				name: "feat-x",
				path: "/wt/platform.feat-x",
				branch: null,
				state: "you",
				hasMainSession: false,
				queued: 0,
			},
		]);
	});

	it("counts queued tasks per worktree lane", () => {
		const snapshot = makeSnapshot({
			worktrees,
			tasks: [
				makeTask("queued", {
					id: "01TASKQ00000000000000000001",
					target: { repo: "platform", ref: "temp", worktree: "wt-a" },
				}),
				makeTask("queued", {
					id: "01TASKQ00000000000000000002",
					target: { repo: "platform", ref: "temp", worktree: "wt-a" },
				}),
				makeTask("running", {
					id: "01TASKQ00000000000000000003",
					target: { repo: "platform", ref: "temp", worktree: "wt-b" },
				}),
			],
		});
		const rows = buildWorktreeRows(snapshot, "platform");
		// wt-a has two queued; running/idle worktrees have none
		expect(rows.find((r) => r.name === "wt-a")?.queued).toBe(2);
		expect(rows.find((r) => r.name === "wt-b")?.queued).toBe(0);
		expect(rows.find((r) => r.name === "wt-c")?.queued).toBe(0);
	});
});

describe("worktreeDotColor", () => {
	it("maps idle to green, active to yellow, failed to red", () => {
		expect(worktreeDotColor("free")).toBe("green");
		expect(worktreeDotColor("busy")).toBe("yellow");
		expect(worktreeDotColor("you")).toBe("yellow");
		expect(worktreeDotColor("failed")).toBe("red");
	});
});

describe("computePaneLayout", () => {
	it("splits bodyHeight into three panes that exactly sum to it", () => {
		for (const bodyHeight of [13, 20, 38, 50, 77]) {
			const { queuePaneH, listPaneH } = computePaneLayout(bodyHeight);
			expect(queuePaneH + 2 * listPaneH).toBe(bodyHeight);
		}
	});

	it("gives the queue pane roughly half and the list panes a quarter each", () => {
		const { queuePaneH, listPaneH } = computePaneLayout(38);
		expect(listPaneH).toBe(9);
		expect(queuePaneH).toBe(20);
	});

	it("sets each capacity to its pane height minus border+title chrome", () => {
		const { queuePaneH, listPaneH, queueCap, listCap } = computePaneLayout(38);
		expect(queueCap).toBe(queuePaneH - 3);
		expect(listCap).toBe(listPaneH - 3);
	});

	it("keeps heights and capacities positive for a tiny body", () => {
		const layout = computePaneLayout(1);
		expect(layout.listPaneH).toBeGreaterThanOrEqual(4);
		expect(layout.queuePaneH).toBeGreaterThanOrEqual(4);
		expect(layout.queueCap).toBeGreaterThanOrEqual(1);
		expect(layout.listCap).toBeGreaterThanOrEqual(1);
	});
});

describe("windowRows", () => {
	const rows = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

	it("returns all rows when capacity >= length", () => {
		expect(windowRows(rows, 3, 20)).toEqual({ rows, offset: 0 });
	});

	it("keeps selection at the top edge", () => {
		expect(windowRows(rows, 0, 4)).toEqual({ rows: [0, 1, 2, 3], offset: 0 });
	});

	it("centers selection in the middle", () => {
		expect(windowRows(rows, 5, 4)).toEqual({ rows: [3, 4, 5, 6], offset: 3 });
	});

	it("clamps to the bottom edge", () => {
		expect(windowRows(rows, 9, 4)).toEqual({ rows: [6, 7, 8, 9], offset: 6 });
	});

	it("returns an empty window for non-positive capacity", () => {
		expect(windowRows(rows, 3, 0)).toEqual({ rows: [], offset: 0 });
	});
});

describe("matchesFilter", () => {
	it("empty query matches everything", () => {
		expect(matchesFilter("anything", "")).toBe(true);
	});
	it("case-insensitive substring", () => {
		expect(matchesFilter("Fix-TUI-Bug", "tui")).toBe(true);
		expect(matchesFilter("fix-tui-bug", "TUI")).toBe(true);
		expect(matchesFilter("fix-tui-bug", "xyz")).toBe(false);
	});
});

describe("paneTitle", () => {
	it("bare title when no filter and not active", () => {
		expect(paneTitle("QUEUE", "", false)).toBe("QUEUE");
	});
	it("shows committed filter", () => {
		expect(paneTitle("QUEUE", "foo", false)).toBe("QUEUE /foo");
	});
	it("shows cursor while typing, even with empty query", () => {
		expect(paneTitle("QUEUE", "fo", true)).toBe("QUEUE /fo█");
		expect(paneTitle("QUEUE", "", true)).toBe("QUEUE /█");
	});
});
