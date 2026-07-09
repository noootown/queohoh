import { describe, expect, it } from "vitest";
import { validateBranchName } from "../branch.js";

describe("validateBranchName", () => {
	it("accepts a plain git-ref-safe name", () => {
		expect(validateBranchName("feature-x")).toBeNull();
		expect(validateBranchName("JUS-1423/fix-auth")).toBeNull();
	});

	it("rejects an empty name", () => {
		expect(validateBranchName("")).toMatch(/required/);
	});

	it("rejects whitespace", () => {
		expect(validateBranchName("fix login")).toMatch(/whitespace/);
		expect(validateBranchName("fix\tlogin")).toMatch(/whitespace/);
	});

	it("rejects '..'", () => {
		expect(validateBranchName("fix..auth")).toMatch(/\.\./);
	});

	it("rejects a leading '-' or '/'", () => {
		expect(validateBranchName("-fix")).toMatch(/start/);
		expect(validateBranchName("/fix")).toMatch(/start/);
	});

	it("rejects a trailing '.lock'", () => {
		expect(validateBranchName("fix.lock")).toMatch(/\.lock/);
	});

	it("rejects non-printable / non-ASCII characters", () => {
		expect(validateBranchName(`fix${String.fromCharCode(1)}`)).toMatch(
			/printable ASCII/,
		);
		expect(validateBranchName("fïx")).toMatch(/printable ASCII/);
	});
});
