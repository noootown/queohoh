import type { SessionMode } from "@queohoh/core";

export type PaneId = "queue" | "tasks" | "worktrees" | "detail";
export type ListPaneId = Exclude<PaneId, "detail">;
export type Direction = "up" | "down" | "left" | "right";

export interface KeyInput {
	input: string; // the char from ink useInput
	ctrl: boolean;
	upArrow: boolean;
	downArrow: boolean;
	leftArrow: boolean;
	rightArrow: boolean;
	return: boolean;
}

export type KeymapAction =
	| { type: "quit" }
	| { type: "move-selection"; delta: 1 | -1 }
	| { type: "activate" } // enter on tasks/worktrees; enter on queue = focus detail
	| { type: "focus"; pane: PaneId }
	| { type: "move-focus"; dir: Direction }
	| { type: "switch-tab"; index: number } // 0-based
	| { type: "cycle-tab"; delta: 1 | -1 }
	| { type: "switch-subtab"; index: number } // 0-based
	| { type: "worktree-add"; session: SessionMode }
	| { type: "queue-retry" }
	| { type: "queue-skip" }
	| { type: "queue-worktree" }
	| { type: "scroll"; delta: 1 | -1 }
	| { type: "scroll-edge"; edge: "top" | "bottom" };

export interface KeymapResult {
	prefixArmed: boolean; // new armed state
	action: KeymapAction | null;
}

const DIR_KEYS: Record<string, Direction> = {
	h: "left",
	j: "down",
	k: "up",
	l: "right",
};

function arrowDir(key: KeyInput): Direction | null {
	if (key.upArrow) return "up";
	if (key.downArrow) return "down";
	if (key.leftArrow) return "left";
	if (key.rightArrow) return "right";
	return null;
}

export function handleKey(
	prefixArmed: boolean,
	focus: PaneId,
	key: KeyInput,
): KeymapResult {
	if (key.ctrl && key.input === "s") {
		return { prefixArmed: true, action: null };
	}
	if (prefixArmed) {
		const dir = arrowDir(key) ?? DIR_KEYS[key.input] ?? null;
		if (dir) return { prefixArmed: false, action: { type: "move-focus", dir } };
		if (/^[1-9]$/.test(key.input)) {
			return {
				prefixArmed: false,
				action: { type: "switch-tab", index: Number(key.input) - 1 },
			};
		}
		if (key.input === "n")
			return { prefixArmed: false, action: { type: "cycle-tab", delta: 1 } };
		if (key.input === "p")
			return { prefixArmed: false, action: { type: "cycle-tab", delta: -1 } };
		return { prefixArmed: false, action: null };
	}
	if (key.input === "q")
		return { prefixArmed: false, action: { type: "quit" } };
	if (/^[1-9]$/.test(key.input)) {
		return {
			prefixArmed: false,
			action: { type: "switch-subtab", index: Number(key.input) - 1 },
		};
	}
	const dir = arrowDir(key) ?? DIR_KEYS[key.input] ?? null;
	if (focus === "detail") {
		if (dir === "down") return act({ type: "scroll", delta: 1 });
		if (dir === "up") return act({ type: "scroll", delta: -1 });
		if (key.input === "g") return act({ type: "scroll-edge", edge: "top" });
		if (key.input === "G") return act({ type: "scroll-edge", edge: "bottom" });
		return { prefixArmed: false, action: null };
	}
	if (dir === "down") return act({ type: "move-selection", delta: 1 });
	if (dir === "up") return act({ type: "move-selection", delta: -1 });
	if (focus === "queue") {
		if (key.return) return act({ type: "focus", pane: "detail" });
		if (key.input === "r") return act({ type: "queue-retry" });
		if (key.input === "s") return act({ type: "queue-skip" });
		if (key.input === "w") return act({ type: "queue-worktree" });
	}
	if (focus === "tasks" && key.return) return act({ type: "activate" });
	if (focus === "worktrees") {
		if (key.input === "f")
			return act({ type: "worktree-add", session: "fresh" });
		if (key.input === "m")
			return act({ type: "worktree-add", session: "main" });
		if (key.return || key.input === "t") return act({ type: "activate" });
	}
	return { prefixArmed: false, action: null };
}

function act(action: KeymapAction): KeymapResult {
	return { prefixArmed: false, action };
}

const COLUMN_ORDER: ListPaneId[] = ["queue", "tasks", "worktrees"];

export function moveFocus(
	current: PaneId,
	dir: Direction,
	lastListPane: ListPaneId,
): PaneId {
	if (current === "detail") {
		return dir === "left" ? lastListPane : "detail";
	}
	if (dir === "right") return "detail";
	if (dir === "left") return current;
	const idx = COLUMN_ORDER.indexOf(current);
	const next = dir === "down" ? idx + 1 : idx - 1;
	const clamped = Math.min(COLUMN_ORDER.length - 1, Math.max(0, next));
	return COLUMN_ORDER[clamped] ?? current;
}
