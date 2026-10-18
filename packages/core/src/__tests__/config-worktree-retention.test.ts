import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
	loadGlobalConfig,
	loadProjectWorktreeRetentionDays,
} from "../config.js";

const dirs: string[] = [];
afterEach(() => {
	vi.restoreAllMocks();
	for (const dir of dirs.splice(0))
		rmSync(dir, { recursive: true, force: true });
});
function fixture() {
	const dir = mkdtempSync(join(tmpdir(), "qoo-retention-config-"));
	dirs.push(dir);
	return dir;
}
describe("worktree retention config", () => {
	it.each([
		"",
		"worktree_retention_days: null\n",
	])("disables persistent cleanup with omitted or explicit null global setting (%s)", (setting) => {
		const path = join(fixture(), "config.yaml");
		writeFileSync(path, `projects: []\n${setting}`);
		expect(loadGlobalConfig(path).worktreeRetentionDays).toBeNull();
	});
	it("accepts a positive global retention", () => {
		const path = join(fixture(), "config.yaml");
		writeFileSync(path, "projects: []\nworktree_retention_days: 14\n");
		expect(loadGlobalConfig(path).worktreeRetentionDays).toBe(14);
	});
	it("allows a project to disable an enabled global retention with null", () => {
		const dir = fixture();
		writeFileSync(join(dir, "vars.yaml"), "worktree_retention_days: null\n");
		expect(loadProjectWorktreeRetentionDays(dir, 14)).toBeNull();
	});
	it("inherits only when the project key is absent", () => {
		const dir = fixture();
		expect(loadProjectWorktreeRetentionDays(dir, 14)).toBe(14);
		writeFileSync(join(dir, "vars.yaml"), "author: alice\n");
		expect(loadProjectWorktreeRetentionDays(dir, 14)).toBe(14);
	});
	it.each([
		"0",
		"-1",
		"1.5",
		"nonsense",
		"false",
	])("rejects invalid project retention %s", (value) => {
		vi.spyOn(console, "warn").mockImplementation(() => {});
		const dir = fixture();
		writeFileSync(
			join(dir, "vars.yaml"),
			`worktree_retention_days: ${value}\n`,
		);
		expect(loadProjectWorktreeRetentionDays(dir, null)).toBeNull();
	});
});
