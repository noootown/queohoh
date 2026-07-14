import { z } from "zod";
import { parseFrontmatter, stringifyFrontmatter } from "./frontmatter.js";

export const TaskStatusSchema = z.enum([
	"queued",
	"needs-input",
	"running",
	"done",
	"failed",
	// Terminal: a chain member whose predecessor did not succeed. The scheduler
	// never runs it; the engine stamps this status with a reason. Unknown to old
	// TUIs, which tolerate it via serde's `#[serde(other)]` → Unknown fallback.
	"skipped",
	// Terminal: a user cancelled the task (skip RPC on a queued/needs-input task,
	// or stop RPC on a running one). Distinct from `failed` so a deliberate cancel
	// doesn't read as a genuine failure. Same old-TUI tolerance as `skipped`.
	"cancelled",
	// Terminal: the worker exited claiming success (and left a clean tree), but the
	// task's `verify` (done-condition) command disagreed — non-zero exit or timeout.
	// Distinct from `failed` so "the worker errored" reads differently from "the
	// worker claimed success but the check said otherwise". Kebab-cased like
	// `needs-input`; old TUIs fall back to Unknown via serde `#[serde(other)]`.
	"verify-failed",
]);
export type TaskStatus = z.infer<typeof TaskStatusSchema>;

export const PrioritySchema = z.enum(["low", "normal", "high"]);
export type Priority = z.infer<typeof PrioritySchema>;

export const TaskSourceSchema = z.enum(["mcp", "tui", "cron"]);
export type TaskSource = z.infer<typeof TaskSourceSchema>;

export const SessionModeSchema = z.enum(["fresh", "main"]);
export type SessionMode = z.infer<typeof SessionModeSchema>;

const TaskMetaSchema = z
	.object({
		id: z.string().min(1),
		status: TaskStatusSchema,
		definition: z.string().nullable().default(null),
		item: z.record(z.string(), z.string()).nullable().default(null),
		item_key: z.string().nullable().default(null),
		target: z
			.object({
				repo: z.string().min(1),
				ref: z.string().min(1),
				worktree: z.string().nullable().default(null),
			})
			.strict(),
		priority: PrioritySchema.default("normal"),
		created: z.string().min(1),
		// Start timestamp (ISO, like `created`), stamped every time the worker flips
		// the task to `running` — so a RE-run re-stamps it and the live `⏱` timer
		// restarts from the re-run rather than the original creation. Absent on
		// legacy task files that predate the field, or on a task that has never run
		// → null (additive; old rows must not break). Drives the TUI live timer;
		// `created` is the fallback so a stale daemon still shows something sane.
		started_at: z.string().nullable().default(null),
		// Completion timestamp (ISO, like `created`), stamped when the task
		// transitions to a terminal status (done/failed). Absent on legacy task
		// files that predate the field → null (additive; old rows must not break).
		finished_at: z.string().nullable().default(null),
		source: TaskSourceSchema,
		ephemeral_worktree: z.boolean().default(false),
		error: z.string().nullable().default(null),
		session: SessionModeSchema.default("fresh"),
		resume_session_id: z.string().nullable().default(null),
		model: z.string().nullable().default(null),
		// Per-task hard wall-clock ceiling override, in ms (additive; absent on
		// legacy files → null). Set from the MCP `timeout` param (enqueue_task /
		// enqueue_chain); resolution precedence at run time is definition >
		// per-task > daemon default (mirrors `model` immediately above).
		timeout_ms: z.number().nullable().default(null),
		// Task-chain linkage (additive; absent on legacy files → null). Members of
		// one chain share `chain_id`; `chain_seq` is the 0-based position (head =
		// 0). A non-chain task has both null.
		chain_id: z.string().nullable().default(null),
		chain_seq: z.number().int().nonnegative().nullable().default(null),
		// Done-condition (`verify`) fields (additive; absent on legacy files → null).
		// `verify` is the configured shell command the framework runs after the
		// worker claims success (from an ad-hoc/chain-step input, or stamped from the
		// definition when a definition task runs its verify). The rest record the
		// LAST verify attempt: `verified` true/false (null = never verified),
		// `verify_exit_code` (null when it timed out — no exit), and a bounded tail
		// (~4 KB) of the command's combined stdout+stderr in `verify_output`.
		verify: z.string().nullable().default(null),
		verified: z.boolean().nullable().default(null),
		verify_exit_code: z.number().int().nullable().default(null),
		verify_output: z.string().nullable().default(null),
	})
	.strict();

