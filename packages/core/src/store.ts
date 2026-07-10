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
	model?: string;
}

export class QueueStore {
	readonly stateDir: string;
	readonly tasksDir: string;
	readonly archiveDir: string;
	invalidFiles: string[] = [];

	private readonly ulid = monotonicFactory();

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
			finishedAt: null,
			source: input.source,
			ephemeralWorktree: false,
			error: null,
			session: input.session ?? "fresh",
			resumeSessionId: input.resumeSessionId ?? null,
			model: input.model ?? null,
			prompt: input.prompt,
		};
		this.write(task);
		return task;
	}

	list(): TaskInstance[] {
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
		return tasks.sort((a, b) => a.id.localeCompare(b.id));
	}

	get(id: string): TaskInstance | undefined {
		try {
			return parseTaskFile(readFileSync(this.taskPath(id), "utf-8"));
		} catch {
			return undefined;
		}
	}

	update(id: string, patch: Partial<Omit<TaskInstance, "id">>): TaskInstance {
		const current = this.get(id);
		if (!current) throw new Error(`task not found: ${id}`);
		const next: TaskInstance = { ...current, ...patch, id };
		// Stamp/clear the completion timestamp on a status transition (unless the
		// caller set finishedAt explicitly): terminal (done/failed) stamps now,
		// keeping an existing stamp so a re-set of the same terminal status is
		// idempotent; any non-terminal status clears it (a re-run un-finishes the
		// task). A patch that doesn't touch status leaves finishedAt untouched.
		if (patch.status !== undefined && !("finishedAt" in patch)) {
			next.finishedAt =
				patch.status === "done" || patch.status === "failed"
					? (current.finishedAt ?? new Date().toISOString())
					: null;
		}
		this.write(next);
		return next;
	}

	archive(id: string): void {
		renameSync(this.taskPath(id), join(this.archiveDir, `${id}.md`));
	}

	listArchived(): TaskInstance[] {
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
		return tasks.sort((a, b) => a.id.localeCompare(b.id));
	}

	private write(task: TaskInstance): void {
		const path = this.taskPath(task.id);
		const tmp = `${path}.tmp`;
		writeFileSync(tmp, serializeTaskFile(task));
		renameSync(tmp, path);
	}
}
