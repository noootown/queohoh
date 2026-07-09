import type { TaskInstance, TaskStatus } from "@queohoh/core";
import type { StateSnapshot } from "@queohoh/daemon";
import { describe, expect, it } from "vitest";
import {
	buildQueueRows,
	elapsed,
	promptSummary,
	statusGlyph,
	stripRepoPrefix,
} from "../format.js";

let seq = 0;
function task(
	status: TaskStatus,
	overrides: Partial<TaskInstance> = {},
): TaskInstance {
	seq += 1;
	return {
		id: `01TUI${String(seq).padStart(21, "0")}`,
		status,
		definition: null,
		item: null,
		itemKey: null,
		target: { repo: "platform", ref: "temp", worktree: "wt-a" },
		priority: "normal",
		created: "2026-07-08T10:00:00.000Z",
		source: "tui",
		ephemeralWorktree: false,
		error: null,
		session: "fresh",
		prompt: "fix the flaky test\nmore context\n",
		...overrides,
	};
}

const NOW = Date.parse("2026-07-08T10:03:12.000Z");

function snap(
	tasks: TaskInstance[],
	archived: TaskInstance[] = [],
): StateSnapshot {
	return {
		tasks,
		archivedRecent: archived,
		sessions: [],
		running: [],
		maxConcurrent: 1,
		projects: [],
		worktrees: {},
		mainSessions: {},
	};
}

describe("statusGlyph", () => {
	it("maps every status", () => {
		expect(statusGlyph("running")).toBe("▶");
		expect(statusGlyph("queued")).toBe("○");
		expect(statusGlyph("needs-input")).toBe("?");
		expect(statusGlyph("done")).toBe("✓");
		expect(statusGlyph("failed")).toBe("✗");
	});
});

describe("elapsed", () => {
	it("formats seconds, minutes, hours", () => {
		const start = "2026-07-08T10:00:00.000Z";
		expect(elapsed(start, Date.parse("2026-07-08T10:00:47.000Z"))).toBe("47s");
		expect(elapsed(start, Date.parse("2026-07-08T10:03:12.000Z"))).toBe(
			"3m12s",
		);
		expect(elapsed(start, Date.parse("2026-07-08T11:04:00.000Z"))).toBe(
			"1h04m",
		);
	});
});

describe("promptSummary", () => {
	it("takes the first non-empty line clipped with ellipsis", () => {
		expect(promptSummary("\n\nfix the thing\nrest", 20)).toBe("fix the thing");
		expect(promptSummary("a very long prompt line that overflows", 12)).toBe(
			"a very long…",
		);
	});
});

describe("stripRepoPrefix", () => {
	it("removes a leading <repo>. prefix", () => {
		expect(stripRepoPrefix("platform.dedup-dependabot-run", "platform")).toBe(
			"dedup-dependabot-run",
		);
	});

	it("keeps the bare repo name and unprefixed names unchanged", () => {
		expect(stripRepoPrefix("platform", "platform")).toBe("platform");
		expect(stripRepoPrefix("wt-a", "platform")).toBe("wt-a");
	});
});

describe("buildQueueRows", () => {
	it("strips the redundant <repo>. prefix from the worktree in the lane", () => {
		const running = task("running", {
			target: {
				repo: "platform",
				ref: "temp",
				worktree: "platform.dedup-dependabot-run",
			},
		});
		const rows = buildQueueRows(snap([running]), NOW, 40);
		expect(rows[0]?.lane).toBe("platform:dedup-dependabot-run");
	});

	it("renders running with elapsed, queued with lane position, failed with error", () => {
		const running = task("running");
		const q1 = task("queued");
		const q2 = task("queued");
		const failed = task("failed", { error: "tree left dirty" });
		const rows = buildQueueRows(snap([running, q1, q2, failed]), NOW, 40);
		expect(rows[0]?.detail).toBe("⏱ 3m12s");
		expect(rows[1]?.detail).toBe("#1 in lane");
		expect(rows[2]?.detail).toBe("#2 in lane");
		expect(rows[3]?.detail).toBe("tree left dirty");
		expect(rows[0]?.lane).toBe("platform:wt-a");
		expect(rows[0]?.kind).toBe("live");
	});

	it("uses ref as lane while unresolved and appends archived rows", () => {
		const pending = task("queued", {
			target: { repo: "platform", ref: "pr:257", worktree: null },
		});
		const old = task("done");
		const rows = buildQueueRows(snap([pending], [old]), NOW, 40);
		expect(rows[0]?.lane).toBe("platform:pr:257");
		expect(rows[1]?.kind).toBe("archived");
		expect(rows[1]?.detail).toBe("archived");
	});

	it("caps archived at last 10", () => {
		const archived = Array.from({ length: 15 }, () => task("done"));
		const rows = buildQueueRows(snap([], archived), NOW, 40);
		expect(rows).toHaveLength(10);
	});

	it("marks main-session tasks with a chain glyph, fresh tasks with none", () => {
		const mainTask = task("running", { session: "main" });
		const freshTask = task("queued", { session: "fresh" });
		const rows = buildQueueRows(snap([mainTask, freshTask]), NOW, 40);
		expect(rows[0]?.sessionMarker).toBe("⛓ ");
		expect(rows[1]?.sessionMarker).toBe("");
	});

	it("marks archived main-session tasks too", () => {
		const archivedMain = task("done", { session: "main" });
		const rows = buildQueueRows(snap([], [archivedMain]), NOW, 40);
		expect(rows[0]?.sessionMarker).toBe("⛓ ");
	});
});
