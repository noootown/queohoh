import { describe, expect, it } from "vitest";
import { render } from "../template.js";

describe("render", () => {
	it("substitutes {{key}} from merged vars", () => {
		expect(render("pr:{{number}}", {}, {}, { number: "257" })).toBe("pr:257");
	});

	it("applies precedence global < repo < item < reserved", () => {
		expect(
			render("{{v}}", { v: "g" }, { v: "r" }, { v: "i" }, { v: "reserved" }),
		).toBe("reserved");
		expect(render("{{v}}", { v: "g" }, { v: "r" })).toBe("r");
	});

	it("leaves unknown keys verbatim", () => {
		expect(render("hi {{nope}}", { v: "x" })).toBe("hi {{nope}}");
	});
});
