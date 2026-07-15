import {
	existsSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	renameSync,
	writeFileSync,
} from "node:fs";
import { join } from "node:path";
import type { TaskDefinition } from "./definition.js";
import type { Redactor } from "./redact.js";
import type { RunResult } from "./runner.js";
import type { TaskInstance } from "./task.js";

/**
 * The exact inputs the shim needs to reconstruct an `executeClaude` call: the
 * rendered prompt, resolved model/cwd/timeout, optional resume id, and the two
 * run-file paths. Written to `spawn.json` by the shim spawner (0600, unredacted
 * — the shim needs the real prompt) and unlinked by the shim after it reads it.
 * `redact`/`onSpawned` are NOT here: the shim builds its own redactor from its
 * inherited env and tracks the claude pid itself.
 */
export interface SpawnSpec {
	prompt: string;
	model: string;
	cwd: string;
	timeoutMs: number;
	resumeSessionId?: string;
	eventsPath: string;
	transcriptPath: string;
}

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
			/** Absolute checkout path this run executed in. The TUI "Resume" action
			 * uses it as the tmux window's cwd; a bare worktree name makes tmux
			 * `-c` fall back to $HOME. */
			resolvedWorktreePath: string;
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
			resolved_worktree_path: data.resolvedWorktreePath,
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

	spawnJsonPath(taskId: string): string {
		return join(this.runDir(taskId), "spawn.json");
	}

	/** Write the shim's launch spec. 0600 + UNREDACTED: it holds the real
	 * prompt, which the shim needs; the shim unlinks it immediately after read. */
	writeSpawnJson(taskId: string, spec: SpawnSpec): void {
		writeFileSync(this.spawnJsonPath(taskId), JSON.stringify(spec), {
			mode: 0o600,
		});
	}

	readSpawnJson(taskId: string): SpawnSpec | null {
		const path = this.spawnJsonPath(taskId);
		if (!existsSync(path)) return null;
		try {
			return JSON.parse(readFileSync(path, "utf-8")) as SpawnSpec;
		} catch {
			return null;
		}
	}

	private resultJsonPath(taskId: string): string {
		return join(this.runDir(taskId), "result.json");
	}

	/** Atomic (tmp + rename): the daemon must never read a torn result. */
	writeResultJson(taskId: string, result: RunResult): void {
		const path = this.resultJsonPath(taskId);
		const tmp = `${path}.tmp`;
		writeFileSync(tmp, JSON.stringify(result));
		renameSync(tmp, path);
	}

	readResultJson(taskId: string): RunResult | null {
		const path = this.resultJsonPath(taskId);
		if (!existsSync(path)) return null;
		try {
			return JSON.parse(readFileSync(path, "utf-8")) as RunResult;
		} catch {
			return null;
		}
	}

	private cancelMarkerPath(taskId: string): string {
		return join(this.runDir(taskId), "cancelled");
	}

	/** Persist a user Stop BEFORE signalling, so a stop that races a daemon death
	 * still settles the run as `cancelled` (not `failed`) on adoption. */
	writeCancelMarker(taskId: string): void {
		writeFileSync(this.cancelMarkerPath(taskId), "");
	}

	readCancelMarker(taskId: string): boolean {
		return existsSync(this.cancelMarkerPath(taskId));
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

	/** Task ids that have a run dir with data.json (for reverse session lookup). */
	listRunTaskIds(): string[] {
		let names: string[];
		try {
			names = readdirSync(this.runsDir);
		} catch {
			return [];
		}
		return names.filter((n) => existsSync(join(this.runsDir, n, "data.json")));
	}

	/**
	 * Lenient read of a run's data.json for reverse session lookup: the
	 * `session_id` (stamped by finishRun), the resolved `model` that run used,
	 * and the originating task's `prompt` matter to callers. Untyped fields are
	 * ignored; malformed files → null.
	 */
	readRunData(taskId: string): {
		session_id?: string | null;
		model?: string | null;
		task?: { prompt?: string };
	} | null {
		return this.readRunMeta(taskId) as {
			session_id?: string | null;
			model?: string | null;
			task?: { prompt?: string };
		} | null;
	}
}
