import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { makeRedactor } from "../redact.js";
import { RunStore } from "../run-store.js";
import type { TaskInstance } from "../task.js";

const task: TaskInstance = {
	id: "01RUNSTORE0000000000000000",
	status: "running",
	definition: "platform/pr-review",
	item: { number: "257" },
	itemKey: "257",
	target: { repo: "platform", ref: "pr:257", worktree: "JUS-257" },
	priority: "normal",
	created: "2026-07-08T10:00:00.000Z",
	source: "mcp",
	ephemeralWorktree: false,
	error: null,
	session: "fresh",
	prompt: "Review PR 257 with secret shh-token.\n",
};

const redact = makeRedactor(new Map([["shh-token", "GH_TOKEN"]]));
const fresh = () => new RunStore(mkdtempSync(join(tmpdir(), "qo-runs-")));

describe("RunStore", () => {
	it("writeSnapshot writes redacted data.json and prompt", () => {
		const rs = fresh();
		rs.writeSnapshot(
			task.id,
			{
				task,
				definition: null,
				resolvedWorktree: "JUS-257",
				prompt: task.prompt,
				model: "opus",
			},
			redact,
		);
		const meta = rs.readRunMeta(task.id);
		expect(meta?.resolved_worktree).toBe("JUS-257");
		expect(meta?.model).toBe("opus");
		expect(typeof meta?.started_at).toBe("string");
		const prompt = readFileSync(
			join(rs.runDir(task.id), "prompt.rendered.md"),
			"utf-8",
		);
		expect(prompt).toContain("[REDACTED:GH_TOKEN]");
		expect(prompt).not.toContain("shh-token");
	});

	it("worker pid round-trips", () => {
		const rs = fresh();
		rs.writeWorkerPid(task.id, 4242);
		expect(rs.readWorkerPid(task.id)).toBe(4242);
		expect(rs.readWorkerPid("01NOPE")).toBeNull();
	});

	it("finishRun merges outcome into data.json and writes report.md", () => {
		const rs = fresh();
		rs.writeSnapshot(
			task.id,
			{
				task,
				definition: null,
				resolvedWorktree: "JUS-257",
				prompt: task.prompt,
				model: "opus",
			},
			redact,
		);
		rs.finishRun(
			task.id,
			{
				result: {
					exitCode: 0,
					timedOut: false,
					sessionId: "s1",
					resultText: "Fixed everything with shh-token.",
					stderr: "",
					usage: { costUsd: 1.5, turns: 7, durationMs: 60000 },
				},
				outcome: "done",
				reason: null,
			},
			redact,
		);
		const meta = rs.readRunMeta(task.id);
		expect(meta?.outcome).toBe("done");
		expect(meta?.exit_code).toBe(0);
		expect((meta?.usage as Record<string, unknown> | undefined)?.costUsd).toBe(
			1.5,
		);
		const report = readFileSync(join(rs.runDir(task.id), "report.md"), "utf-8");
		expect(report).toContain("[REDACTED:GH_TOKEN]");
		expect(report).toContain("- model: opus");
		expect(report).toContain("$1.5");
		expect(existsSync(rs.eventsPath(task.id))).toBe(false); // runner owns these
	});

	it("readRunMeta returns null for unknown task", () => {
		expect(fresh().readRunMeta("01NOPE")).toBeNull();
	});
});
