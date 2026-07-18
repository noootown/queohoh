// packages/core/src/__tests__/env-file.test.ts
import { describe, expect, it } from "vitest";
import { parseEnvFile } from "../env-file.js";

describe("parseEnvFile", () => {
	it("parses KEY=VALUE lines", () => {
		expect(parseEnvFile("A=1\nB=two")).toEqual({ A: "1", B: "two" });
	});

	it("ignores blank lines and # comments", () => {
		expect(parseEnvFile("# comment\n\nA=1\n   \n# another")).toEqual({
			A: "1",
		});
	});

	it("strips a leading `export `", () => {
		expect(parseEnvFile("export A=1")).toEqual({ A: "1" });
	});

	it("strips surrounding single or double quotes", () => {
		expect(parseEnvFile(`A="he llo"\nB='wo rld'`)).toEqual({
			A: "he llo",
			B: "wo rld",
		});
	});

	it("trims trailing whitespace on unquoted values but keeps inner value", () => {
		expect(parseEnvFile("A=abc   ")).toEqual({ A: "abc" });
	});

	it("keeps `=` and `#` characters inside a value", () => {
		expect(parseEnvFile("A=a=b#c")).toEqual({ A: "a=b#c" });
	});

	it("skips malformed lines with no `=` or a bad key", () => {
		expect(parseEnvFile("NOTANASSIGNMENT\n1BAD=x\nGOOD=y")).toEqual({
			GOOD: "y",
		});
	});

	it("later duplicate keys win", () => {
		expect(parseEnvFile("A=1\nA=2")).toEqual({ A: "2" });
	});

	it("returns an empty object for empty input", () => {
		expect(parseEnvFile("")).toEqual({});
	});
});
