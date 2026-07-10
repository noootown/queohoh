import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { MainSessionStore } from "../main-sessions.js";

const file = () =>
	join(mkdtempSync(join(tmpdir(), "qo-main-sess-")), "main-sessions.json");

describe("MainSessionStore", () => {
	it("get on empty returns null", () => {
		const store = new MainSessionStore(file());
		expect(store.get("platform:JUS-1")).toBeNull();
	});

	it("set/get round-trips", () => {
		const store = new MainSessionStore(file());
		store.set("platform:JUS-1", "sess-abc");
		expect(store.get("platform:JUS-1")).toBe("sess-abc");
	});

	it("persists across a second instance on the same path", () => {
		const path = file();
		const store = new MainSessionStore(path);
		store.set("platform:JUS-1", "sess-abc");
		const reloaded = new MainSessionStore(path);
		expect(reloaded.get("platform:JUS-1")).toBe("sess-abc");
	});

	it("corrupt file yields empty store without throwing", () => {
		const path = file();
		writeFileSync(path, "{not valid json");
		const store = new MainSessionStore(path);
		expect(store.get("platform:JUS-1")).toBeNull();
		expect(store.all()).toEqual({});
	});

	it("get on prototype-key lanes returns null on an empty store", () => {
		const store = new MainSessionStore(file());
		expect(store.get("toString")).toBeNull();
		expect(store.get("hasOwnProperty")).toBeNull();
		expect(store.get("__proto__")).toBeNull();
	});

	it("proto-like lane keys round-trip normally, including across reload", () => {
		const path = file();
		const store = new MainSessionStore(path);
		store.set("__proto__", "sess-proto");
		store.set("constructor", "sess-ctor");
		expect(store.get("__proto__")).toBe("sess-proto");
		expect(store.get("constructor")).toBe("sess-ctor");
		const reloaded = new MainSessionStore(path);
		expect(reloaded.get("__proto__")).toBe("sess-proto");
		expect(reloaded.get("constructor")).toBe("sess-ctor");
	});

	it("all() returns a copy that does not affect the store when mutated", () => {
		const store = new MainSessionStore(file());
		store.set("lane-a", "id-a");
		const snapshot = store.all();
		snapshot["lane-a"] = "mutated";
		snapshot["lane-b"] = "id-b";
		expect(store.get("lane-a")).toBe("id-a");
		expect(store.get("lane-b")).toBeNull();
	});
});

describe("timestamped entries", () => {
	it("entry() returns sessionId with an ISO updatedAt after set()", () => {
		const store = new MainSessionStore(file());
		const before = new Date().toISOString();
		store.set("platform:JUS-1", "sess-abc");
		const entry = store.entry("platform:JUS-1");
		expect(entry?.sessionId).toBe("sess-abc");
		expect(entry?.updatedAt && entry.updatedAt >= before).toBe(true);
	});

	it("entry() on missing lane returns null", () => {
		const store = new MainSessionStore(file());
		expect(store.entry("nope")).toBeNull();
	});

	it("upgrades legacy bare-string entries to epoch updatedAt", () => {
		const path = file();
		writeFileSync(
			path,
			JSON.stringify({ sessions: { "platform:JUS-1": "sess-legacy" } }),
		);
		const store = new MainSessionStore(path);
		expect(store.get("platform:JUS-1")).toBe("sess-legacy");
		expect(store.entry("platform:JUS-1")).toEqual({
			sessionId: "sess-legacy",
			updatedAt: "1970-01-01T00:00:00.000Z",
		});
	});

	it("persists timestamped entries across reload", () => {
		const path = file();
		const store = new MainSessionStore(path);
		store.set("platform:JUS-1", "sess-abc");
		const reloaded = new MainSessionStore(path);
		expect(reloaded.entry("platform:JUS-1")?.sessionId).toBe("sess-abc");
		expect(typeof reloaded.entry("platform:JUS-1")?.updatedAt).toBe("string");
	});

	it("all() still maps lane to bare sessionId strings", () => {
		const store = new MainSessionStore(file());
		store.set("lane-a", "id-a");
		expect(store.all()).toEqual({ "lane-a": "id-a" });
	});
});
