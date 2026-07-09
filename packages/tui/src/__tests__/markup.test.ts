import { describe, expect, it } from "vitest";
import { styleLine } from "../markup.js";

describe("styleLine", () => {
	it("bolds a heading and strips the # markers", () => {
		expect(styleLine("## Findings")).toEqual([
			{ text: "Findings", bold: true },
		]);
		expect(styleLine("# Title")).toEqual([{ text: "Title", bold: true }]);
		expect(styleLine("### Deep")).toEqual([{ text: "Deep", bold: true }]);
	});

	it("dims a horizontal rule", () => {
		expect(styleLine("---")).toEqual([{ text: "---", dim: true }]);
	});

	it("returns a single plain segment for plain text", () => {
		expect(styleLine("just some text")).toEqual([{ text: "just some text" }]);
	});

	it("bolds **bold** spans and strips the markers", () => {
		expect(styleLine("see **Full report:** here")).toEqual([
			{ text: "see " },
			{ text: "Full report:", bold: true },
			{ text: " here" },
		]);
	});

	it("colors inline `code` cyan and strips the backticks", () => {
		expect(styleLine("call `foo.py:275` now")).toEqual([
			{ text: "call " },
			{ text: "foo.py:275", color: "cyan" },
			{ text: " now" },
		]);
	});

	it("colors URLs blue", () => {
		expect(styleLine("link https://example.com/x done")).toEqual([
			{ text: "link " },
			{ text: "https://example.com/x", color: "blue" },
			{ text: " done" },
		]);
	});

	it("styles multiple spans in one line", () => {
		expect(styleLine("**Full report:** `pr.md` at https://x.io")).toEqual([
			{ text: "Full report:", bold: true },
			{ text: " " },
			{ text: "pr.md", color: "cyan" },
			{ text: " at " },
			{ text: "https://x.io", color: "blue" },
		]);
	});

	it("returns one segment for an empty line", () => {
		expect(styleLine("")).toEqual([{ text: "" }]);
	});
});
