import { describe, expect, it } from "vitest";
import { contextArgValues, extractTicket } from "../worktree-context.js";

describe("extractTicket", () => {
	it("returns an exact ticket-named branch unchanged", () => {
		expect(extractTicket("JUS-1008")).toBe("JUS-1008");
	});

	it("extracts the ticket from a prefixed slug branch", () => {
		expect(extractTicket("jus-1008-fix-thing")).toBe("JUS-1008");
	});

	it("uppercases a lowercase ticket", () => {
		expect(extractTicket("jus-1008")).toBe("JUS-1008");
	});

	it("returns empty string when the branch has no ticket token", () => {
		expect(extractTicket("main")).toBe("");
		expect(extractTicket("feature/no-number")).toBe("");
		expect(extractTicket("")).toBe("");
	});

	it("returns the first match when several ticket tokens are present", () => {
		expect(extractTicket("jus-1008-then-abc-42")).toBe("JUS-1008");
	});
});

describe("contextArgValues", () => {
	it("maps source/branch/ticket from a ticket-named branch", () => {
		expect(contextArgValues("jus-1008-fix-thing")).toEqual({
			source: "jus-1008-fix-thing",
			branch: "jus-1008-fix-thing",
			ticket: "JUS-1008",
		});
	});

	it("omits ticket when the branch carries no ticket token", () => {
		expect(contextArgValues("feature/no-number")).toEqual({
			source: "feature/no-number",
			branch: "feature/no-number",
		});
	});

	it("returns an empty map for null/undefined/empty branch", () => {
		expect(contextArgValues(null)).toEqual({});
		expect(contextArgValues(undefined)).toEqual({});
		expect(contextArgValues("")).toEqual({});
	});
});
