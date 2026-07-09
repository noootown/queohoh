import { basename } from "node:path";
import { laneKey, type TaskInstance } from "@queohoh/core";
import type { StateSnapshot } from "@queohoh/daemon";
import { buildQueueRows, type QueueRow } from "./format.js";

export interface ProjectTab {
	name: string;
	/** repo seen in tasks but absent from config projects */
	synthetic: boolean;
}

/**
 * Config projects in declared order, then synthetic repos (seen in tasks or
 * archivedRecent but absent from config) sorted alphabetically.
 */
export function buildProjectTabs(snapshot: StateSnapshot): ProjectTab[] {
	const configured = new Set(snapshot.projects.map((p) => p.name));
	const tabs: ProjectTab[] = snapshot.projects.map((p) => ({
		name: p.name,
		synthetic: false,
	}));

	const orphans = new Set<string>();
	for (const task of [...snapshot.tasks, ...snapshot.archivedRecent]) {
		const repo = task.target.repo;
		if (!configured.has(repo)) orphans.add(repo);
	}

	for (const name of [...orphans].sort()) {
		tabs.push({ name, synthetic: true });
	}
	return tabs;
}

/**
 * Rows for a single project's queue: filters tasks/archivedRecent by
 * `target.repo === project`, then delegates to `buildQueueRows`.
 */
export function queueRowsForProject(
	snapshot: StateSnapshot,
	project: string,
	now: number,
	width: number,
): QueueRow[] {
	const scoped: StateSnapshot = {
		...snapshot,
		tasks: snapshot.tasks.filter((t) => t.target.repo === project),
		archivedRecent: snapshot.archivedRecent.filter(
			(t) => t.target.repo === project,
		),
	};
	return buildQueueRows(scoped, now, width);
}

export type WorktreeState = "busy" | "failed" | "free";

export interface WorktreeRow {
	kind: "worktree" | "session";
	/** worktree name, or session label (cwd basename) */
	name: string;
	path: string;
	branch: string | null;
	state: WorktreeState | "you";
	/** lane (`${project}:${name}`) has a stored main session to resume */
	hasMainSession: boolean;
}

function worktreeState(
	snapshot: StateSnapshot,
	project: string,
	name: string,
): WorktreeState {
	const lane = `${project}:${name}`;
	const onLane = snapshot.tasks.filter((t) => laneKey(t) === lane);
	if (onLane.some((t) => t.status === "running")) return "busy";
	// newest by id — ULIDs sort chronologically
	let newest: TaskInstance | null = null;
	for (const task of onLane) {
		if (newest === null || task.id > newest.id) newest = task;
	}
	if (newest !== null && newest.status === "failed") return "failed";
	return "free";
}

function cwdInWorktree(cwd: string, path: string): boolean {
	return cwd === path || cwd.startsWith(`${path}/`);
}

/**
 * One row per `snapshot.worktrees[project]` entry (kind "worktree"), each
 * tagged busy/failed/free by its lane's task activity; then one "session" row
 * (state "you") per interactive session whose cwd is inside a project worktree.
 */
export function buildWorktreeRows(
	snapshot: StateSnapshot,
	project: string,
): WorktreeRow[] {
	const worktrees = snapshot.worktrees[project] ?? [];
	const rows: WorktreeRow[] = worktrees.map((wt) => ({
		kind: "worktree",
		name: wt.name,
		path: wt.path,
		branch: wt.branch,
		state: worktreeState(snapshot, project, wt.name),
		hasMainSession: Object.hasOwn(
			snapshot.mainSessions,
			`${project}:${wt.name}`,
		),
	}));

	for (const session of snapshot.sessions) {
		if (session.kind !== "interactive") continue;
		const cwd = session.cwd;
		if (cwd === null) continue;
		if (!worktrees.some((wt) => cwdInWorktree(cwd, wt.path))) continue;
		rows.push({
			kind: "session",
			name: basename(cwd),
			path: cwd,
			branch: null,
			state: "you",
			hasMainSession: false,
		});
	}
	return rows;
}

/**
 * Slice of `rows` sized ≤ `capacity` that keeps `selected` visible (scroll
 * window); `offset` is the index of the first returned row.
 */
export function windowRows<T>(
	rows: T[],
	selected: number,
	capacity: number,
): { rows: T[]; offset: number } {
	if (capacity <= 0) return { rows: [], offset: 0 };
	if (rows.length <= capacity) return { rows, offset: 0 };
	const clamped = Math.min(Math.max(selected, 0), rows.length - 1);
	let offset = clamped - Math.floor(capacity / 2);
	offset = Math.max(0, Math.min(offset, rows.length - capacity));
	return { rows: rows.slice(offset, offset + capacity), offset };
}
