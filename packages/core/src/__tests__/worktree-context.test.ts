import { describe, expect, it } from "vitest";
import { extractTicket } from "../worktree-context.js";

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
