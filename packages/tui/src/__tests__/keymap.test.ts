import { describe, expect, it } from "vitest";
import {
	handleKey,
	isMouseEvent,
	type KeyInput,
	type ListPaneId,
	moveFocus,
	type PaneId,
	parseMouseWheel,
} from "../keymap.js";

function key(overrides: Partial<KeyInput> = {}): KeyInput {
	return {
		input: "",
		ctrl: false,
		upArrow: false,
		downArrow: false,
		leftArrow: false,
		rightArrow: false,
		return: false,
		escape: false,
		...overrides,
	};
}

const LIST_PANES: PaneId[] = ["queue", "tasks", "worktrees"];
const ALL_PANES: PaneId[] = ["queue", "tasks", "worktrees", "detail"];

describe("handleKey — ctrl+s prefix arming", () => {
	it("arms the prefix on ctrl+s from any focus, emitting no action", () => {
		for (const focus of ALL_PANES) {
			const res = handleKey(false, focus, key({ ctrl: true, input: "s" }));
			expect(res).toEqual({ prefixArmed: true, action: null });
		}
	});

	it("re-arms on ctrl+s even when already armed", () => {
		const res = handleKey(true, "queue", key({ ctrl: true, input: "s" }));
		expect(res).toEqual({ prefixArmed: true, action: null });
	});

	it("disarms on any non-dispatching second key", () => {
		const res = handleKey(true, "queue", key({ input: "z" }));
		expect(res).toEqual({ prefixArmed: false, action: null });
	});
});

describe("handleKey — armed dispatch", () => {
	it("armed + hjkl → move-focus in the right dir", () => {
		expect(handleKey(true, "queue", key({ input: "h" }))).toEqual({
			prefixArmed: false,
			action: { type: "move-focus", dir: "left" },
		});
		expect(handleKey(true, "queue", key({ input: "j" }))).toEqual({
			prefixArmed: false,
			action: { type: "move-focus", dir: "down" },
		});
		expect(handleKey(true, "queue", key({ input: "k" }))).toEqual({
			prefixArmed: false,
			action: { type: "move-focus", dir: "up" },
		});
		expect(handleKey(true, "queue", key({ input: "l" }))).toEqual({
			prefixArmed: false,
			action: { type: "move-focus", dir: "right" },
		});
	});

	it("armed + arrows → move-focus in the right dir", () => {
		expect(handleKey(true, "queue", key({ upArrow: true }))).toEqual({
			prefixArmed: false,
			action: { type: "move-focus", dir: "up" },
		});
		expect(handleKey(true, "queue", key({ downArrow: true }))).toEqual({
			prefixArmed: false,
			action: { type: "move-focus", dir: "down" },
		});
		expect(handleKey(true, "queue", key({ leftArrow: true }))).toEqual({
			prefixArmed: false,
			action: { type: "move-focus", dir: "left" },
		});
		expect(handleKey(true, "queue", key({ rightArrow: true }))).toEqual({
			prefixArmed: false,
			action: { type: "move-focus", dir: "right" },
		});
	});

	it("armed + 1..9 → switch-tab with 0-based index", () => {
		for (let n = 1; n <= 9; n += 1) {
			expect(handleKey(true, "tasks", key({ input: String(n) }))).toEqual({
				prefixArmed: false,
				action: { type: "switch-tab", index: n - 1 },
			});
		}
	});

	it("armed + n/p → cycle-tab ±1", () => {
		expect(handleKey(true, "tasks", key({ input: "n" }))).toEqual({
			prefixArmed: false,
			action: { type: "cycle-tab", delta: 1 },
		});
		expect(handleKey(true, "tasks", key({ input: "p" }))).toEqual({
			prefixArmed: false,
			action: { type: "cycle-tab", delta: -1 },
		});
	});

	it("armed + other → no action, disarmed", () => {
		expect(handleKey(true, "tasks", key({ input: "x" }))).toEqual({
			prefixArmed: false,
			action: null,
		});
		expect(handleKey(true, "tasks", key({ input: "0" }))).toEqual({
			prefixArmed: false,
			action: null,
		});
		expect(handleKey(true, "tasks", key({ return: true }))).toEqual({
			prefixArmed: false,
			action: null,
		});
	});
});

