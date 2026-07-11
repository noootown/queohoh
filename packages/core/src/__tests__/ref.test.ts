import { describe, expect, it } from "vitest";
import { extractRef, extractTicketId, formatRef, parseRef } from "../ref.js";

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
		expect(parseRef("repo")).toEqual({ kind: "repo" });
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

	it("accepts a bare ticket id", () => {
		expect(parseRef("JUS-1821")).toEqual({ kind: "ticket", id: "JUS-1821" });
		expect(parseRef("ABC2-99")).toEqual({ kind: "ticket", id: "ABC2-99" });
	});

	it("accepts #N as a PR ref", () => {
		expect(parseRef("#123")).toEqual({ kind: "pr", number: 123 });
	});

	it("parses GitHub PR URLs including trailing path/query/fragment", () => {
		const want = { kind: "pr", number: 1821 };
		expect(parseRef("https://github.com/acme/widgets/pull/1821")).toEqual(want);
		expect(parseRef("http://github.com/acme/widgets/pull/1821")).toEqual(want);
		expect(parseRef("github.com/acme/widgets/pull/1821")).toEqual(want);
		expect(parseRef("https://github.com/acme/widgets/pull/1821/files")).toEqual(
			want,
		);
		expect(
			parseRef("https://github.com/acme/widgets/pull/1821?diff=split"),
		).toEqual(want);
		expect(
			parseRef("https://github.com/acme/widgets/pull/1821#discussion_r1"),
		).toEqual(want);
	});

	it("parses Linear issue URLs into the ticket id", () => {
		const want = { kind: "ticket", id: "JUS-1821" };
		expect(parseRef("https://linear.app/justicebid/issue/JUS-1821")).toEqual(
			want,
		);
		expect(
			parseRef("https://linear.app/justicebid/issue/JUS-1821-fix-the-thing"),
		).toEqual(want);
		expect(
			parseRef("linear.app/justicebid/issue/JUS-1821-slug?foo=bar#c1"),
		).toEqual(want);
	});

	it("trims surrounding whitespace before parsing", () => {
		expect(parseRef("  pr:1423  ")).toEqual({ kind: "pr", number: 1423 });
		expect(parseRef("\tJUS-1821\n")).toEqual({
			kind: "ticket",
			id: "JUS-1821",
		});
		expect(parseRef("  https://github.com/acme/widgets/pull/1821  ")).toEqual({
			kind: "pr",
			number: 1821,
		});
	});

	it("still rejects bare numbers and other garbage", () => {
		expect(() => parseRef("123")).toThrow("invalid ref: 123");
		expect(() => parseRef("https://github.com/acme/widgets")).toThrow(
			"invalid ref: https://github.com/acme/widgets",
		);
		expect(() =>
			parseRef("https://linear.app/justicebid/issue/no-ticket"),
		).toThrow("invalid ref: https://linear.app/justicebid/issue/no-ticket");
	});
});

describe("formatRef", () => {
	it("round-trips", () => {
		for (const raw of [
			"pr:1423",
			"ticket:JUS-1423",
			"worktree:main",
			"temp",
			"repo",
		]) {
			expect(formatRef(parseRef(raw))).toBe(raw);
		}
	});
});

describe("extractRef", () => {
	it("finds a GitHub PR URL anywhere in prose", () => {
		expect(
			extractRef(
				"please look at https://github.com/acme/widgets/pull/1821 now",
			),
		).toEqual({ kind: "pr", number: 1821 });
		expect(
			extractRef("scheme-less github.com/acme/widgets/pull/42 works too"),
		).toEqual({ kind: "pr", number: 42 });
	});

	it("finds a Linear issue URL anywhere in prose", () => {
		expect(
			extractRef(
				"context in https://linear.app/justicebid/issue/JUS-123-fix-it thanks",
			),
		).toEqual({ kind: "ticket", id: "JUS-123" });
	});

	it("prefers a PR URL over a Linear URL or a leading ticket", () => {
		expect(
			extractRef(
				"JUS-1 see https://linear.app/jb/issue/JUS-2 and github.com/a/b/pull/3",
			),
		).toEqual({ kind: "pr", number: 3 });
	});

	it("prefers a Linear URL over a leading ticket when no PR URL", () => {
		expect(
			extractRef("JUS-1 details at https://linear.app/jb/issue/JUS-2"),
		).toEqual({ kind: "ticket", id: "JUS-2" });
	});

	it("extracts a leading ticket id with trailing punctuation stripped", () => {
		expect(extractRef("JUS-1821: rework the extraction")).toEqual({
			kind: "ticket",
			id: "JUS-1821",
		});
		expect(extractRef("JUS-1821 rework the extraction")).toEqual({
			kind: "ticket",
			id: "JUS-1821",
		});
	});

	it("does not extract ticket-shaped tokens mid-prose", () => {
		expect(extractRef("hash it with SHA-256 before sending")).toBeNull();
		expect(extractRef("decode the UTF-8 payload")).toBeNull();
		expect(extractRef("negotiate HTTP-2 first")).toBeNull();
	});

	it("returns null on empty or no-match text", () => {
		expect(extractRef("")).toBeNull();
		expect(extractRef("   ")).toBeNull();
		expect(extractRef("just some plain prose here")).toBeNull();
	});
});

describe("extractTicketId", () => {
	it("finds ticket ids in branch names", () => {
		expect(extractTicketId("JUS-1423-fix-auth")).toBe("JUS-1423");
		expect(extractTicketId("feature/ABC2-99")).toBe("ABC2-99");
		expect(extractTicketId("no-ticket-here")).toBeNull();
	});
});
