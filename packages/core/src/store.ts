import {
	mkdirSync,
	readdirSync,
	readFileSync,
	renameSync,
	writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { monotonicFactory } from "ulid";
import type {
	Priority,
	SessionMode,
	TaskInstance,
	TaskSource,
} from "./task.js";
import { parseTaskFile, serializeTaskFile } from "./task.js";

export interface NewTaskInput {
	prompt: string;
	repo: string;
	ref: string;
	source: TaskSource;
	priority?: Priority;
	definition?: string;
	item?: Record<string, string>;
	itemKey?: string;
	session?: SessionMode;
	resumeSessionId?: string;
	/** Requested model(s): a single `provider/label` ref or an ordered fallback
	 * list (see `TaskInstance.model` / `resolveModelChain`). */
	model?: string | string[];
	/** True when `model` is an explicit pick that must run EXACTLY that ref
	 * (see `TaskInstance.modelPinned` / `resolvePinnedModel`). Defaults to
	 * false. Set by TUI dialog picks and by MCP/API when a single-string
	 * `model` is stamped (so active-provider re-head cannot override a
	 * host-session handoff). */
	modelPinned?: boolean;
	/** Per-task hard wall-clock ceiling override, in ms (from the MCP `timeout`
	 * param); a definition task's own `timeout:` still wins at run time
	 * (unlike `model`, which is task-first so operator overrides win). */
	timeoutMs?: number;
	/** Done-condition command run after the worker claims success; a definition
	 * task leaves this unset and uses the definition's own `verify` at run time. */
	verify?: string;
	/** Scheduler-lane override from the definition's `lane:`; see task.ts. */
	lane?: string;
}

/** One step of a task chain. `definition` steps carry a rendered prompt plus the
 * `repo/name` and item for display; `prompt` steps carry only the prompt. Model
 * is stored as the raw `provider/label` ref(s); worker resolves
 * `task.model ?? def?.model` so a stamped override beats the def's list. */
export interface ChainStepInput {
	prompt: string;
	definition?: string;
	item?: Record<string, string>;
	itemKey?: string;
	model?: string | string[];
	/** True when `model` is an explicit single-ref pick that must run EXACTLY
	 * that ref (see `TaskInstance.modelPinned`). Chain-level stamp from
	 * enqueue_chain when a single-string model is given. */
	modelPinned?: boolean;
	/** Chain-wide hard wall-clock ceiling override, in ms (a definition step's
	 * own `timeout:` still wins at run time — def-first, unlike `model`). */
	timeoutMs?: number;
	priority?: Priority;
	/** Per-step done-condition command (a definition step's own `verify` still
	 * wins at run time — def-first, unlike `model`). */
	verify?: string;
	/** Scheduler-lane override from the definition's `lane:`; see task.ts. */
	lane?: string;
}

/** Target + provenance shared by every member of a chain. `resumeSessionId`
 * applies to the head only (steps 2+ are always fresh). */
export interface ChainSharedInput {
	repo: string;
	ref: string;
	source: TaskSource;
	priority?: Priority;
	resumeSessionId?: string;
}

export class QueueStore {
	readonly stateDir: string;
	readonly tasksDir: string;
	readonly archiveDir: string;
	invalidFiles: string[] = [];

	private readonly ulid = monotonicFactory();
	/** In-memory live queue. Invalidated on every write/rename and on `reload()`.
	 * Avoids re-reading hundreds of task files on every 2s broadcast (was the
	 * dominant cost once the queue grew past a few hundred tasks). */
	private liveCache: TaskInstance[] | null = null;
	/** In-memory archive list — same invalidation rules as `liveCache`. */
	private archiveCache: TaskInstance[] | null = null;

	constructor(stateDir: string) {
		this.stateDir = stateDir;
		this.tasksDir = join(stateDir, "tasks");
		this.archiveDir = join(stateDir, "archive");
		mkdirSync(this.tasksDir, { recursive: true });
		mkdirSync(this.archiveDir, { recursive: true });
	}

	taskPath(id: string): string {
		return join(this.tasksDir, `${id}.md`);
	}

	/** Drop both list caches. Call after an external tasks/ mutation (fs.watch)
	 * so the next list() re-reads disk. Writes through this class invalidate
	 * themselves. */
	reload(): void {
		this.liveCache = null;
		this.archiveCache = null;
	}

	create(input: NewTaskInput): TaskInstance {
		const task: TaskInstance = {
			id: this.ulid(),
			status: "queued",
			definition: input.definition ?? null,
			item: input.item ?? null,
			itemKey: input.itemKey ?? null,
			target: { repo: input.repo, ref: input.ref, worktree: null },
			priority: input.priority ?? "normal",
			created: new Date().toISOString(),
			startedAt: null,
			finishedAt: null,
			source: input.source,
			ephemeralWorktree: false,
			error: null,
			session: input.session ?? "fresh",
			resumeSessionId: input.resumeSessionId ?? null,
			model: input.model ?? null,
			modelPinned: input.modelPinned ?? false,
			timeoutMs: input.timeoutMs ?? null,
			prompt: input.prompt,
			chainId: null,
			chainSeq: null,
			verify: input.verify ?? null,
			verified: null,
			verifyExitCode: null,
			verifyOutput: null,
			attemptedModels: [],
			lane: input.lane ?? null,
			notBefore: null,
		};
		this.write(task);
		return task;
	}

	/**
	 * Create an ordered chain of linked tasks in one shot. Every member shares
	 * `chainId` and the target (`repo`/`ref`, worktree unresolved); `chainSeq` is
	 * the 0-based position. All start `queued`; the scheduler runs them in order,
	 * gating each on its predecessor succeeding (see scheduler.ts). Monotonic
	 * ulids keep member ids ascending in creation order. Returns the members
	 * head-first.
	 */
	createChain(
		steps: ChainStepInput[],
		shared: ChainSharedInput,
	): TaskInstance[] {
		const chainId = this.ulid();
		const now = new Date().toISOString();
		const created = steps.map((step, i) => {
			const task: TaskInstance = {
				id: this.ulid(),
				status: "queued",
				definition: step.definition ?? null,
				item: step.item ?? null,
				itemKey: step.itemKey ?? null,
				target: { repo: shared.repo, ref: shared.ref, worktree: null },
				priority: step.priority ?? shared.priority ?? "normal",
				created: now,
				startedAt: null,
				finishedAt: null,
				source: shared.source,
				ephemeralWorktree: false,
				error: null,
				session: "fresh",
				// Resume applies to the head only; later steps are always fresh.
				resumeSessionId: i === 0 ? (shared.resumeSessionId ?? null) : null,
				model: step.model ?? null,
				modelPinned: step.modelPinned ?? false,
				timeoutMs: step.timeoutMs ?? null,
				prompt: step.prompt,
				chainId,
				chainSeq: i,
				verify: step.verify ?? null,
				verified: null,
				verifyExitCode: null,
				verifyOutput: null,
				attemptedModels: [],
				lane: step.lane ?? null,
				notBefore: null,
			};
			this.write(task, { keepCache: true });
			return task;
		});
		// Single invalidate after the batch so intermediate list() callers (none
		// today) don't re-read partial state; write() with keepCache skipped
		// per-file invalidation.
		this.liveCache = null;
		return created;
	}

	list(): TaskInstance[] {
		if (this.liveCache !== null) return this.liveCache;
		this.invalidFiles = [];
		const tasks: TaskInstance[] = [];
		for (const file of readdirSync(this.tasksDir).sort()) {
			if (!file.endsWith(".md")) continue;
			const path = join(this.tasksDir, file);
			try {
				tasks.push(parseTaskFile(readFileSync(path, "utf-8")));
			} catch {
				this.invalidFiles.push(path);
			}
		}
		tasks.sort((a, b) => a.id.localeCompare(b.id));
		this.liveCache = tasks;
		return tasks;
	}

	get(id: string): TaskInstance | undefined {
		// Prefer cache for hot paths (engine tick / scheduler); fall back to disk
		// so a get of a just-written id still works if cache is cold.
		const cached = this.liveCache?.find((t) => t.id === id);
		if (cached) return cached;
		try {
			return parseTaskFile(readFileSync(this.taskPath(id), "utf-8"));
		} catch {
			return undefined;
		}
	}

	/** Live task, or archived if not in the live queue. Used by detail fetches. */
	getAny(id: string): TaskInstance | undefined {
		const live = this.get(id);
		if (live) return live;
		const archived = this.listArchived().find((t) => t.id === id);
		if (archived) return archived;
		try {
			return parseTaskFile(readFileSync(join(this.archiveDir, `${id}.md`), "utf-8"));
		} catch {
			return undefined;
		}
	}

	update(id: string, patch: Partial<Omit<TaskInstance, "id">>): TaskInstance {
		const current = this.get(id);
		if (!current) throw new Error(`task not found: ${id}`);
		const next: TaskInstance = { ...current, ...patch, id };
		// Stamp/clear the completion timestamp on a status transition (unless the
		// caller set finishedAt explicitly): a terminal status (done/failed/
		// cancelled/skipped) stamps now, keeping an existing stamp so a re-set of
		// the same terminal status is idempotent; any non-terminal status clears it
		// (a re-run un-finishes the task). A patch that doesn't touch status leaves
		// finishedAt untouched.
		if (patch.status !== undefined && !("finishedAt" in patch)) {
			const terminal =
				patch.status === "done" ||
				patch.status === "failed" ||
				patch.status === "cancelled" ||
				patch.status === "skipped" ||
				patch.status === "verify-failed";
			next.finishedAt = terminal
				? (current.finishedAt ?? new Date().toISOString())
				: null;
		}
		this.write(next);
		return next;
	}

	archive(id: string): void {
		renameSync(this.taskPath(id), join(this.archiveDir, `${id}.md`));
		this.liveCache = null;
		this.archiveCache = null;
	}

	/** Reverse of `archive`: move the task file back into the live queue. Throws
	 * a task-not-found error (matching the API's `mustGet` wording) when the id
	 * isn't in the archive, so a stale TUI row surfaces a clear message instead
	 * of a raw ENOENT. */
	unarchive(id: string): void {
		try {
			renameSync(join(this.archiveDir, `${id}.md`), this.taskPath(id));
		} catch {
			throw new Error(`task not found in archive: ${id}`);
		}
		this.liveCache = null;
		this.archiveCache = null;
	}

	listArchived(): TaskInstance[] {
		if (this.archiveCache !== null) return this.archiveCache;
		const tasks: TaskInstance[] = [];
		for (const file of readdirSync(this.archiveDir).sort()) {
			if (!file.endsWith(".md")) continue;
			try {
				tasks.push(
					parseTaskFile(readFileSync(join(this.archiveDir, file), "utf-8")),
				);
			} catch {
				// archived junk is ignored silently
			}
		}
		tasks.sort((a, b) => a.id.localeCompare(b.id));
		this.archiveCache = tasks;
		return tasks;
	}

	private write(
		task: TaskInstance,
		opts?: { keepCache?: boolean },
	): void {
		const path = this.taskPath(task.id);
		const tmp = `${path}.tmp`;
		writeFileSync(tmp, serializeTaskFile(task));
		renameSync(tmp, path);
		if (!opts?.keepCache) {
			// Live list is stale; archive is unchanged by a live write.
			this.liveCache = null;
		}
	}
}
