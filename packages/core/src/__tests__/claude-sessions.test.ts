import { mkdirSync, mkdtempSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { encodeProjectDir, listClaudeSessions } from "../claude-sessions.js";

describe("encodeProjectDir", () => {
	it("replaces slashes and dots with dashes", () => {
		expect(
			encodeProjectDir("/Users/n/Downloads/agent247/queohoh.action-menu"),
		).toBe("-Users-n-Downloads-agent247-queohoh-action-menu");
	});
});

function writeSession(
	dir: string,
	id: string,
	lines: unknown[],
	mtimeSec: number,
): void {
	const path = join(dir, `${id}.jsonl`);
	writeFileSync(path, lines.map((l) => JSON.stringify(l)).join("\n"));
	utimesSync(path, mtimeSec, mtimeSec);
}

describe("listClaudeSessions", () => {
	it("lists newest-first, capped, with ai-title and first-prompt labels", () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		const wt = "/wt/demo";
		const dir = join(projects, encodeProjectDir(wt));
		mkdirSync(dir, { recursive: true });
		// Subdir = subagent transcripts; must be skipped.
		mkdirSync(join(dir, "aaaa-sub"), { recursive: true });
		writeSession(
			dir,
			"s-old",
			[{ type: "user", message: { content: "old prompt\nrest" } }],
			1_000,
		);
		writeSession(
			dir,
			"s-titled",
			[
				{
					type: "user",
					message: { content: [{ type: "text", text: "first line here" }] },
				},
				{ type: "ai-title", aiTitle: "Stale title", sessionId: "s-titled" },
				{ type: "ai-title", aiTitle: "Fresh title", sessionId: "s-titled" },
			],
			3_000,
		);
		writeSession(
			dir,
			"s-untitled",
			[{ type: "user", message: { content: "just a prompt" } }],
			2_000,
		);

		const got = listClaudeSessions(projects, wt, 5);
		expect(got.map((s) => s.sessionId)).toEqual([
			"s-titled",
			"s-untitled",
			"s-old",
		]);
		expect(got[0]?.aiTitle).toBe("Fresh title"); // last ai-title wins
		expect(got[0]?.firstPrompt).toBe("first line here");
		expect(got[1]?.aiTitle).toBeNull();
		expect(got[1]?.firstPrompt).toBe("just a prompt");
		expect(got[2]?.firstPrompt).toBe("old prompt");
	});

	it("caps at limit", () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		const wt = "/wt/many";
		const dir = join(projects, encodeProjectDir(wt));
		mkdirSync(dir, { recursive: true });
		for (let i = 0; i < 8; i++)
			writeSession(
				dir,
				`s-${i}`,
				[{ type: "user", message: { content: `p${i}` } }],
				1_000 + i,
			);
		const got = listClaudeSessions(projects, wt, 5);
		expect(got).toHaveLength(5);
		expect(got[0]?.sessionId).toBe("s-7");
	});

	it("returns [] for a worktree with no session dir", () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		expect(listClaudeSessions(projects, "/nowhere", 5)).toEqual([]);
	});

	it("tolerates malformed jsonl lines", () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		const wt = "/wt/bad";
		const dir = join(projects, encodeProjectDir(wt));
		mkdirSync(dir, { recursive: true });
		const path = join(dir, "s-bad.jsonl");
		writeFileSync(
			path,
			'not json\n{"type":"ai-title","aiTitle":"Ok","sessionId":"s-bad"}\n',
		);
		const got = listClaudeSessions(projects, wt, 5);
		expect(got[0]?.aiTitle).toBe("Ok");
	});
});
