import { mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type {
	Exec,
	GlobalConfig,
	ResolverIO,
	RunResult,
	SessionEntry,
	TaskInstance,
	TaskStatus,
	WorktreeInfo,
} from "@queohoh/core";
import {
	createResolverIO,
	MainSessionStore,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionRegistry,
} from "@queohoh/core";
import { ApiServer, Engine, type StateSnapshot } from "@queohoh/daemon";

export const cleanups: (() => Promise<void> | void)[] = [];

let taskSeq = 0;

/** Pure fixture: a TaskInstance with sensible defaults, overridable per field. */
export function makeTask(
	status: TaskStatus,
	overrides: Partial<TaskInstance> = {},
): TaskInstance {
	taskSeq += 1;
	return {
		id: `01TASK${String(taskSeq).padStart(20, "0")}`,
		status,
		definition: null,
		item: null,
		itemKey: null,
		target: { repo: "platform", ref: "temp", worktree: "wt-a" },
		priority: "normal",
		created: "2026-07-08T10:00:00.000Z",
		source: "tui",
		ephemeralWorktree: false,
		error: null,
		session: "fresh",
		resumeSessionId: null,
		model: null,
		prompt: "do the thing\n",
		...overrides,
	};
}

/** Pure fixture: an interactive session entry. */
export function makeSession(
	overrides: Partial<SessionEntry> = {},
): SessionEntry {
	return {
		kind: "interactive",
		key: "sess-1",
		lane: null,
		cwd: null,
		pid: 1234,
		startedAt: "2026-07-08T10:00:00.000Z",
		heartbeatAt: "2026-07-08T10:00:00.000Z",
		...overrides,
	};
}

/** Pure fixture: a StateSnapshot with empty defaults, overridable per field. */
export function makeSnapshot(
	partial: Partial<StateSnapshot> = {},
): StateSnapshot {
	return {
		tasks: [],
		archivedRecent: [],
		sessions: [],
		running: [],
		maxConcurrent: 1,
		projects: [],
		worktrees: {},
		mainSessions: {},
		...partial,
	};
}

export async function startServer(opts?: {
	worktrees?: WorktreeInfo[];
	execCalls?: { command: string; args: string[] }[];
}) {
	const base = mkdtempSync(join(tmpdir(), "qo-tui-"));
	const repoPath = join(base, "repo");
	mkdirSync(repoPath, { recursive: true });
	const store = new QueueStore(join(base, "state"));
	const runStore = new RunStore(join(base, "runs"));
	const registry = new SessionRegistry(join(base, "sessions.json"));
	const config: GlobalConfig = {
		workspace: join(base, "ws"),
		projects: [{ name: "platform", path: repoPath }],
		maxConcurrentTasks: 1,
		archiveAfterDays: 7,
		vars: {},
	};
	const okResult: RunResult = {
		exitCode: 0,
		timedOut: false,
		sessionId: null,
		resultText: "ok",
		stderr: "",
		usage: { costUsd: 0, turns: 1, durationMs: 1 },
	};
	const exec: Exec = async (command, args) => {
		opts?.execCalls?.push({ command, args });
		return { stdout: "", exitCode: 0 };
	};
	const resolverIO: ResolverIO = {
		listWorktrees: async () => opts?.worktrees ?? [],
		prBranch: async () => null,
		spawnWorktree: async (_r, name) => ({
			name,
			path: `/wt/${name}`,
			branch: name,
		}),
		// Real removal implementation so the recording exec sees the actual
		// force-clean → `wt remove` → `git branch -D` command sequence.
		removeWorktree: createResolverIO(exec).removeWorktree,
	};
	const mainSessions = new MainSessionStore(join(base, "main-sessions.json"));
	const engine = new Engine({
		store,
		runStore,
		registry,
		config,
		resolverIO,
		exec,
		executeClaude: async () => okResult,
		redact: makeRedactor(new Map()),
		mainSessions,
	});
	const server = new ApiServer({
		engine,
		store,
		runStore,
		registry,
		config,
		mainSessions,
		onMutation: () => server.broadcast(),
	});
	const sock = join(base, "d.sock");
	await server.listen(sock);
	cleanups.push(() => server.close());
	return { server, store, engine, sock, base };
}