describe("handleKey — unprefixed global keys", () => {
	it("q → quit from every pane", () => {
		for (const focus of ALL_PANES) {
			expect(handleKey(false, focus, key({ input: "q" }))).toEqual({
				prefixArmed: false,
				action: { type: "quit" },
			});
		}
	});

	it("digits 1..9 → switch-subtab with 0-based index", () => {
		for (let n = 1; n <= 9; n += 1) {
			expect(handleKey(false, "tasks", key({ input: String(n) }))).toEqual({
				prefixArmed: false,
				action: { type: "switch-subtab", index: n - 1 },
			});
		}
	});
});

describe("handleKey — queue focus", () => {
	it("j / downArrow → move-selection +1", () => {
		expect(handleKey(false, "queue", key({ input: "j" }))).toEqual({
			prefixArmed: false,
			action: { type: "move-selection", delta: 1 },
		});
		expect(handleKey(false, "queue", key({ downArrow: true }))).toEqual({
			prefixArmed: false,
			action: { type: "move-selection", delta: 1 },
		});
	});

	it("k / upArrow → move-selection -1", () => {
		expect(handleKey(false, "queue", key({ input: "k" }))).toEqual({
			prefixArmed: false,
			action: { type: "move-selection", delta: -1 },
		});
		expect(handleKey(false, "queue", key({ upArrow: true }))).toEqual({
			prefixArmed: false,
			action: { type: "move-selection", delta: -1 },
		});
	});

	it("r/s/w are unbound (moved to the action menu)", () => {
		for (const input of ["r", "s", "w"]) {
			expect(handleKey(false, "queue", key({ input }))).toEqual({
				prefixArmed: false,
				action: null,
			});
		}
	});

	it("return → focus detail", () => {
		expect(handleKey(false, "queue", key({ return: true }))).toEqual({
			prefixArmed: false,
			action: { type: "focus", pane: "detail" },
		});
	});
});

describe("handleKey — tasks focus", () => {
	it("selection keys move selection", () => {
		expect(handleKey(false, "tasks", key({ input: "j" }))).toEqual({
			prefixArmed: false,
			action: { type: "move-selection", delta: 1 },
		});
		expect(handleKey(false, "tasks", key({ input: "k" }))).toEqual({
			prefixArmed: false,
			action: { type: "move-selection", delta: -1 },
		});
	});

	it("return → focus detail", () => {
		expect(handleKey(false, "tasks", key({ return: true }))).toEqual({
			prefixArmed: false,
			action: { type: "focus", pane: "detail" },
		});
	});

	it("does not treat queue-only keys (r/s/w) as actions", () => {
		for (const input of ["r", "s", "w"]) {
			expect(handleKey(false, "tasks", key({ input }))).toEqual({
				prefixArmed: false,
				action: null,
			});
		}
	});
});

describe("handleKey — action menu", () => {
	it("a → open-action-menu from every pane", () => {
		for (const focus of ALL_PANES) {
			expect(handleKey(false, focus, key({ input: "a" }))).toEqual({
				prefixArmed: false,
				action: { type: "open-action-menu" },
			});
		}
	});
});

describe("handleKey — create", () => {
	it("c → create from every pane (dispatch routes it by pane)", () => {
		for (const focus of ALL_PANES) {
			expect(handleKey(false, focus, key({ input: "c" }))).toEqual({
				prefixArmed: false,
				action: { type: "create" },
			});
		}
	});

	it("c is not a create action while the prefix is armed", () => {
		expect(handleKey(true, "worktrees", key({ input: "c" }))).toEqual({
			prefixArmed: false,
			action: null,
		});
	});
});

