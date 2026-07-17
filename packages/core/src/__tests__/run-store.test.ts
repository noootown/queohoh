import { existsSync, mkdtempSync, readFileSync, statSync } from "node:fs";
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
	finishedAt: null,
	source: "mcp",
	ephemeralWorktree: false,
	error: null,
	session: "fresh",
	resumeSessionId: null,
	model: null,
	timeoutMs: null,
	prompt: "Review PR 257 with secret shh-token.\n",
	attemptedModels: [],
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
				resolvedWorktreePath: "/wt/platform.JUS-257",
				prompt: task.prompt,
				model: "opus",
			},
			redact,
		);
		const meta = rs.readRunMeta(task.id);
		expect(meta?.resolved_worktree).toBe("JUS-257");
		// The absolute path the run executed in — the TUI's "Resume" action uses
		// it as the tmux window cwd (name alone → tmux falls back to $HOME).
		expect(meta?.resolved_worktree_path).toBe("/wt/platform.JUS-257");
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
				resolvedWorktreePath: "/wt/platform.JUS-257",
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
					signal: null,
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

	it("writeSnapshot records provider; finishRun renders <provider>/<model>", () => {
		const rs = fresh();
		rs.writeSnapshot(
			task.id,
			{
				task,
				definition: null,
				resolvedWorktree: "JUS-257",
				resolvedWorktreePath: "/wt/platform.JUS-257",
				prompt: task.prompt,
				model: "grok-composer-2.5-fast",
				provider: "grok",
			},
			redact,
		);
		expect(rs.readRunMeta(task.id)?.provider).toBe("grok");
		rs.finishRun(
			task.id,
			{
				result: {
					exitCode: 0,
					timedOut: false,
					signal: null,
					sessionId: "s1",
					resultText: "ok",
					stderr: "",
					usage: { costUsd: null, turns: null, durationMs: null },
				},
				outcome: "done",
				reason: null,
			},
			redact,
		);
		const report = readFileSync(join(rs.runDir(task.id), "report.md"), "utf-8");
		// Provider recorded on the snapshot → "<provider>/<model>" Stats line.
		expect(report).toContain("- model: grok/grok-composer-2.5-fast");
	});
});

describe("RunStore attempt trail (fallback hops)", () => {
	const doneResult = {
		exitCode: 0,
		timedOut: false,
		signal: null,
		sessionId: "s1",
		resultText: "ok",
		stderr: "",
		usage: { costUsd: null, turns: null, durationMs: null },
	};

	const snapshot = (rs: RunStore) =>
		rs.writeSnapshot(
			task.id,
			{
				task,
				definition: null,
				resolvedWorktree: "JUS-257",
				resolvedWorktreePath: "/wt/platform.JUS-257",
				prompt: task.prompt,
				model: "opus",
			},
			redact,
		);

	it("appendAttempt accumulates a trail that finishRun renders into report.md", () => {
		const rs = fresh();
		snapshot(rs);
		rs.appendAttempt(
			task.id,
			"attempt 1: claude — session limit → falling back",
			redact,
		);
		rs.appendAttempt(task.id, "attempt 2: grok — provider unavailable", redact);
		// Persisted on data.json in order.
		expect(rs.readRunMeta(task.id)?.attempts).toEqual([
			"attempt 1: claude — session limit → falling back",
			"attempt 2: grok — provider unavailable",
		]);
		rs.finishRun(
			task.id,
			{ result: doneResult, outcome: "failed", reason: "provider unavailable" },
			redact,
		);
		const report = readFileSync(join(rs.runDir(task.id), "report.md"), "utf-8");
		expect(report).toContain("## Attempts");
		expect(report).toContain(
			"- attempt 1: claude — session limit → falling back",
		);
		expect(report).toContain("- attempt 2: grok — provider unavailable");
	});

	it("finishRun omits the Attempts section when no hop was recorded", () => {
		const rs = fresh();
		snapshot(rs);
		rs.finishRun(
			task.id,
			{ result: doneResult, outcome: "done", reason: null },
			redact,
		);
		const report = readFileSync(join(rs.runDir(task.id), "report.md"), "utf-8");
		expect(report).not.toContain("## Attempts");
	});

	it("writeSnapshot preserves the attempt trail across the next attempt's rewrite", () => {
		const rs = fresh();
		snapshot(rs);
		rs.appendAttempt(
			task.id,
			"attempt 1: claude — session limit → falling back",
			redact,
		);
		// A fresh attempt fully rewrites data.json — the trail must survive it,
		// so the next attempt's finishRun can render the whole history.
		snapshot(rs);
		expect(rs.readRunMeta(task.id)?.attempts).toEqual([
			"attempt 1: claude — session limit → falling back",
		]);
		// And it is not double-counted: appending again grows the trail by one.
		rs.appendAttempt(task.id, "attempt 2: grok — provider unavailable", redact);
		expect(rs.readRunMeta(task.id)?.attempts).toHaveLength(2);
	});

	it("the trail is redacted on disk", () => {
		const rs = fresh();
		snapshot(rs);
		rs.appendAttempt(task.id, "attempt 1: leaked shh-token here", redact);
		const raw = readFileSync(join(rs.runDir(task.id), "data.json"), "utf-8");
		expect(raw).toContain("[REDACTED:GH_TOKEN]");
		expect(raw).not.toContain("shh-token");
	});
});

