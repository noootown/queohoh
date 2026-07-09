import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { readRunFiles } from "../run-files.js";

describe("readRunFiles", () => {
	it("reads report and last 25 transcript lines", () => {
		const runsDir = mkdtempSync(join(tmpdir(), "qo-runfiles-"));
		const dir = join(runsDir, "01TASK");
		mkdirSync(dir, { recursive: true });
		writeFileSync(join(dir, "report.md"), "# Result\nok\n");
		const lines = Array.from({ length: 40 }, (_, i) => `line ${i}`);
		writeFileSync(join(dir, "transcript.md"), lines.join("\n"));
		const out = readRunFiles(runsDir, "01TASK");
		expect(out.report).toContain("# Result");
		expect(out.transcriptTail).toHaveLength(25);
		expect(out.transcriptTail[24]).toBe("line 39");
	});

	it("honors tailLines and defaults to 25", () => {
		const runsDir = mkdtempSync(join(tmpdir(), "qo-runfiles-"));
		const dir = join(runsDir, "01TAIL");
		mkdirSync(dir, { recursive: true });
		const lines = Array.from({ length: 200 }, (_, i) => `line ${i}`);
		writeFileSync(join(dir, "transcript.md"), lines.join("\n"));
		const custom = readRunFiles(runsDir, "01TAIL", { tailLines: 100 });
		expect(custom.transcriptTail).toHaveLength(100);
		expect(custom.transcriptTail[99]).toBe("line 199");
		const def = readRunFiles(runsDir, "01TAIL");
		expect(def.transcriptTail).toHaveLength(25);
		expect(def.transcriptTail[24]).toBe("line 199");
	});

	it("clamps tailLines below 1 to a single (last) line", () => {
		const runsDir = mkdtempSync(join(tmpdir(), "qo-runfiles-"));
		const dir = join(runsDir, "01ZERO");
		mkdirSync(dir, { recursive: true });
		const lines = Array.from({ length: 10 }, (_, i) => `line ${i}`);
		writeFileSync(join(dir, "transcript.md"), lines.join("\n"));
		const out = readRunFiles(runsDir, "01ZERO", { tailLines: 0 });
		expect(out.transcriptTail).toEqual(["line 9"]);
	});

	it("handles missing files", () => {
		const runsDir = mkdtempSync(join(tmpdir(), "qo-runfiles-"));
		const out = readRunFiles(runsDir, "01NOPE");
		expect(out.report).toBeNull();
		expect(out.transcriptTail).toEqual([]);
	});

	it("returns the correct tail from a transcript larger than 64KB", () => {
		const runsDir = mkdtempSync(join(tmpdir(), "qo-runfiles-"));
		const dir = join(runsDir, "01BIG");
		mkdirSync(dir, { recursive: true });
		// Padding to push the file well past 64KB, then 25 known tail lines.
		const padding = Array.from(
			{ length: 5000 },
			(_, i) => `padding line ${i} ${"x".repeat(32)}`,
		);
		const tail = Array.from({ length: 25 }, (_, i) => `tail ${i}`);
		const all = [...padding, ...tail];
		const content = all.join("\n");
		expect(content.length).toBeGreaterThan(65536);
		writeFileSync(join(dir, "transcript.md"), content);
		const out = readRunFiles(runsDir, "01BIG");
		expect(out.transcriptTail).toHaveLength(25);
		expect(out.transcriptTail).toEqual(tail);
		expect(out.transcriptTail[0]).toBe("tail 0");
		expect(out.transcriptTail[24]).toBe("tail 24");
	});
});