export interface TaskInstance {
	id: string;
	status: TaskStatus;
	definition: string | null;
	item: Record<string, string> | null;
	itemKey: string | null;
	target: { repo: string; ref: string; worktree: string | null };
	priority: Priority;
	created: string;
	/** ISO start timestamp of the current run, re-stamped each time the worker
	 * flips the task to `running`; null when it has never run (or on a legacy
	 * file that predates the field). Drives the live timer (falls back to
	 * `created`). Optional so pre-run callers and test literals need not set it. */
	startedAt?: string | null;
	finishedAt: string | null;
	source: TaskSource;
	ephemeralWorktree: boolean;
	error: string | null;
	session: SessionMode;
	resumeSessionId: string | null;
	model: string | null;
	/** Per-task hard wall-clock ceiling override, in ms; null = fall back to the
	 * definition's `timeout:` (if any) or the daemon default. See
	 * `TaskMetaSchema.timeout_ms`. */
	timeoutMs: number | null;
	prompt: string;
	/** Chain id shared by all members of a task chain; null for a standalone
	 * task. Optional so pre-chain callers and test literals need not set it. */
	chainId?: string | null;
	/** 0-based position within the chain (head = 0); null for a standalone task. */
	chainSeq?: number | null;
	/** Configured done-condition command run after the worker claims success; null
	 * when unset. For a definition task it is stamped from the definition when the
	 * verify runs. Optional so pre-verify callers and test literals need not set it. */
	verify?: string | null;
	/** Result of the last verify: true (passed), false (failed/timed out), or null
	 * (never verified). */
	verified?: boolean | null;
	/** Exit code of the last verify command; null when it timed out or never ran. */
	verifyExitCode?: number | null;
	/** Bounded (~4 KB) tail of the last verify command's combined output; null when
	 * it never ran. */
	verifyOutput?: string | null;
}

export function parseTaskFile(content: string): TaskInstance {
	const { meta, body } = parseFrontmatter(content);
	const m = TaskMetaSchema.parse(meta);
	return {
		id: m.id,
		status: m.status,
		definition: m.definition,
		item: m.item,
		itemKey: m.item_key,
		target: m.target,
		priority: m.priority,
		created: m.created,
		startedAt: m.started_at,
		finishedAt: m.finished_at,
		source: m.source,
		ephemeralWorktree: m.ephemeral_worktree,
		error: m.error,
		session: m.session,
		resumeSessionId: m.resume_session_id,
		model: m.model,
		timeoutMs: m.timeout_ms,
		prompt: body,
		chainId: m.chain_id,
		chainSeq: m.chain_seq,
		verify: m.verify,
		verified: m.verified,
		verifyExitCode: m.verify_exit_code,
		verifyOutput: m.verify_output,
	};
}

export function serializeTaskFile(task: TaskInstance): string {
	const meta = {
		id: task.id,
		status: task.status,
		definition: task.definition,
		item: task.item,
		item_key: task.itemKey,
		target: task.target,
		priority: task.priority,
		created: task.created,
		started_at: task.startedAt ?? null,
		finished_at: task.finishedAt,
		source: task.source,
		ephemeral_worktree: task.ephemeralWorktree,
		error: task.error,
		session: task.session,
		resume_session_id: task.resumeSessionId,
		model: task.model,
		timeout_ms: task.timeoutMs ?? null,
		chain_id: task.chainId ?? null,
		chain_seq: task.chainSeq ?? null,
		verify: task.verify ?? null,
		verified: task.verified ?? null,
		verify_exit_code: task.verifyExitCode ?? null,
		verify_output: task.verifyOutput ?? null,
	};
	return stringifyFrontmatter(meta, task.prompt);
}

export function laneKey(task: TaskInstance): string | null {
	if (task.target.worktree === null) return null;
	return `${task.target.repo}:${task.target.worktree}`;
}
