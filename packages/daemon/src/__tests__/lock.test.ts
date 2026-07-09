import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { acquireLock, releaseLock } from "../lock.js";

const pidFile = () => join(mkdtempSync(join(tmpdir(), "qo-lock-")), "d.pid");

describe("acquireLock", () => {
	it("acquires and writes own pid", () => {
		const path = pidFile();
		expect(acquireLock(path)).toBe(true);
		expect(readFileSync(path, "utf-8").trim()).toBe(String(process.pid));
	});

	it("refuses when a live pid holds the lock", () => {
		const path = pidFile();
		writeFileSync(path, "99999");
		expect(acquireLock(path, { isPidAlive: () => true })).toBe(false);
	});

	it("steals a stale lock (dead pid)", () => {
		const path = pidFile();
		writeFileSync(path, "99999");
		expect(acquireLock(path, { isPidAlive: () => false })).toBe(true);
	});

	it("steals a garbage lock file", () => {
		const path = pidFile();
		writeFileSync(path, "not-a-pid");
		expect(acquireLock(path)).toBe(true);
	});

	it("releaseLock removes the file", () => {
		const path = pidFile();
		acquireLock(path);
		releaseLock(path);
		expect(acquireLock(path)).toBe(true);
	});
});
