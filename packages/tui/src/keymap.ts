export type PaneId = "queue" | "tasks" | "worktrees" | "detail";
export type ListPaneId = Exclude<PaneId, "detail">;
export type Direction = "up" | "down" | "left" | "right";

export interface KeyInput {
	input: string; // the char from ink useInput
	ctrl: boolean;
	shift: boolean;
	upArrow: boolean;
	downArrow: boolean;
	leftArrow: boolean;
	rightArrow: boolean;
	return: boolean;
	escape: boolean;
}

export type KeymapAction =
	| { type: "quit" }
	| { type: "move-selection"; delta: 1 | -1 }
	| { type: "extend-selection"; delta: 1 | -1 }
	| { type: "focus"; pane: PaneId }
	| { type: "move-focus"; dir: Direction }
	| { type: "switch-tab"; index: number } // 0-based
	| { type: "cycle-tab"; delta: 1 | -1 }
	| { type: "switch-subtab"; index: number } // 0-based
	| { type: "open-action-menu" }
	| { type: "create" }
	| { type: "scroll"; delta: 1 | -1 }
	| { type: "scroll-edge"; edge: "top" | "bottom" }
	| { type: "open-search" }
	| { type: "clear-search" };

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
	if (key.input === "a") return act({ type: "open-action-menu" });
	if (key.input === "c") return act({ type: "create" });
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
	if (key.input === "/") return act({ type: "open-search" });
	if (key.escape) return act({ type: "clear-search" });
	if (key.shift && dir === "down")
		return act({ type: "extend-selection", delta: 1 });
	if (key.shift && dir === "up")
		return act({ type: "extend-selection", delta: -1 });
	if (key.input === "J") return act({ type: "extend-selection", delta: 1 });
	if (key.input === "K") return act({ type: "extend-selection", delta: -1 });
	if (dir === "down") return act({ type: "move-selection", delta: 1 });
	if (dir === "up") return act({ type: "move-selection", delta: -1 });
	if (key.return) return act({ type: "focus", pane: "detail" });
	return { prefixArmed: false, action: null };
}

function act(action: KeymapAction): KeymapResult {
	return { prefixArmed: false, action };
}

// Single source of truth for the SGR mouse-report shape so `isMouseEvent` and
// `parseMouseWheel` can never drift. Terminals emit `ESC [ < btn ; col ; row
// (M|m)` in SGR mode (enabled by `\x1b[?1006h`); ink delivers this as one
// keypress with the leading ESC stripped, so we accept an optional `ESC[` / `[`
// prefix. The capture group is the button code.
// biome-ignore lint/suspicious/noControlCharactersInRegex: ESC (\x1b) is the literal first byte of an SGR mouse report; matching it is the point
const SGR_MOUSE_RE = /^(?:\x1b)?\[<(\d+);\d+;\d+[Mm]/;

/**
 * True when `input` is ANY SGR mouse report — wheel, click press/release, or
 * motion. Mouse tracking (enabled in alt-screen.ts) makes the terminal emit
 * these on every click/drag; they must be swallowed so they never leak into
 * `handleKey`, the search box, or a text field. A fresh RegExp per call avoids
 * shared-`lastIndex` state (SGR_MOUSE_RE has no `g` flag, but this keeps it
 * obviously reentrant).
 */
export function isMouseEvent(input: string): boolean {
	return SGR_MOUSE_RE.test(input);
}

/**
 * Detect an SGR mouse wheel report and return its direction, or `null` for any
 * non-wheel input (including non-wheel mouse reports like clicks). Wheel buttons
 * set bit 6 (64): 64 = up, 65 = down; higher bits carry modifier keys we ignore.
 * Column/row coordinates are ignored — scrolling targets the focused pane, not
 * the pane under the cursor.
 */
export function parseMouseWheel(input: string): "up" | "down" | null {
	const match = SGR_MOUSE_RE.exec(input);
	if (!match) return null;
	const button = Number(match[1]);
	if ((button & 0b100_0000) === 0) return null; // not a wheel event
	return button & 1 ? "down" : "up";
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
