import { describe, expect, it } from "vitest";
import { mergeSessionSources, type SessionRow } from "../session-merge.js";

function row(
	overrides: Partial<SessionRow> & { sessionId: string },
): SessionRow {
	return {
		mtimeMs: 1_000,
		provider: "claude",
		label: overrides.sessionId,
		...overrides,
	};
}

describe("mergeSessionSources", () => {
	it("unions disjoint providers from both sources", () => {
		const diskRows = [
			row({ sessionId: "c1", provider: "claude", mtimeMs: 100 }),
		];
		const runStoreRows = [
			row({ sessionId: "g1", provider: "grok", mtimeMs: 200 }),
			row({ sessionId: "x1", provider: "codex", mtimeMs: 300 }),
		];
		const got = mergeSessionSources(diskRows, runStoreRows, 5);
		expect(got.map((r) => r.sessionId).sort()).toEqual(["c1", "g1", "x1"]);
		expect(got.map((r) => r.provider).sort()).toEqual([
			"claude",
			"codex",
			"grok",
		]);
	});

	it("dedups a session present in both sources, preferring run-store metadata", () => {
		const diskRows = [
			row({
				sessionId: "s1",
				provider: "claude",
				label: "disk label",
				model: undefined,
				mtimeMs: 100,
			}),
		];
		const runStoreRows = [
			row({
				sessionId: "s1",
				provider: "claude",
				label: "run-store label",
				model: "claude/opus",
				mtimeMs: 50, // older than the disk row
			}),
		];
		const got = mergeSessionSources(diskRows, runStoreRows, 5);
		expect(got).toHaveLength(1);
		expect(got[0]?.label).toBe("run-store label"); // run-store metadata wins
		expect(got[0]?.model).toBe("claude/opus");
		expect(got[0]?.mtimeMs).toBe(100); // max of the two mtimes survives
	});

	it("dedups duplicate run-store rows for the same session, keeping the max mtime", () => {
		const runStoreRows = [
			row({ sessionId: "s1", provider: "claude", mtimeMs: 50, label: "old" }),
			row({ sessionId: "s1", provider: "claude", mtimeMs: 500, label: "new" }),
		];
		const got = mergeSessionSources([], runStoreRows, 5);
		expect(got).toHaveLength(1);
		expect(got[0]?.label).toBe("new");
		expect(got[0]?.mtimeMs).toBe(500);
	});

	it("caps each provider independently — one provider over the limit does not starve another", () => {
		const grokRows = Array.from({ length: 8 }, (_, i) =>
			row({ sessionId: `g${i}`, provider: "grok", mtimeMs: i }),
		);
		const codexRows = [row({ sessionId: "x0", provider: "codex", mtimeMs: 3 })];
		const got = mergeSessionSources([], [...grokRows, ...codexRows], 5);
		const grokIds = got
			.filter((r) => r.provider === "grok")
			.map((r) => r.sessionId);
		const codexIds = got
			.filter((r) => r.provider === "codex")
			.map((r) => r.sessionId);
		expect(grokIds).toHaveLength(5);
		// The 5 most recent grok sessions (highest mtime) survive.
		expect(grokIds).toEqual(["g7", "g6", "g5", "g4", "g3"]);
		expect(codexIds).toEqual(["x0"]);
	});

	it("merges providers into one list sorted by recency, interleaved not grouped", () => {
		const diskRows = [
			row({ sessionId: "c1", provider: "claude", mtimeMs: 300 }),
		];
		const runStoreRows = [
			row({ sessionId: "g1", provider: "grok", mtimeMs: 400 }),
			row({ sessionId: "x1", provider: "codex", mtimeMs: 200 }),
			row({ sessionId: "g2", provider: "grok", mtimeMs: 100 }),
		];
		const got = mergeSessionSources(diskRows, runStoreRows, 5);
		expect(got.map((r) => r.sessionId)).toEqual(["g1", "c1", "x1", "g2"]);
	});

	it("returns an empty list when both sources are empty", () => {
		expect(mergeSessionSources([], [], 5)).toEqual([]);
	});
});
