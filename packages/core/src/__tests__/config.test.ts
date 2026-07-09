import { mkdtempSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	loadGlobalConfig,
	loadProjectVars,
	projectWorkspaceDir,
} from "../config.js";

describe("loadGlobalConfig", () => {
	it("parses projects and applies defaults", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-"));
		const path = join(dir, "config.yaml");
		writeFileSync(
			path,
			[
				"projects:",
				"  - name: platform",
				"    path: ~/workspace/platform",
				"vars:",
				"  github_user: noootown",
			].join("\n"),
		);
		const config = loadGlobalConfig(path);
		expect(config.projects).toEqual([
			{ name: "platform", path: join(homedir(), "workspace/platform") },
		]);
		expect(config.maxConcurrentTasks).toBe(3);
		expect(config.archiveAfterDays).toBe(7);
		expect(config.vars).toEqual({ github_user: "noootown" });
	});

	it("throws on missing file", () => {
		expect(() => loadGlobalConfig("/nope/config.yaml")).toThrow(
			"config not found: /nope/config.yaml",
		);
	});

	it("rejects duplicate project names", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-dup-"));
		const path = join(dir, "config.yaml");
		writeFileSync(
			path,
			[
				"projects:",
				"  - name: platform",
				"    path: ~/workspace/platform",
				"  - name: platform",
				"    path: ~/workspace/platform-2",
			].join("\n"),
		);
		expect(() => loadGlobalConfig(path)).toThrow(
			"duplicate project name: platform",
		);
	});
});

describe("workspace", () => {
	it("defaults workspace to ~/.config/queohoh and expands tilde", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-"));
		const path = join(dir, "config.yaml");
		writeFileSync(path, "projects: []\n");
		expect(loadGlobalConfig(path).workspace).toBe(
			join(homedir(), ".config/queohoh"),
		);
		writeFileSync(path, "workspace: ~/workspace/queohoh\nprojects: []\n");
		expect(loadGlobalConfig(path).workspace).toBe(
			join(homedir(), "workspace/queohoh"),
		);
	});

	it("projectWorkspaceDir joins workspace and project name", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-"));
		const path = join(dir, "config.yaml");
		writeFileSync(path, "workspace: /ws\nprojects: []\n");
		const config = loadGlobalConfig(path);
		expect(projectWorkspaceDir(config, "platform")).toBe("/ws/platform");
	});
});

describe("loadProjectVars", () => {
	it("reads and stringifies vars.yaml", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pv-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"repo: justicebid/platform\nport: 3000\n",
		);
		expect(loadProjectVars(dir)).toEqual({
			repo: "justicebid/platform",
			port: "3000",
		});
	});

	it("returns {} when absent and rejects non-scalar values", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pv-"));
		expect(loadProjectVars(dir)).toEqual({});
		writeFileSync(join(dir, "vars.yaml"), "nested:\n  a: 1\n");
		expect(() => loadProjectVars(dir)).toThrow(/non-scalar var: nested/);
	});
});
