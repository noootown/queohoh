import type { TaskInstance, TaskStatus } from "@queohoh/core";
import type { StateSnapshot } from "@queohoh/daemon";

const GLYPHS: Record<TaskStatus, string> = {
	running: "▶",
	queued: "○",
	"needs-input": "?",
	done: "✓",
	failed: "✗",
};

export function statusGlyph(status: TaskStatus): string {
	return GLYPHS[status];
}

export function elapsed(sinceIso: string, now: number): string {
	const totalSec = Math.max(0, Math.floor((now - Date.parse(sinceIso)) / 1000));
	const hours = Math.floor(totalSec / 3600);
	const minutes = Math.floor((totalSec % 3600) / 60);
	const seconds = totalSec % 60;
	if (hours > 0) return `${hours}h${String(minutes).padStart(2, "0")}m`;
	if (minutes > 0) return `${minutes}m${String(seconds).padStart(2, "0")}s`;
	return `${seconds}s`;
}

export function promptSummary(prompt: string, width: number): string {
	const line =
		prompt
			.split("\n")
			.find((l) => l.trim().length > 0)
			?.trim() ?? "";
	if (line.length <= width) return line;
	return `${line.slice(0, width - 1)}…`;
}

export interface QueueRow {
	id: string;
	glyph: string;
	/** "⛓ " for tasks resuming the lane's main session, "" otherwise */
	sessionMarker: string;
	lane: string;
	summary: string;
	detail: string;
	kind: "live" | "archived";
}

/**
 * Strip a redundant `<repo>.` prefix from a display name. Worktree directories
 * are named `<repo>.<branch>` by the `wt` tool, but the project/repo name is
 * already shown at the top of the TUI, so repeating it per row is noise. The
 * bare repo (name exactly `<repo>`) and names without the prefix are returned
 * unchanged. Only display strings are stripped — never identifiers used for
 * actions.
 */
export function stripRepoPrefix(name: string, repo: string): string {
	const prefix = `${repo}.`;
	return name.startsWith(prefix) ? name.slice(prefix.length) : name;
}

function laneLabel(task: TaskInstance): string {
	const lane = task.target.worktree ?? task.target.ref;
	return `${task.target.repo}:${stripRepoPrefix(lane, task.target.repo)}`;
}

function sessionMarker(task: TaskInstance): string {
	return task.session === "main" ? "⛓ " : "";
}

export function buildQueueRows(
	snapshot: StateSnapshot,
	now: number,
	width: number,
): QueueRow[] {
	const queuedPosition = new Map<string, number>();

	const liveRows = snapshot.tasks.map((task): QueueRow => {
		let detail: string;
		switch (task.status) {
			case "running":
				detail = `⏱ ${elapsed(task.created, now)}`;
				break;
			case "queued": {
				const lane = laneLabel(task);
				const position = (queuedPosition.get(lane) ?? 0) + 1;
				queuedPosition.set(lane, position);
				detail = `#${position} in lane`;
				break;
			}
			case "needs-input":
			case "failed":
				detail = task.error ?? task.status;
				break;
			case "done":
				detail = "done";
				break;
		}
		return {
			id: task.id,
			glyph: statusGlyph(task.status),
			sessionMarker: sessionMarker(task),
			lane: laneLabel(task),
			summary: promptSummary(task.prompt, width),
			detail,
			kind: "live",
		};
	});

	const archivedRows = snapshot.archivedRecent.slice(-10).map(
		(task): QueueRow => ({
			id: task.id,
			glyph: statusGlyph(task.status),
			sessionMarker: sessionMarker(task),
			lane: laneLabel(task),
			summary: promptSummary(task.prompt, width),
			detail: "archived",
			kind: "archived",
		}),
	);

	return [...liveRows, ...archivedRows];
}
