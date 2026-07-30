import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type { ProviderConfig } from "@queohoh/core";
import { describe, expect, it } from "vitest";
import { settingsPath } from "../paths.js";
import { SettingsStore } from "../settings-store.js";

const PROVIDERS: ProviderConfig[] = [
	{ name: "claude", enabled: true },
	{ name: "grok", enabled: true },
	{ name: "codex", enabled: false },
];

function freshStateDir(): string {
	return mkdtempSync(join(tmpdir(), "qoo-settings-"));
}

function readSettings(stateDir: string): Record<string, unknown> {
	return JSON.parse(readFileSync(settingsPath(stateDir), "utf-8"));
}

describe("SettingsStore cron pause-set", () => {
	it("defaults to nothing disabled (a never-toggled def is enabled)", () => {
		const store = new SettingsStore(freshStateDir(), PROVIDERS);
		expect(store.isCronDisabled("demo/ping")).toBe(false);
	});

	it("setCronDisabled toggles, is write-through, and returns the ENABLED state", () => {
		const stateDir = freshStateDir();
		const store = new SettingsStore(stateDir, PROVIDERS);

		expect(store.setCronDisabled("demo/ping", true)).toBe(false); // now paused
		expect(store.isCronDisabled("demo/ping")).toBe(true);
		expect(readSettings(stateDir).disabled_crons).toEqual(["demo/ping"]);

		expect(store.setCronDisabled("demo/ping", false)).toBe(true); // resumed
		expect(store.isCronDisabled("demo/ping")).toBe(false);
		expect(readSettings(stateDir).disabled_crons).toEqual([]);
	});

	it("persists the pause-set across a restart (new store, same state dir)", () => {
		const stateDir = freshStateDir();
		const first = new SettingsStore(stateDir, PROVIDERS);
		first.setCronDisabled("demo/ping", true);
		first.setCronDisabled("acme/pr-review", true);

		const restarted = new SettingsStore(stateDir, PROVIDERS);
		expect(restarted.isCronDisabled("demo/ping")).toBe(true);
		expect(restarted.isCronDisabled("acme/pr-review")).toBe(true);
		expect(restarted.isCronDisabled("other/task")).toBe(false);
	});

	it("keeps active_provider and disabled_crons independent in one file", () => {
		const stateDir = freshStateDir();
		const store = new SettingsStore(stateDir, PROVIDERS);
		store.setActiveProvider("grok", PROVIDERS);
		store.setCronDisabled("demo/ping", true);

		const persisted = readSettings(stateDir);
		expect(persisted.active_provider).toBe("grok");
		expect(persisted.disabled_crons).toEqual(["demo/ping"]);

		// A provider switch must not drop the pause-set, and vice versa.
		store.setActiveProvider("claude", PROVIDERS);
		expect(readSettings(stateDir).disabled_crons).toEqual(["demo/ping"]);
	});

	it("tolerates an old settings.json with only active_provider", () => {
		const stateDir = freshStateDir();
		const path = settingsPath(stateDir);
		mkdirSync(dirname(path), { recursive: true });
		writeFileSync(path, `${JSON.stringify({ active_provider: "grok" })}\n`);

		const store = new SettingsStore(stateDir, PROVIDERS);
		expect(store.activeProvider()).toBe("grok");
		expect(store.isCronDisabled("demo/ping")).toBe(false);
	});
});

describe("SettingsStore favorite_worktrees set", () => {
	it("favorite_worktrees defaults empty, toggles, persists, and coexists with the other keys", () => {
		const stateDir = freshStateDir();
		const store = new SettingsStore(stateDir, PROVIDERS);

		expect(store.isWorktreeFavorite("platform/wt-a")).toBe(false);
		expect(store.setWorktreeFavorite("platform/wt-a", true)).toBe(true);
		expect(readSettings(stateDir).favorite_worktrees).toEqual(["platform/wt-a"]);

		store.setCronDisabled("platform/intake", true); // other keys survive a favorite write and vice versa
		expect(readSettings(stateDir).favorite_worktrees).toEqual(["platform/wt-a"]);
		expect(readSettings(stateDir).disabled_crons).toEqual(["platform/intake"]);

		const restarted = new SettingsStore(stateDir, PROVIDERS);
		expect(restarted.isWorktreeFavorite("platform/wt-a")).toBe(true);
		expect(restarted.setWorktreeFavorite("platform/wt-a", false)).toBe(false);
		expect(readSettings(stateDir).favorite_worktrees).toEqual([]);
	});

	it("tolerates an old settings.json without favorite_worktrees", () => {
		const stateDir = freshStateDir();
		const path = settingsPath(stateDir);
		mkdirSync(dirname(path), { recursive: true });
		writeFileSync(path, `${JSON.stringify({ active_provider: "claude" })}\n`);

		expect(new SettingsStore(stateDir, PROVIDERS).isWorktreeFavorite("x/y")).toBe(
			false,
		);
	});
});
