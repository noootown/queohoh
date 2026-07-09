import type { TaskInstance } from "@queohoh/core";
import type { WorktreeRow } from "./selectors.js";

export type DetailContext =
	| { kind: "run"; task: TaskInstance }
	| { kind: "definition"; repo: string; name: string }
	| { kind: "worktree"; row: WorktreeRow; laneTasks: TaskInstance[] }
	| { kind: "empty" };

const SUB_TABS: Record<DetailContext["kind"], string[]> = {
	run: ["transcript", "report", "prompt"],
	definition: ["prompt", "config"],
	worktree: ["info"],
	empty: [],
};

/** Ordered sub-tab labels for a context kind (`empty` → `[]`). */
export function subTabsFor(kind: DetailContext["kind"]): string[] {
	return SUB_TABS[kind];
}

/** Clamp a sub-tab index into `[0, count)`; returns 0 when the kind has none. */
export function clampSubTab(
	index: number,
	kind: DetailContext["kind"],
): number {
	const count = SUB_TABS[kind].length;
	if (count === 0) return 0;
	if (index < 0) return 0;
	if (index >= count) return count - 1;
	return index;
}

/**
 * The scroll anchor a detail sub-tab view uses. Only the run transcript (kind
 * "run", sub-tab 0) is bottom-anchored (default view is the live tail); every
 * other view (report, prompt, config, worktree info) is top-anchored.
 */
export function anchorFor(
	kind: DetailContext["kind"],
	subTab: number,
): "top" | "bottom" {
	return kind === "run" && subTab === 0 ? "bottom" : "top";
}

/**
 * A `height`-tall window over `lines`, shifted by `scrollOffset` from its
 * default anchor. `scrollOffset` is the number of lines scrolled away from the
 * default view (0 = default); it is clamped so the window never scrolls past
 * content.
 *
 * - `anchor: "bottom"` — default view is the tail; offset N hides the last N
 *   lines, revealing earlier content (transcript).
 * - `anchor: "top"` — default view is the head; offset N hides the first N
 *   lines (report/prompt/config/info).
 */
export function windowLines(
	lines: string[],
	height: number,
	scrollOffset: number,
	anchor: "top" | "bottom",
): string[] {
	if (height <= 0) return [];
	if (lines.length <= height) return lines;
	const maxOffset = lines.length - height;
	const offset = Math.max(0, Math.min(scrollOffset, maxOffset));
	if (anchor === "bottom") {
		const end = lines.length - offset;
		return lines.slice(end - height, end);
	}
	return lines.slice(offset, offset + height);
}
