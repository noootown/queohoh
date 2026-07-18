import {
	existsSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	renameSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { join } from "node:path";
import type { TaskDefinition } from "./definition.js";
import type { Redactor } from "./redact.js";
import type { RunResult, RunUsage } from "./runner.js";
import type { TaskInstance } from "./task.js";

/** Compact token-count formatting for the report.md Stats block, mirroring the
 * TUI's `compact_count` (crates/qoo-tui/src/view/detail.rs): below 1000 the
 * bare number, below 1,000,000 rounded to the nearest thousand with a `k`
 * suffix, at or above 1,000,000 one decimal place with an `M` suffix. */
export function formatTokenCount(n: number): string {
	if (n < 1000) return String(n);
	if (n < 1_000_000) return `${Math.round(n / 1000)}k`;
	return `${(n / 1_000_000).toFixed(1)}M`;
}

/** The report.md `- tokens:` line body: `n/a` when the run recorded no token
 * usage at all (neither side present — e.g. a provider whose events carry no
 * usage object), else `<in> in / <out> out` with each side independently
 * falling back to `n/a` if that one side is missing. */
export function formatTokensLine(usage: RunUsage): string {
	if (usage.inputTokens === null && usage.outputTokens === null) return "n/a";
	const inStr =
		usage.inputTokens === null ? "n/a" : formatTokenCount(usage.inputTokens);
	const outStr =
		usage.outputTokens === null ? "n/a" : formatTokenCount(usage.outputTokens);
	return `${inStr} in / ${outStr} out`;
}

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
	/** Provider adapter name (e.g. "claude", "codex", "grok"). Absent ⇒ "claude"
	 * — adoption-safe for a spawn.json written by an older daemon that predates
	 * multi-provider support. */
	provider?: string;
	/** Appended via `--append-system-prompt` for adapters that support it. */
	systemPrompt?: string;
	/** Provider-config `args` — additional adapter-produced argv. */
	extraArgs?: string[];
	/** Bin override; undefined ⇒ the adapter's own `defaultBin`. */
	bin?: string;
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
			/** Adapter name that produced/will produce `model` (spec §5). Optional so
			 * pre-provider-adapter callers/test literals need not set it — absent ⇒
			 * report.md's Stats line falls back to the bare model id. */
			provider?: string;
		},
		redact: Redactor,
	): void {
		const dir = this.runDir(taskId);
		const dataPath = join(dir, "data.json");
		// This write otherwise fully replaces data.json on every fresh attempt —
		// preserve the prior attempt's hop trail (finding 5's `appendAttempt`)
		// across that rewrite rather than silently dropping it.
		let attempts: string[] = [];
		if (existsSync(dataPath)) {
			try {
				const existing = JSON.parse(readFileSync(dataPath, "utf-8"));
				if (Array.isArray(existing.attempts)) attempts = existing.attempts;
			} catch {}
		}
		const snapshot = {
			task: data.task,
			definition: data.definition,
			resolved_worktree: data.resolvedWorktree,
			resolved_worktree_path: data.resolvedWorktreePath,
			model: data.model,
			provider: data.provider,
			started_at: new Date().toISOString(),
			...(attempts.length > 0 && { attempts }),
		};
		writeFileSync(dataPath, redact(JSON.stringify(snapshot, null, 2)));
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

	/** Best-effort unlink of a stale `result.json` from a PRIOR attempt. Without
	 * this, attempt N's result.json survives into attempt N+1's window: a daemon
	 * restart/reload mid-attempt-(N+1) would let the adoption sweep's `hasResult
	 * → "finalize"` check finalize the task with attempt N's stale result while
	 * the fresh attempt's detached shim is still running. Called by `startRun`
	 * right before a fresh attempt's own artifacts are (re-)written. Absent file
	 * is not an error — the common case (a task's first attempt). */
	clearResultJson(taskId: string): void {
		try {
			unlinkSync(this.resultJsonPath(taskId));
		} catch {}
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

	/** Append one line to the run's persisted attempt trail (finding 5 — the
	 * fallback-chain hop history, e.g. "attempt 1: claude — session limit →
	 * falling back"). Read-merge-write like `finishRun`, so it survives both a
	 * same-attempt `finishRun` call (either call order) and the NEXT attempt's
	 * `writeSnapshot`, which explicitly preserves `attempts` across its rewrite
	 * (see above) — the only two writers that touch data.json wholesale.
	 * `finishRun` renders the trail into report.md's "## Attempts" section. */
	appendAttempt(taskId: string, line: string, redact: Redactor): void {
		const dataPath = join(this.runDir(taskId), "data.json");
		let existing: Record<string, unknown> = {};
		if (existsSync(dataPath)) {
			try {
				existing = JSON.parse(readFileSync(dataPath, "utf-8"));
			} catch {}
		}
		const attempts = Array.isArray(existing.attempts) ? existing.attempts : [];
		const merged = { ...existing, attempts: [...attempts, line] };
		writeFileSync(dataPath, redact(JSON.stringify(merged, null, 2)));
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
		// `<provider>/<model>` (spec §5) when the snapshot recorded a provider;
		// bare model id otherwise (older run, or a caller that predates adapters).
		const model = typeof existing.model === "string" ? existing.model : null;
		const provider =
			typeof existing.provider === "string" ? existing.provider : null;
		const modelLine =
			model === null ? "n/a" : provider ? `${provider}/${model}` : model;
		const lines = [
			"# Result",
			"",
			data.result.resultText || "(no result text)",
			"",
			"## Stats",
			`- outcome: ${data.outcome}${data.reason ? ` (${data.reason})` : ""}`,
			`- model: ${modelLine}`,
			`- cost: ${usage.costUsd === null ? "n/a" : `$${usage.costUsd}`}`,
			`- tokens: ${formatTokensLine(usage)}`,
			`- turns: ${usage.turns ?? "n/a"}`,
			`- duration: ${usage.durationMs === null ? "n/a" : `${Math.round(usage.durationMs / 1000)}s`}`,
			"",
		];
		// Fallback-chain hop trail (finding 5), when any hop was recorded via
		// appendAttempt — absent for a task that never fell back (the common case).
		// Read from `existing` (the on-disk snapshot, which `appendAttempt` already
		// wrote the trail into before finishRun runs) not `merged`: the object spread
		// that builds `merged` drops the `Record<string, unknown>` index signature,
		// so `merged.attempts` has no type. `merged` never touches `attempts` anyway,
		// so the two are identical at runtime.
		const attempts = Array.isArray(existing.attempts)
			? (existing.attempts as string[])
			: [];
		if (attempts.length > 0) {
			lines.push("## Attempts", "", ...attempts.map((a) => `- ${a}`), "");
		}
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
	 * the `provider` adapter that produced it, the absolute worktree path the
	 * run executed in, its `started_at`/`finished_at` timestamps (for session
	 * recency), and the originating task's `prompt` matter to callers. Untyped
	 * fields are ignored; malformed files → null.
	 */
	readRunData(taskId: string): {
		session_id?: string | null;
		model?: string | null;
		provider?: string | null;
		resolved_worktree_path?: string | null;
		started_at?: string | null;
		finished_at?: string | null;
		task?: { prompt?: string };
	} | null {
		return this.readRunMeta(taskId) as {
			session_id?: string | null;
			model?: string | null;
			provider?: string | null;
			resolved_worktree_path?: string | null;
			started_at?: string | null;
			finished_at?: string | null;
			task?: { prompt?: string };
		} | null;
	}
}
