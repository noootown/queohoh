import { describe, expect, it } from "vitest";
import { parseTaskFile, serializeTaskFile } from "../task.js";

describe("attempted_models field", () => {
	it("round-trips attemptedModels and defaults legacy files to []", () => {
		const legacy = parseTaskFile(
			`---\nid: t1\nstatus: queued\ntarget:\n  repo: r\n  ref: temp\ncreated: "2026-07-15"\nsource: mcp\n---\nhi`,
		);
		expect(legacy.attemptedModels).toEqual([]);
		const round = parseTaskFile(
			serializeTaskFile({ ...legacy, attemptedModels: ["claude"] }),
		);
		expect(round.attemptedModels).toEqual(["claude"]);
	});

	it("reads the legacy attempted_providers key into attemptedModels", () => {
		const legacy = parseTaskFile(
			`---\nid: t1\nstatus: queued\ntarget:\n  repo: r\n  ref: temp\ncreated: "2026-07-15"\nsource: mcp\nattempted_providers:\n  - claude\n---\nhi`,
		);
		expect(legacy.attemptedModels).toEqual(["claude"]);
	});

	it("prefers attempted_models over the legacy key when both are present", () => {
		const both = parseTaskFile(
			`---\nid: t1\nstatus: queued\ntarget:\n  repo: r\n  ref: temp\ncreated: "2026-07-15"\nsource: mcp\nattempted_providers:\n  - grok\nattempted_models:\n  - claude/claude-opus-4.8\n---\nhi`,
		);
		expect(both.attemptedModels).toEqual(["claude/claude-opus-4.8"]);
	});

	it("writes emit only attempted_models, never the legacy key", () => {
		const legacy = parseTaskFile(
			`---\nid: t1\nstatus: queued\ntarget:\n  repo: r\n  ref: temp\ncreated: "2026-07-15"\nsource: mcp\n---\nhi`,
		);
		const serialized = serializeTaskFile({
			...legacy,
			attemptedModels: ["claude"],
		});
		expect(serialized).toContain("attempted_models:");
		expect(serialized).not.toContain("attempted_providers:");
	});
});
