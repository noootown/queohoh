import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { TaskDefinition } from "./definition.js";
import type { Redactor } from "./redact.js";
import type { RunResult } from "./runner.js";
import type { TaskInstance } from "./task.js";

export class RunStore {
	constructor(readonly runsDir: string) {
		mkdirSync(runsDir, { recursive: true });
	}

	runDir(taskId: string): string {
		const dir = join(this.runsDir, taskId);
		mkdirSync(dir, { recursive: true });
		return dir;
	}

	eventsPath(taskId: string): string {
		return join(this.runDir(taskId), "events.jsonl");
	}

	transcriptPath(taskId: string): string {
		return join(this.runDir(taskId), "transcript.md");
	}

	writeSnapshot(
		taskId: string,
		data: {
			task: TaskInstance;
			definition: TaskDefinition | null;
			resolvedWorktree: string;
			prompt: string;
			model: string;
		},
		redact: Redactor,
	): void {
		const dir = this.runDir(taskId);
		const snapshot = {
			task: data.task,
			definition: data.definition,
			resolved_worktree: data.resolvedWorktree,
			model: data.model,
			started_at: new Date().toISOString(),
		};
		writeFileSync(
			join(dir, "data.json"),
			redact(JSON.stringify(snapshot, null, 2)),
		);
		writeFileSync(join(dir, "prompt.rendered.md"), redact(data.prompt));
	}

	writeWorkerPid(taskId: string, pid: number): void {
		writeFileSync(
			join(this.runDir(taskId), "worker.json"),
			JSON.stringify({ pid }),
		);
	}

	readWorkerPid(taskId: string): number | null {
		const path = join(this.runsDir, taskId, "worker.json");
		if (!existsSync(path)) return null;
		try {
			const parsed = JSON.parse(readFileSync(path, "utf-8"));
			return typeof parsed.pid === "number" ? parsed.pid : null;
		} catch {
			return null;
		}
	}

	finishRun(
		taskId: string,
		data: {
			result: RunResult;
			outcome: "done" | "failed" | "cancelled" | "verify-failed";
			reason: string | null;
			// The done-condition (`verify`) outcome, when the gate ran. `output` is
			// the raw combined-output tail — redacted here on the way to disk.
			verify?: {
				command: string;
				verified: boolean;
				exitCode: number | null;
				output: string;
			} | null;
		},
		redact: Redactor,
	): void {
		const dir = this.runDir(taskId);
		const dataPath = join(dir, "data.json");
		let existing: Record<string, unknown> = {};
		if (existsSync(dataPath)) {
			try {
				existing = JSON.parse(readFileSync(dataPath, "utf-8"));
			} catch {}
		}
		const merged = {
			...existing,
			finished_at: new Date().toISOString(),
			outcome: data.outcome,
			reason: data.reason,
			exit_code: data.result.exitCode,
			timed_out: data.result.timedOut,
			session_id: data.result.sessionId,
			usage: data.result.usage,
			// Verify verdict (snake_case, like the rest of data.json) when the gate
			// ran; absent otherwise.
			...(data.verify && {
				verify_command: data.verify.command,
				verified: data.verify.verified,
				verify_exit_code: data.verify.exitCode,
				verify_output: data.verify.output,
			}),
		};
		writeFileSync(dataPath, redact(JSON.stringify(merged, null, 2)));

		const { usage } = data.result;
		const lines = [
			"# Result",
			"",
			data.result.resultText || "(no result text)",
			"",
			"## Stats",
			`- outcome: ${data.outcome}${data.reason ? ` (${data.reason})` : ""}`,
			`- model: ${typeof existing.model === "string" ? existing.model : "n/a"}`,
			`- cost: ${usage.costUsd === null ? "n/a" : `$${usage.costUsd}`}`,
			`- turns: ${usage.turns ?? "n/a"}`,
			`- duration: ${usage.durationMs === null ? "n/a" : `${Math.round(usage.durationMs / 1000)}s`}`,
			"",
		];
		// Done-condition section — mirrors the Stats block's error-display pattern so
		// the detail pane's report tab shows what was checked and its output tail.
		if (data.verify) {
			lines.push(
				"## Verify",
				`- result: ${data.verify.verified ? "passed" : "failed"}`,
				`- exit: ${data.verify.exitCode ?? "timed out"}`,
				`- command: ${data.verify.command}`,
				"",
				"```",
				data.verify.output.trim() || "(no output)",
				"```",
				"",
			);
		}
		writeFileSync(join(dir, "report.md"), redact(lines.join("\n")));
	}

	readRunMeta(taskId: string): Record<string, unknown> | null {
		const path = join(this.runsDir, taskId, "data.json");
		if (!existsSync(path)) return null;
		try {
			return JSON.parse(readFileSync(path, "utf-8"));
		} catch {
			return null;
		}
	}
}
