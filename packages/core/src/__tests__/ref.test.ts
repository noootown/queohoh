import { describe, expect, it } from "vitest";
import { extractTicketId, formatRef, parseRef } from "../ref.js";

describe("parseRef", () => {
	it("parses each kind", () => {
		expect(parseRef("pr:1423")).toEqual({ kind: "pr", number: 1423 });
		expect(parseRef("ticket:JUS-1423")).toEqual({
			kind: "ticket",
			id: "JUS-1423",
		});
		expect(parseRef("worktree:main")).toEqual({
			kind: "worktree",
			name: "main",
		});
		expect(parseRef("temp")).toEqual({ kind: "temp" });
	});

	it("rejects garbage", () => {
		expect(() => parseRef("pr:abc")).toThrow("invalid ref: pr:abc");
		expect(() => parseRef("nonsense")).toThrow("invalid ref: nonsense");
	});

	it("anchors the ticket guard to the full string", () => {
		expect(() => parseRef("ticket:xxJUS-123")).toThrow(
			"invalid ref: ticket:xxJUS-123",
		);
		expect(parseRef("ticket:JUS-123")).toEqual({
			kind: "ticket",
			id: "JUS-123",
		});
	});
});

describe("formatRef", () => {
	it("round-trips", () => {
		for (const raw of ["pr:1423", "ticket:JUS-1423", "worktree:main", "temp"]) {
			expect(formatRef(parseRef(raw))).toBe(raw);
		}
	});
});

describe("extractTicketId", () => {
	it("finds ticket ids in branch names", () => {
		expect(extractTicketId("JUS-1423-fix-auth")).toBe("JUS-1423");
		expect(extractTicketId("feature/ABC2-99")).toBe("ABC2-99");
		expect(extractTicketId("no-ticket-here")).toBeNull();
	});
});
