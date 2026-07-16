import { describe, expect, it } from "vitest";
import { parseTaskFile, serializeTaskFile } from "../task.js";

describe("attempted_providers field", () => {
	it("round-trips attemptedProviders and defaults legacy files to []", () => {
		const legacy = parseTaskFile(
			`---\nid: t1\nstatus: queued\ntarget:\n  repo: r\n  ref: temp\ncreated: "2026-07-15"\nsource: mcp\n---\nhi`,
		);
		expect(legacy.attemptedProviders).toEqual([]);
		const round = parseTaskFile(
			serializeTaskFile({ ...legacy, attemptedProviders: ["claude"] }),
		);
		expect(round.attemptedProviders).toEqual(["claude"]);
	});
});
