import { describe, expect, it } from "vitest";
import { insideTmux } from "../tmux.js";

describe("insideTmux", () => {
	it("is true when TMUX is set to a non-empty value", () => {
		expect(insideTmux({ TMUX: "/tmp/tmux-501/default,1234,0" })).toBe(true);
	});

	it("is false when TMUX is unset", () => {
		expect(insideTmux({})).toBe(false);
	});

	it("is false when TMUX is an empty string", () => {
		expect(insideTmux({ TMUX: "" })).toBe(false);
	});
});