describe("handleKey — worktrees focus", () => {
	it("selection keys move selection", () => {
		expect(handleKey(false, "worktrees", key({ input: "j" }))).toEqual({
			prefixArmed: false,
			action: { type: "move-selection", delta: 1 },
		});
		expect(handleKey(false, "worktrees", key({ input: "k" }))).toEqual({
			prefixArmed: false,
			action: { type: "move-selection", delta: -1 },
		});
	});

	it("return → focus detail", () => {
		expect(handleKey(false, "worktrees", key({ return: true }))).toEqual({
			prefixArmed: false,
			action: { type: "focus", pane: "detail" },
		});
	});

	it("f/m/t are unbound (moved to the action menu)", () => {
		for (const input of ["f", "m", "t"]) {
			expect(handleKey(false, "worktrees", key({ input }))).toEqual({
				prefixArmed: false,
				action: null,
			});
		}
	});
});

describe("handleKey — detail focus", () => {
	it("j / downArrow → scroll +1", () => {
		expect(handleKey(false, "detail", key({ input: "j" }))).toEqual({
			prefixArmed: false,
			action: { type: "scroll", delta: 1 },
		});
		expect(handleKey(false, "detail", key({ downArrow: true }))).toEqual({
			prefixArmed: false,
			action: { type: "scroll", delta: 1 },
		});
	});

	it("k / upArrow → scroll -1", () => {
		expect(handleKey(false, "detail", key({ input: "k" }))).toEqual({
			prefixArmed: false,
			action: { type: "scroll", delta: -1 },
		});
		expect(handleKey(false, "detail", key({ upArrow: true }))).toEqual({
			prefixArmed: false,
			action: { type: "scroll", delta: -1 },
		});
	});

	it("g → scroll-edge top", () => {
		expect(handleKey(false, "detail", key({ input: "g" }))).toEqual({
			prefixArmed: false,
			action: { type: "scroll-edge", edge: "top" },
		});
	});

	it("G → scroll-edge bottom", () => {
		expect(handleKey(false, "detail", key({ input: "G" }))).toEqual({
			prefixArmed: false,
			action: { type: "scroll-edge", edge: "bottom" },
		});
	});

	it("unhandled key → no action", () => {
		expect(handleKey(false, "detail", key({ input: "z" }))).toEqual({
			prefixArmed: false,
			action: null,
		});
		expect(handleKey(false, "detail", key({ return: true }))).toEqual({
			prefixArmed: false,
			action: null,
		});
	});
});

describe("handleKey — search", () => {
	it("/ → open-search on list panes", () => {
		for (const focus of LIST_PANES) {
			expect(handleKey(false, focus, key({ input: "/" }))).toEqual({
				prefixArmed: false,
				action: { type: "open-search" },
			});
		}
	});

	it("/ is a no-op on detail", () => {
		expect(handleKey(false, "detail", key({ input: "/" }))).toEqual({
			prefixArmed: false,
			action: null,
		});
	});

	it("esc → clear-search on list panes, no-op on detail", () => {
		for (const focus of LIST_PANES) {
			expect(handleKey(false, focus, key({ escape: true }))).toEqual({
				prefixArmed: false,
				action: { type: "clear-search" },
			});
		}
		expect(handleKey(false, "detail", key({ escape: true }))).toEqual({
			prefixArmed: false,
			action: null,
		});
	});
});

