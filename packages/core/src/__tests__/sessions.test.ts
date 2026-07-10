import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { buildLiveState, SessionRegistry } from "../sessions.js";
import type { TaskInstance } from "../task.js";

const file = () =>
	join(mkdtempSync(join(tmpdir(), "qo-sess-")), "sessions.json");

function runningTask(worktree: string): TaskInstance {
	return {
		id: `01SESS${worktree.padEnd(20, "0")}`,
		status: "running",
		definition: null,
		item: null,
		itemKey: null,
		target: { repo: "platform", ref: "temp", worktree },
		priority: "normal",
		created: "2026-07-08T00:00:00.000Z",
		source: "tui",
		ephemeralWorktree: false,
		error: null,
		session: "fresh",
		resumeSessionId: null,
		model: null,
		prompt: "p",
	};
}

describe("SessionRegistry", () => {
	it("registers and persists workers, reloads from disk", () => {
		const path = file();
		const reg = new SessionRegistry(path);
		reg.registerWorker("t1", "platform:JUS-1", 111);
		const reloaded = new SessionRegistry(path);
		expect(reloaded.list()).toHaveLength(1);
		expect(reloaded.list()[0]?.lane).toBe("platform:JUS-1");
	});

	it("unregisters workers", () => {
		const reg = new SessionRegistry(file());
		reg.registerWorker("t1", "l", 1);
		reg.unregisterWorker("t1");
		expect(reg.list()).toEqual([]);
	});

	it("upserts interactive sessions keyed by cwd", () => {
		const reg = new SessionRegistry(file());
		reg.upsertInteractive("/wt/a", 5);
		reg.upsertInteractive("/wt/a", 5);
		expect(reg.list().filter((s) => s.kind === "interactive")).toHaveLength(1);
	});

	it("sweep drops stale interactive and dead workers", () => {
		const reg = new SessionRegistry(file(), {
			interactiveTtlMs: 1000,
			isPidAlive: (pid) => pid === 111,
		});
		reg.registerWorker("alive", "l1", 111);
		reg.registerWorker("dead", "l2", 222);
		reg.upsertInteractive("/wt/a", null);
		reg.sweep(Date.now() + 5000);
		const kinds = reg.list().map((s) => [s.kind, s.key]);
		expect(kinds).toEqual([["worker", "alive"]]);
	});

	it("tolerates corrupt file", () => {
		const path = file();
		writeFileSync(path, "{nope");
		expect(new SessionRegistry(path).list()).toEqual([]);
	});
});

describe("buildLiveState", () => {
	it("derives lanes from running tasks and interactive sessions", () => {
		const reg = new SessionRegistry(file());
		reg.upsertInteractive("/wt/main", null);
		reg.upsertInteractive("/wt/unknown", null);
		const live = buildLiveState(reg.list(), [runningTask("JUS-1")], (cwd) =>
			cwd === "/wt/main" ? "platform:main" : null,
		);
		expect(live.runningLanes).toEqual(new Set(["platform:JUS-1"]));
		expect(live.interactiveLanes).toEqual(new Set(["platform:main"]));
		expect(live.runningCount).toBe(1);
	});
});
