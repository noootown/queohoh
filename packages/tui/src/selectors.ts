import { basename } from "node:path";
import {
	type ArgSpec,
	contextArgValues,
	laneKey,
	type TaskInstance,
} from "@queohoh/core";
import type { StateSnapshot } from "@queohoh/daemon";
import { buildQueueRows, type QueueRow, stripRepoPrefix } from "./format.js";

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
	/** display name: worktree name with the redundant `<repo>.` prefix stripped,
	 * or session label (cwd basename). For display/filter only — never an
	 * identifier sent to the daemon (see `rawName`). */
	name: string;
	/** the untouched worktree identifier (`<repo>.<branch>`), used for every
	 * action dispatched to the daemon — ref building (`worktree:<rawName>`),
	 * lane keys, removal. Session rows have no real worktree, so this mirrors
	 * `name` (never used as an identifier for a session). */
	rawName: string;
	path: string;
	branch: string | null;
	state: WorktreeState | "you";
	/** lane (`${project}:${name}`) has a stored main session to resume */
	hasMainSession: boolean;
	/** queued (not-yet-running) tasks on this worktree's lane; 0 for sessions */
	queued: number;
}

/**
 * Ink color for a worktree row's status dot: green = idle, yellow = active (a
 * task is running, or it's the user's own session), red = the last task failed.
 * Replaces the old trailing state word with a compact colored-dot prefix.
 */
export function worktreeDotColor(state: WorktreeState | "you"): string {
	switch (state) {
		case "free":
			return "green";
		case "busy":
		case "you":
			return "yellow";
		case "failed":
			return "red";
	}
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

function queuedOnLane(
	snapshot: StateSnapshot,
	project: string,
	name: string,
): number {
	const lane = `${project}:${name}`;
	return snapshot.tasks.filter(
		(t) => laneKey(t) === lane && t.status === "queued",
	).length;
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
		// `wt.name` is the underlying identifier (used for state/action lookup);
		// only the displayed name drops the redundant `<repo>.` prefix.
		name: stripRepoPrefix(wt.name, project),
		rawName: wt.name,
		path: wt.path,
		branch: wt.branch,
		state: worktreeState(snapshot, project, wt.name),
		hasMainSession: Object.hasOwn(
			snapshot.mainSessions,
			`${project}:${wt.name}`,
		),
		queued: queuedOnLane(snapshot, project, wt.name),
	}));

	for (const session of snapshot.sessions) {
		if (session.kind !== "interactive") continue;
		const cwd = session.cwd;
		if (cwd === null) continue;
		if (!worktrees.some((wt) => cwdInWorktree(cwd, wt.path))) continue;
		rows.push({
			kind: "session",
			// A session is not a real worktree, so there is no distinct identifier;
			// rawName mirrors the display name (never dispatched as a worktree ref).
			name: stripRepoPrefix(basename(cwd), project),
			rawName: stripRepoPrefix(basename(cwd), project),
			path: cwd,
			branch: null,
			state: "you",
			hasMainSession: false,
			queued: 0,
		});
	}
	return rows;
}

/**
 * Ambient arg prefill for running a task definition off the TASKS pane, derived
 * from whichever worktree row is currently selected in the worktrees pane. The
 * association is ambient (the run wasn't targeted at this worktree), so the
 * caller uses these as an *editable* prefill, not fixed rows. Empty (no prefill)
 * unless the selection is a real worktree row with a branch — a session row has
 * no branch, and the primary checkout's `main`/`master` is never a sensible
 * `source`, so a wrong prefill of it would only invite a wasted run.
 */
export function ambientContextArgValues(
	row: WorktreeRow | undefined,
): Record<string, string> {
	if (row?.kind !== "worktree") return {};
	if (row.branch === "main" || row.branch === "master") return {};
	return contextArgValues(row.branch);
}

/**
 * Branches offered as `source`/`branch` dropdown candidates for an ambient run:
 * every real worktree's branch in row order, minus branchless/session rows and
 * the primary checkout (`main`/`master` is never a sensible source). Mirrors the
 * exclusion `ambientContextArgValues` applies to the selected row.
 */
function branchCandidates(rows: WorktreeRow[]): string[] {
	const out: string[] = [];
	for (const row of rows) {
		if (row.kind !== "worktree") continue;
		const branch = row.branch;
		if (branch === null || branch === "main" || branch === "master") continue;
		out.push(branch);
	}
	return out;
}

