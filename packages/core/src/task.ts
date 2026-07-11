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
		// Task-chain linkage (additive; absent on legacy files → null). Members of
		// one chain share `chain_id`; `chain_seq` is the 0-based position (head =
		// 0). A non-chain task has both null.
		chain_id: z.string().nullable().default(null),
		chain_seq: z.number().int().nonnegative().nullable().default(null),
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
	finishedAt: string | null;
	source: TaskSource;
	ephemeralWorktree: boolean;
	error: string | null;
	session: SessionMode;
	resumeSessionId: string | null;
	model: string | null;
	prompt: string;
	/** Chain id shared by all members of a task chain; null for a standalone
	 * task. Optional so pre-chain callers and test literals need not set it. */
	chainId?: string | null;
	/** 0-based position within the chain (head = 0); null for a standalone task. */
	chainSeq?: number | null;
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
		finishedAt: m.finished_at,
		source: m.source,
		ephemeralWorktree: m.ephemeral_worktree,
		error: m.error,
		session: m.session,
		resumeSessionId: m.resume_session_id,
		model: m.model,
		prompt: body,
		chainId: m.chain_id,
		chainSeq: m.chain_seq,
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
		finished_at: task.finishedAt,
		source: task.source,
		ephemeral_worktree: task.ephemeralWorktree,
		error: task.error,
		session: task.session,
		resume_session_id: task.resumeSessionId,
		model: task.model,
		chain_id: task.chainId ?? null,
		chain_seq: task.chainSeq ?? null,
	};
	return stringifyFrontmatter(meta, task.prompt);
}

export function laneKey(task: TaskInstance): string | null {
	if (task.target.worktree === null) return null;
	return `${task.target.repo}:${task.target.worktree}`;
}
