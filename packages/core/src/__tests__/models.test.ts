import { describe, expect, it } from "vitest";
import {
	DEFAULT_MODEL_ALIASES,
	effectiveModelTable,
	resolveModel,
} from "../models.js";

describe("resolveModel", () => {
	it("resolves a known alias", () => {
		expect(resolveModel("sonnet", { sonnet: "claude-sonnet-5" })).toBe(
			"claude-sonnet-5",
		);
	});
	it("passes unknown names through untouched (full ids keep working)", () => {
		expect(resolveModel("claude-fable-5", { sonnet: "x" })).toBe(
			"claude-fable-5",
		);
	});
	it("passes through on an empty table", () => {
		expect(resolveModel("opus", {})).toBe("opus");
	});
});

describe("effectiveModelTable", () => {
	it("layers defaults <- global <- project per key", () => {
		const t = effectiveModelTable(
			{ sonnet: "claude-sonnet-4-6" },
			{ opus: "claude-opus-4-7" },
		);
		expect(t.sonnet).toBe("claude-sonnet-4-6"); // global override
		expect(t.opus).toBe("claude-opus-4-7"); // project override wins
		expect(t.fable).toBe(DEFAULT_MODEL_ALIASES.fable); // default inherited
		expect(t.haiku).toBe("claude-haiku-4-5");
	});
	it("project overrides global for the same key", () => {
		const t = effectiveModelTable({ sonnet: "a" }, { sonnet: "b" });
		expect(t.sonnet).toBe("b");
	});
});