describe("RunStore shim contract files", () => {
	const spec = {
		prompt: "do it with shh-token",
		model: "opus",
		cwd: "/wt/x",
		timeoutMs: 60_000,
		resumeSessionId: "sess-1",
		eventsPath: "/wt/x/events.jsonl",
		transcriptPath: "/wt/x/transcript.md",
	};
	const result = {
		exitCode: 0,
		timedOut: false,
		signal: null,
		sessionId: "s1",
		resultText: "ok",
		stderr: "",
		usage: { costUsd: 1, turns: 2, durationMs: 3 },
	};

	it("spawn.json round-trips UNREDACTED and is mode 0600", () => {
		const rs = fresh();
		rs.writeSpawnJson(task.id, spec);
		const back = rs.readSpawnJson(task.id);
		expect(back).toEqual(spec);
		// Unredacted on disk: the shim needs the real prompt.
		const raw = readFileSync(rs.spawnJsonPath(task.id), "utf-8");
		expect(raw).toContain("shh-token");
		const mode = statSync(rs.spawnJsonPath(task.id)).mode & 0o777;
		expect(mode).toBe(0o600);
	});

	it("readSpawnJson returns null for missing/malformed", () => {
		const rs = fresh();
		expect(rs.readSpawnJson("01NOPE")).toBeNull();
	});

	it("result.json round-trips and readResultJson is null when absent", () => {
		const rs = fresh();
		expect(rs.readResultJson(task.id)).toBeNull();
		rs.writeResultJson(task.id, result);
		expect(rs.readResultJson(task.id)).toEqual(result);
	});

	it("clearResultJson removes a stale result.json and tolerates absence", () => {
		const rs = fresh();
		// Absent file: best-effort unlink is a no-op, not an error (the common
		// case — a task's first attempt).
		expect(() => rs.clearResultJson(task.id)).not.toThrow();
		rs.writeResultJson(task.id, result);
		expect(rs.readResultJson(task.id)).toEqual(result);
		// A fresh attempt clears the prior attempt's result so the adoption
		// sweep can't finalize with a stale one.
		rs.clearResultJson(task.id);
		expect(rs.readResultJson(task.id)).toBeNull();
	});

	it("cancel marker: absent → false, written → true", () => {
		const rs = fresh();
		expect(rs.readCancelMarker(task.id)).toBe(false);
		rs.writeCancelMarker(task.id);
		expect(rs.readCancelMarker(task.id)).toBe(true);
	});
});
