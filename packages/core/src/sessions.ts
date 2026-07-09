import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import type { LiveState } from "./scheduler.js";
import type { TaskInstance } from "./task.js";
import { laneKey } from "./task.js";

export interface SessionEntry {
	kind: "worker" | "interactive";
	key: string;
	lane: string | null;
	cwd: string | null;
	pid: number | null;
	startedAt: string;
	heartbeatAt: string;
}

function defaultIsPidAlive(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch {
		return false;
	}
}

export class SessionRegistry {
	private sessions: SessionEntry[] = [];
	private readonly ttlMs: number;
	private readonly isPidAlive: (pid: number) => boolean;

	constructor(
		readonly filePath: string,
		opts?: {
			interactiveTtlMs?: number;
			isPidAlive?: (pid: number) => boolean;
		},
	) {
		this.ttlMs = opts?.interactiveTtlMs ?? 300_000;
		this.isPidAlive = opts?.isPidAlive ?? defaultIsPidAlive;
		if (existsSync(filePath)) {
			try {
				const parsed = JSON.parse(readFileSync(filePath, "utf-8"));
				if (Array.isArray(parsed.sessions)) this.sessions = parsed.sessions;
			} catch {
				this.sessions = [];
			}
		}
	}

	private persist(): void {
		const tmp = `${this.filePath}.tmp`;
		writeFileSync(tmp, JSON.stringify({ sessions: this.sessions }, null, 2));
		renameSync(tmp, this.filePath);
	}

	registerWorker(taskId: string, lane: string, pid: number): void {
		const now = new Date().toISOString();
		this.sessions = this.sessions.filter(
			(s) => !(s.kind === "worker" && s.key === taskId),
		);
		this.sessions.push({
			kind: "worker",
			key: taskId,
			lane,
			cwd: null,
			pid,
			startedAt: now,
			heartbeatAt: now,
		});
		this.persist();
	}

	unregisterWorker(taskId: string): void {
		this.sessions = this.sessions.filter(
			(s) => !(s.kind === "worker" && s.key === taskId),
		);
		this.persist();
	}

	upsertInteractive(cwd: string, pid: number | null): void {
		const now = new Date().toISOString();
		const existing = this.sessions.find(
			(s) => s.kind === "interactive" && s.key === cwd,
		);
		if (existing) {
			existing.heartbeatAt = now;
			existing.pid = pid;
		} else {
			this.sessions.push({
				kind: "interactive",
				key: cwd,
				lane: null,
				cwd,
				pid,
				startedAt: now,
				heartbeatAt: now,
			});
		}
		this.persist();
	}

	removeInteractive(cwd: string): void {
		this.sessions = this.sessions.filter(
			(s) => !(s.kind === "interactive" && s.key === cwd),
		);
		this.persist();
	}

	sweep(now: number = Date.now()): void {
		this.sessions = this.sessions.filter((s) => {
			if (s.kind === "interactive") {
				return now - Date.parse(s.heartbeatAt) < this.ttlMs;
			}
			return s.pid !== null && this.isPidAlive(s.pid);
		});
		this.persist();
	}

	list(): SessionEntry[] {
		return [...this.sessions];
	}
}

export function buildLiveState(
	sessions: SessionEntry[],
	tasks: TaskInstance[],
	laneOfCwd: (cwd: string) => string | null,
): LiveState {
	const running = tasks.filter((t) => t.status === "running");
	const runningLanes = new Set<string>();
	for (const t of running) {
		const lane = laneKey(t);
		if (lane) runningLanes.add(lane);
	}
	const interactiveLanes = new Set<string>();
	for (const s of sessions) {
		if (s.kind === "interactive" && s.cwd) {
			const lane = laneOfCwd(s.cwd);
			if (lane) interactiveLanes.add(lane);
		}
	}
	return { runningLanes, interactiveLanes, runningCount: running.length };
}