describe("moveFocus — geometry", () => {
	it("queue ↓ → tasks ↓ → worktrees, clamped at ends", () => {
		expect(moveFocus("queue", "down", "queue")).toBe("tasks");
		expect(moveFocus("tasks", "down", "tasks")).toBe("worktrees");
		expect(moveFocus("worktrees", "down", "worktrees")).toBe("worktrees");
	});

	it("worktrees ↑ → tasks ↑ → queue, clamped at top", () => {
		expect(moveFocus("worktrees", "up", "worktrees")).toBe("tasks");
		expect(moveFocus("tasks", "up", "tasks")).toBe("queue");
		expect(moveFocus("queue", "up", "queue")).toBe("queue");
	});

	it("any list pane + right → detail", () => {
		for (const pane of LIST_PANES) {
			expect(moveFocus(pane, "right", pane as ListPaneId)).toBe("detail");
		}
	});

	it("list pane + left stays put", () => {
		for (const pane of LIST_PANES) {
			expect(moveFocus(pane, "left", pane as ListPaneId)).toBe(pane);
		}
	});

	it("detail + left → lastListPane", () => {
		expect(moveFocus("detail", "left", "queue")).toBe("queue");
		expect(moveFocus("detail", "left", "tasks")).toBe("tasks");
		expect(moveFocus("detail", "left", "worktrees")).toBe("worktrees");
	});

	it("detail + up/down/right stays detail", () => {
		expect(moveFocus("detail", "up", "tasks")).toBe("detail");
		expect(moveFocus("detail", "down", "tasks")).toBe("detail");
		expect(moveFocus("detail", "right", "tasks")).toBe("detail");
	});
});

describe("parseMouseWheel", () => {
	// ink strips the leading ESC before it reaches useInput, so the common case
	// is the bare `[<btn;col;row M` form.
	it("maps SGR wheel-up (button 64) to up", () => {
		expect(parseMouseWheel("[<64;10;5M")).toBe("up");
	});

	it("maps SGR wheel-down (button 65) to down", () => {
		expect(parseMouseWheel("[<65;10;5M")).toBe("down");
	});

	it("accepts an optional leading ESC and the release (m) final byte", () => {
		expect(parseMouseWheel("\x1b[<64;1;1M")).toBe("up");
		expect(parseMouseWheel("[<65;200;48m")).toBe("down");
	});

	it("ignores modifier bits above the wheel bit (68 = wheel-up + ctrl)", () => {
		expect(parseMouseWheel("[<68;10;5M")).toBe("up");
		expect(parseMouseWheel("[<69;10;5M")).toBe("down");
	});

	it("returns null for non-wheel mouse buttons (0 = left click)", () => {
		expect(parseMouseWheel("[<0;10;5M")).toBeNull();
		expect(parseMouseWheel("[<2;10;5M")).toBeNull();
	});

	it("returns null for ordinary key input", () => {
		expect(parseMouseWheel("q")).toBeNull();
		expect(parseMouseWheel("")).toBeNull();
		expect(parseMouseWheel("j")).toBeNull();
	});
});

describe("isMouseEvent", () => {
	it("matches wheel reports (press M and release m, with/without ESC prefix)", () => {
		expect(isMouseEvent("[<64;10;5M")).toBe(true);
		expect(isMouseEvent("[<65;10;5m")).toBe(true);
		expect(isMouseEvent("\x1b[<64;1;1M")).toBe(true);
	});

	it("matches non-wheel mouse reports: click press (0/M), release (0/m), motion", () => {
		expect(isMouseEvent("[<0;34;12M")).toBe(true); // left-button press
		expect(isMouseEvent("[<0;34;12m")).toBe(true); // left-button release
		expect(isMouseEvent("\x1b[<0;34;12m")).toBe(true); // release, ESC-prefixed
		expect(isMouseEvent("[<35;80;24M")).toBe(true); // drag/motion
	});

	it("is false for ordinary key input and empty strings", () => {
		expect(isMouseEvent("q")).toBe(false);
		expect(isMouseEvent("")).toBe(false);
		expect(isMouseEvent("j")).toBe(false);
		expect(isMouseEvent("\r")).toBe(false);
		expect(isMouseEvent("[A")).toBe(false); // arrow key CSI, not a mouse report
	});
});
