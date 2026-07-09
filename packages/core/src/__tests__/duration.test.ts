import { describe, expect, it } from "vitest";
import { parseDuration } from "../duration.js";

describe("parseDuration", () => {
	it("parses s/m/h/d", () => {
		expect(parseDuration("45s")).toBe(45_000);
		expect(parseDuration("30m")).toBe(1_800_000);
		expect(parseDuration("2h")).toBe(7_200_000);
		expect(parseDuration("7d")).toBe(604_800_000);
	});

	it("rejects garbage", () => {
		for (const bad of ["", "30", "m30", "30x", "-5m"]) {
			expect(() => parseDuration(bad)).toThrow(`invalid duration: ${bad}`);
		}
	});
});
