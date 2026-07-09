import { describe, expect, it } from "vitest";
import { anchorFor, clampSubTab, subTabsFor, windowLines } from "../detail.js";

describe("anchorFor", () => {
	it("bottom-anchors only the run transcript (sub-tab 0)", () => {
		expect(anchorFor("run", 0)).toBe("bottom");
	});

	it("top-anchors the other run sub-tabs (report, prompt)", () => {
		expect(anchorFor("run", 1)).toBe("top");
		expect(anchorFor("run", 2)).toBe("top");
	});

	it("top-anchors every non-run view regardless of sub-tab", () => {
		expect(anchorFor("definition", 0)).toBe("top");
		expect(anchorFor("definition", 1)).toBe("top");
		expect(anchorFor("worktree", 0)).toBe("top");
		expect(anchorFor("empty", 0)).toBe("top");
	});
});

describe("subTabsFor", () => {
	it("returns the sub-tabs per context kind", () => {
		expect(subTabsFor("run")).toEqual(["transcript", "report", "prompt"]);
		expect(subTabsFor("definition")).toEqual(["prompt", "config"]);
		expect(subTabsFor("worktree")).toEqual(["info"]);
		expect(subTabsFor("empty")).toEqual([]);
	});
});

describe("clampSubTab", () => {
	it("clamps an index into range for the kind", () => {
		expect(clampSubTab(-1, "run")).toBe(0);
		expect(clampSubTab(0, "run")).toBe(0);
		expect(clampSubTab(2, "run")).toBe(2);
		expect(clampSubTab(5, "run")).toBe(2);
		expect(clampSubTab(3, "definition")).toBe(1);
		expect(clampSubTab(1, "worktree")).toBe(0);
	});

	it("returns 0 for the empty kind with no sub-tabs", () => {
		expect(clampSubTab(0, "empty")).toBe(0);
		expect(clampSubTab(4, "empty")).toBe(0);
	});
});

describe("windowLines", () => {
	const lines = ["a", "b", "c", "d", "e"];

	it("returns all lines when they fit within height", () => {
		expect(windowLines(lines, 10, 0, "top")).toEqual(lines);
		expect(windowLines(lines, 10, 3, "bottom")).toEqual(lines);
	});

	it("returns empty for non-positive height", () => {
		expect(windowLines(lines, 0, 0, "top")).toEqual([]);
		expect(windowLines(lines, -2, 0, "bottom")).toEqual([]);
	});

	it("top anchor shows the first height lines by default", () => {
		expect(windowLines(lines, 2, 0, "top")).toEqual(["a", "b"]);
	});

	it("top anchor offset N hides the first N lines", () => {
		expect(windowLines(lines, 2, 2, "top")).toEqual(["c", "d"]);
	});

	it("bottom anchor shows the last height lines by default", () => {
		expect(windowLines(lines, 2, 0, "bottom")).toEqual(["d", "e"]);
	});

	it("bottom anchor offset N hides the last N lines", () => {
		expect(windowLines(lines, 2, 1, "bottom")).toEqual(["c", "d"]);
	});

	it("clamps offset so the window never scrolls past content", () => {
		expect(windowLines(lines, 2, 99, "top")).toEqual(["d", "e"]);
		expect(windowLines(lines, 2, 99, "bottom")).toEqual(["a", "b"]);
	});
});