export interface AmbientRunArgs {
	/** the def's args, with `source`/`branch` overlaid as worktree-branch enums */
	args: ArgSpec[];
	/** editable prefill for `source`/`branch`/`ticket` from the selected row */
	initial: Record<string, string>;
}

/**
 * Overlay worktree context onto a definition's args for a TASKS-pane (ambient)
 * run. An arg named `source` or `branch` that does not already declare `options`
 * becomes a dropdown of the repo's worktree branches, so the user picks the
 * source instead of typing it; `initial` prefills from the currently-selected
 * worktree row (ArgsForm falls back to the first option when the selection
 * contributes none — main/master/session). With no candidate branches the arg
 * is left as free text. The run is ambient, so nothing here is fixed — the def's
 * own `worktree:` field still governs where it runs, and the daemon never sees
 * these options (the def declares none; submission stays positional).
 */
export function ambientRunArgs(
	defArgs: ArgSpec[],
	rows: WorktreeRow[],
	selected: WorktreeRow | undefined,
): AmbientRunArgs {
	const candidates = branchCandidates(rows);
	const args = defArgs.map((arg) => {
		const named = arg.name === "source" || arg.name === "branch";
		const hasOptions = arg.options !== undefined && arg.options.length > 0;
		if (!named || hasOptions || candidates.length === 0) return arg;
		return { ...arg, options: candidates };
	});
	// `initial` comes straight from the selected row's context: when the branch
	// qualifies it is guaranteed to be among `candidates` (so a valid enum value);
	// when it doesn't, the key is absent and ArgsForm picks the first option.
	return { args, initial: ambientContextArgValues(selected) };
}

export interface PaneLayout {
	queuePaneH: number;
	listPaneH: number;
	queueCap: number;
	listCap: number;
}

/**
 * Fixed heights for the three left-column panes (queue : tasks : worktrees ≈
 * 2:1:1) that sum to `bodyHeight`, plus the row capacity each pane can show
 * (height minus border+title chrome, which is 3 lines). Heights are explicit —
 * not flex-grown — so a pane never balloons past its capped content: with
 * flexGrow the free space left by short panes was redistributed to every pane,
 * stretching the worktrees box well below its last row.
 */
export function computePaneLayout(bodyHeight: number): PaneLayout {
	const listPaneH = Math.max(4, Math.floor(bodyHeight / 4));
	const queuePaneH = Math.max(4, bodyHeight - 2 * listPaneH);
	return {
		queuePaneH,
		listPaneH,
		queueCap: Math.max(1, queuePaneH - 3),
		listCap: Math.max(1, listPaneH - 3),
	};
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

export interface PaneSelection {
	cursor: number;
	anchor: number | null;
}

/**
 * Clamp a selection to `count` visible rows. An emptied pane resets to the
 * origin with no range; a clamped anchor sticks to the last row rather than
 * disappearing, so a filtered-down list keeps a sensible range.
 */
export function clampSelection(
	sel: PaneSelection,
	count: number,
): PaneSelection {
	if (count <= 0) return { cursor: 0, anchor: null };
	const clamp = (n: number) => Math.max(0, Math.min(n, count - 1));
	return {
		cursor: clamp(sel.cursor),
		anchor: sel.anchor === null ? null : clamp(sel.anchor),
	};
}

/** Inclusive [start, end] visible-row span covered by a selection. */
export function selectionRange(sel: PaneSelection): {
	start: number;
	end: number;
} {
	const anchor = sel.anchor ?? sel.cursor;
	return {
		start: Math.min(anchor, sel.cursor),
		end: Math.max(anchor, sel.cursor),
	};
}

/** Case-insensitive substring match; the empty query matches everything. */
export function matchesFilter(name: string, query: string): boolean {
	if (query === "") return true;
	return name.toLowerCase().includes(query.toLowerCase());
}

/**
 * Pane title with the active search filter appended (`QUEUE /foo`); `active`
 * appends a block cursor while the search input has focus. A range selection of
 * more than one row appends `· N selected` before the filter. Bare `base` when
 * there is nothing to show.
 */
export function paneTitle(
	base: string,
	filter: string,
	active: boolean,
	selectedCount = 0,
): string {
	const title =
		selectedCount > 1 ? `${base} · ${selectedCount} selected` : base;
	if (!active && filter === "") return title;
	return `${title} /${filter}${active ? "█" : ""}`;
}
