import { mkdtempSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { currentBuildId } from "../build-id.js";

function writeJs(dir: string, name: string, mtimeSec: number): void {
	const path = join(dir, name);
	writeFileSync(path, "// built\n");
	// atime, mtime in seconds; mtimeMs = mtimeSec * 1000.
	utimesSync(path, mtimeSec, mtimeSec);
}

describe("currentBuildId", () => {
	it("returns the newest .js mtime in the dir, as a millisecond string", () => {
		const dir = mkdtempSync(join(tmpdir(), "qo-buildid-"));
		writeJs(dir, "api.js", 1000);
		writeJs(dir, "cli.js", 2500); // newest
		writeJs(dir, "index.js", 1500);
		expect(currentBuildId(dir)).toBe(String(2500 * 1000));
	});

	it("ignores non-.js files when picking the newest mtime", () => {
		const dir = mkdtempSync(join(tmpdir(), "qo-buildid-"));
		writeJs(dir, "api.js", 1000);
		// A newer .d.ts / .map must not win — only .js counts.
		const later = join(dir, "api.d.ts");
		writeFileSync(later, "types");
		utimesSync(later, 9999, 9999);
		expect(currentBuildId(dir)).toBe(String(1000 * 1000));
	});

	it('returns "0" for a directory with no .js files (source-mode / vitest)', () => {
		const dir = mkdtempSync(join(tmpdir(), "qo-buildid-"));
		writeFileSync(join(dir, "build-id.ts"), "source");
		expect(currentBuildId(dir)).toBe("0");
	});

	it('returns "0" for a non-existent directory', () => {
		expect(currentBuildId(join(tmpdir(), "qo-does-not-exist-xyz"))).toBe("0");
	});

	it("advances when a .js file is rewritten with a newer mtime (rebuild)", () => {
		const dir = mkdtempSync(join(tmpdir(), "qo-buildid-"));
		writeJs(dir, "api.js", 1000);
		const before = currentBuildId(dir);
		writeJs(dir, "api.js", 2000); // tsc rewrites the file on rebuild
		expect(currentBuildId(dir)).not.toBe(before);
		expect(currentBuildId(dir)).toBe(String(2000 * 1000));
	});
});
