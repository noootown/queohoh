import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { SessionLineageStore } from "../session-lineage.js";

function storePath(): string {
	return join(mkdtempSync(join(tmpdir(), "lineage-")), "session-lineage.json");
}

describe("SessionLineageStore", () => {
	it("tip returns the id itself when no fork is recorded", () => {
		const s = new SessionLineageStore(storePath());
		expect(s.tip("sess-x")).toBe("sess-x");
	});

	it("follows multi-hop chains to the newest descendant", () => {
		const s = new SessionLineageStore(storePath());
		s.recordFork("x", "y");
		s.recordFork("y", "z");
		expect(s.tip("x")).toBe("z");
		expect(s.tip("y")).toBe("z");
	});

	it("keeps two chains independent", () => {
		const s = new SessionLineageStore(storePath());
		s.recordFork("x", "y");
		s.recordFork("q", "r");
		expect(s.tip("x")).toBe("y");
		expect(s.tip("q")).toBe("r");
	});

	it("is cycle-guarded", () => {
		const s = new SessionLineageStore(storePath());
		s.recordFork("x", "y");
		s.recordFork("y", "x");
		// Must terminate; returns the last id before revisiting.
		expect(["x", "y"]).toContain(s.tip("x"));
	});

	it("ignores self-forks", () => {
		const s = new SessionLineageStore(storePath());
		s.recordFork("x", "x");
		expect(s.tip("x")).toBe("x");
	});

	it("persists across instances and survives a corrupt file", () => {
		const path = storePath();
		const a = new SessionLineageStore(path);
		a.recordFork("x", "y");
		const b = new SessionLineageStore(path);
		expect(b.tip("x")).toBe("y");
		writeFileSync(path, "not json");
		const c = new SessionLineageStore(path);
		expect(c.tip("x")).toBe("x");
	});

	it("is safe against prototype-chain keys", () => {
		const s = new SessionLineageStore(storePath());
		expect(s.tip("toString")).toBe("toString");
		expect(s.tip("__proto__")).toBe("__proto__");
	});

	it("records and reads a session's provider; unknown → null", () => {
		const s = new SessionLineageStore(storePath());
		s.recordProvider("gs1", "grok");
		expect(s.providerOf("gs1")).toBe("grok");
		expect(s.providerOf("unknown")).toBeNull();
	});

	it("persists providers across instances, alongside forks", () => {
		const path = storePath();
		const a = new SessionLineageStore(path);
		a.recordFork("x", "y");
		a.recordProvider("y", "codex");
		const b = new SessionLineageStore(path);
		expect(b.tip("x")).toBe("y");
		expect(b.providerOf("y")).toBe("codex");
	});

	it("loads a legacy file containing only { forks } with providerOf → null", () => {
		const path = storePath();
		writeFileSync(path, JSON.stringify({ forks: { x: "y" } }));
		const s = new SessionLineageStore(path);
		expect(s.tip("x")).toBe("y");
		expect(s.providerOf("x")).toBeNull();
		expect(s.providerOf("y")).toBeNull();
	});
});
