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
			outcome: "done" | "failed";
			reason: string | null;
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
		};
		writeFileSync(dataPath, redact(JSON.stringify(merged, null, 2)));

		const { usage } = data.result;
		const report = [
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
		].join("\n");
		writeFileSync(join(dir, "report.md"), redact(report));
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
