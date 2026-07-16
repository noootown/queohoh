import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { GlobalConfig } from "../config.js";
import {
	globalWorkspaceDir,
	loadGlobalConfig,
	loadProjectDefaultBranch,
	loadProjectDefaultModel,
	loadProjectGithubId,
	loadProjectModels,
	loadProjectProtectedWorktrees,
	loadProjectTaskRetentionDays,
	loadProjectVars,
	projectWorkspaceDir,
	resolveDefinition,
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
		expect(config.maxConcurrentTasks).toBe(5);
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

	it("parses goto_command when present and omits it when absent", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-goto-"));
		const withCmd = join(dir, "with.yaml");
		writeFileSync(
			withCmd,
			["projects: []", 'goto_command: "init-tab {cmd}"'].join("\n"),
		);
		expect(loadGlobalConfig(withCmd).gotoCommand).toBe("init-tab {cmd}");

		const without = join(dir, "without.yaml");
		writeFileSync(without, "projects: []\n");
		expect(loadGlobalConfig(without).gotoCommand).toBeUndefined();
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

	it("globalWorkspaceDir joins workspace and 'global'", () => {
		const config = { workspace: "/ws" } as GlobalConfig;
		expect(globalWorkspaceDir(config)).toBe("/ws/global");
	});
});

describe("resolveDefinition — project vs global", () => {
	function writeDef(dir: string, config: string, prompt: string) {
		mkdirSync(dir, { recursive: true });
		writeFileSync(join(dir, "config.yaml"), config);
		writeFileSync(join(dir, "prompt.md"), prompt);
	}

	function makeWorkspace(): GlobalConfig {
		const workspace = mkdtempSync(join(tmpdir(), "queohoh-ws-"));
		return {
			workspace,
			projects: [{ name: "platform", path: "/repo/platform" }],
			maxConcurrentTasks: 3,
			archiveAfterDays: 7,
			vars: {},
			models: {},
		};
	}

	it("falls back to the global tasks dir, keeping repo as the target project", () => {
		const config = makeWorkspace();
		writeDef(
			join(globalWorkspaceDir(config), "tasks", "squash-merge"),
			"worktree: repo\n",
			"squash {{source}}\n",
		);
		const def = resolveDefinition(config, "platform", "squash-merge");
		expect(def.name).toBe("squash-merge");
		expect(def.repo).toBe("platform");
		expect(def.worktree).toBe("repo");
	});

	it("prefers a project-local definition that shadows the global one", () => {
		const config = makeWorkspace();
		writeDef(
			join(globalWorkspaceDir(config), "tasks", "greet"),
			"worktree: repo\n",
			"global greet\n",
		);
		writeDef(
			join(projectWorkspaceDir(config, "platform"), "tasks", "greet"),
			"worktree: temp\n",
			"local greet\n",
		);
		const def = resolveDefinition(config, "platform", "greet");
		expect(def.prompt).toBe("local greet\n");
		expect(def.worktree).toBe("temp");
	});

	it("surfaces the project-dir ENOENT when the name is in neither dir", () => {
		const config = makeWorkspace();
		expect(() => resolveDefinition(config, "platform", "nope")).toThrow();
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

	it("skips the reserved models block instead of throwing", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pv-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"ticket: JUS-1\nmodels:\n  sonnet: claude-sonnet-4-6\n",
		);
		expect(loadProjectVars(dir)).toEqual({ ticket: "JUS-1" });
	});

	it("skips the reserved github_id key instead of exposing it as a var", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pv-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"ticket: JUS-1\ngithub_id: noootown\n",
		);
		expect(loadProjectVars(dir)).toEqual({ ticket: "JUS-1" });
	});

	it("skips the reserved default_model key instead of exposing it as a var", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pv-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"ticket: JUS-1\ndefault_model: opus\n",
		);
		expect(loadProjectVars(dir)).toEqual({ ticket: "JUS-1" });
	});

	it("skips the reserved task_retention_days key instead of exposing it as a var", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pv-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"ticket: JUS-1\ntask_retention_days: 15\n",
		);
		expect(loadProjectVars(dir)).toEqual({ ticket: "JUS-1" });
	});
});

describe("loadProjectTaskRetentionDays", () => {
	it("reads a positive-integer task_retention_days from vars.yaml", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-trd-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"task_retention_days: 15\ngithub_id: noootown\n",
		);
		expect(loadProjectTaskRetentionDays(dir, 7)).toBe(15);
	});

	it("falls back to the default for absent file or absent key", () => {
		const absent = mkdtempSync(join(tmpdir(), "queohoh-trd-"));
		expect(loadProjectTaskRetentionDays(absent, 7)).toBe(7);

		const noKey = mkdtempSync(join(tmpdir(), "queohoh-trd-"));
		writeFileSync(join(noKey, "vars.yaml"), "ticket: JUS-1\n");
		expect(loadProjectTaskRetentionDays(noKey, 7)).toBe(7);
	});

	it("falls back to the default for zero, negative, non-integer, or non-numeric values", () => {
		const zero = mkdtempSync(join(tmpdir(), "queohoh-trd-"));
		writeFileSync(join(zero, "vars.yaml"), "task_retention_days: 0\n");
		expect(loadProjectTaskRetentionDays(zero, 7)).toBe(7);

		const negative = mkdtempSync(join(tmpdir(), "queohoh-trd-"));
		writeFileSync(join(negative, "vars.yaml"), "task_retention_days: -3\n");
		expect(loadProjectTaskRetentionDays(negative, 7)).toBe(7);

		const fractional = mkdtempSync(join(tmpdir(), "queohoh-trd-"));
		writeFileSync(join(fractional, "vars.yaml"), "task_retention_days: 7.5\n");
		expect(loadProjectTaskRetentionDays(fractional, 7)).toBe(7);

		const str = mkdtempSync(join(tmpdir(), "queohoh-trd-"));
		writeFileSync(join(str, "vars.yaml"), "task_retention_days: '15'\n");
		expect(loadProjectTaskRetentionDays(str, 7)).toBe(7);
	});

	it("returns the given fallback verbatim, not a hardcoded 7", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-trd-"));
		writeFileSync(join(dir, "vars.yaml"), "ticket: JUS-1\n");
		expect(loadProjectTaskRetentionDays(dir, 30)).toBe(30);
	});
});

describe("loadProjectDefaultModel", () => {
	it("reads a string default_model from vars.yaml", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-dm-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"default_model: opus\ngithub_id: noootown\n",
		);
		expect(loadProjectDefaultModel(dir)).toBe("opus");
	});

	it("returns undefined for absent file, absent key, empty string, or non-string", () => {
		const absent = mkdtempSync(join(tmpdir(), "queohoh-dm-"));
		expect(loadProjectDefaultModel(absent)).toBeUndefined();

		const noKey = mkdtempSync(join(tmpdir(), "queohoh-dm-"));
		writeFileSync(join(noKey, "vars.yaml"), "ticket: JUS-1\n");
		expect(loadProjectDefaultModel(noKey)).toBeUndefined();

		const blank = mkdtempSync(join(tmpdir(), "queohoh-dm-"));
		writeFileSync(join(blank, "vars.yaml"), "default_model: ''\n");
		expect(loadProjectDefaultModel(blank)).toBeUndefined();

		const nested = mkdtempSync(join(tmpdir(), "queohoh-dm-"));
		writeFileSync(join(nested, "vars.yaml"), "default_model:\n  a: b\n");
		expect(loadProjectDefaultModel(nested)).toBeUndefined();
	});

	it("coexists with a models: override block", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-dm-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"default_model: opus\nmodels:\n  opus: claude-opus-4-8\n",
		);
		expect(loadProjectDefaultModel(dir)).toBe("opus");
		expect(loadProjectModels(dir)).toEqual({ opus: "claude-opus-4-8" });
	});
});

describe("loadProjectDefaultBranch", () => {
	it("reads a string default_branch from vars.yaml", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-db-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"default_branch: develop\ngithub_id: noootown\n",
		);
		expect(loadProjectDefaultBranch(dir)).toBe("develop");
	});

	it("falls back to main for absent file, absent key, empty string, or non-string", () => {
		const absent = mkdtempSync(join(tmpdir(), "queohoh-db-"));
		expect(loadProjectDefaultBranch(absent)).toBe("main");

		const noKey = mkdtempSync(join(tmpdir(), "queohoh-db-"));
		writeFileSync(join(noKey, "vars.yaml"), "ticket: JUS-1\n");
		expect(loadProjectDefaultBranch(noKey)).toBe("main");

		const blank = mkdtempSync(join(tmpdir(), "queohoh-db-"));
		writeFileSync(join(blank, "vars.yaml"), "default_branch: ''\n");
		expect(loadProjectDefaultBranch(blank)).toBe("main");

		const nested = mkdtempSync(join(tmpdir(), "queohoh-db-"));
		writeFileSync(join(nested, "vars.yaml"), "default_branch:\n  a: b\n");
		expect(loadProjectDefaultBranch(nested)).toBe("main");
	});

	it("falls back to main (never throws) for a non-mapping vars.yaml", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-db-"));
		writeFileSync(join(dir, "vars.yaml"), "[not, a, map]\n");
		expect(loadProjectDefaultBranch(dir)).toBe("main");
	});

	it("is not surfaced as a template var by loadProjectVars", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-db-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"ticket: JUS-1\ndefault_branch: develop\n",
		);
		expect(loadProjectVars(dir)).toEqual({ ticket: "JUS-1" });
	});
});

describe("loadProjectGithubId", () => {
	it("reads a string github_id from vars.yaml", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-gh-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"ticket: JUS-1\ngithub_id: noootown\n",
		);
		expect(loadProjectGithubId(dir)).toBe("noootown");
	});

	it("returns undefined for absent file, absent key, empty string, or non-string", () => {
		const absent = mkdtempSync(join(tmpdir(), "queohoh-gh-"));
		expect(loadProjectGithubId(absent)).toBeUndefined();

		const noKey = mkdtempSync(join(tmpdir(), "queohoh-gh-"));
		writeFileSync(join(noKey, "vars.yaml"), "ticket: JUS-1\n");
		expect(loadProjectGithubId(noKey)).toBeUndefined();

		const blank = mkdtempSync(join(tmpdir(), "queohoh-gh-"));
		writeFileSync(join(blank, "vars.yaml"), "github_id: ''\n");
		expect(loadProjectGithubId(blank)).toBeUndefined();

		const numeric = mkdtempSync(join(tmpdir(), "queohoh-gh-"));
		writeFileSync(join(numeric, "vars.yaml"), "github_id: 12345\n");
		expect(loadProjectGithubId(numeric)).toBeUndefined();
	});

	it("returns undefined (never throws) for a non-mapping vars.yaml", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-gh-"));
		writeFileSync(join(dir, "vars.yaml"), "[not, a, map]\n");
		expect(loadProjectGithubId(dir)).toBeUndefined();
	});
});

describe("loadGlobalConfig — models map", () => {
	it("parses a global models: map and defaults to empty", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-models-"));
		const withModels = join(dir, "with.yaml");
		writeFileSync(
			withModels,
			["projects: []", "models:", "  sonnet: claude-sonnet-4-6"].join("\n"),
		);
		expect(loadGlobalConfig(withModels).models).toEqual({
			sonnet: "claude-sonnet-4-6",
		});
		const bare = join(dir, "bare.yaml");
		writeFileSync(bare, "projects: []\n");
		expect(loadGlobalConfig(bare).models).toEqual({});
	});

	it("tolerates malformed models entries instead of crashing", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-models-bad-"));
		const path = join(dir, "config.yaml");
		writeFileSync(
			path,
			[
				"projects: []",
				"models:",
				"  sonnet: claude-x",
				"  bad: 4.6",
				"  nested:",
				"    a: b",
			].join("\n"),
		);
		expect(() => loadGlobalConfig(path)).not.toThrow();
		expect(loadGlobalConfig(path).models).toEqual({ sonnet: "claude-x" });
	});
});

describe("loadProjectModels", () => {
	it("reads the block and tolerates absence/garbage", () => {
		const withBlock = mkdtempSync(join(tmpdir(), "queohoh-pm-"));
		writeFileSync(
			join(withBlock, "vars.yaml"),
			"ticket: JUS-1\nmodels:\n  sonnet: claude-sonnet-4-6\n",
		);
		expect(loadProjectModels(withBlock)).toEqual({
			sonnet: "claude-sonnet-4-6",
		});

		const withoutVarsYaml = mkdtempSync(join(tmpdir(), "queohoh-pm-"));
		expect(loadProjectModels(withoutVarsYaml)).toEqual({});

		const withGarbage = mkdtempSync(join(tmpdir(), "queohoh-pm-"));
		writeFileSync(join(withGarbage, "vars.yaml"), "models: [not, a, map]\n");
		expect(loadProjectModels(withGarbage)).toEqual({});
	});

	it("skips non-string and empty-string values, keeping valid entries", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pm-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			[
				"models:",
				"  sonnet: claude-sonnet-4-6",
				"  bad: 4.6",
				"  blank: ''",
			].join("\n"),
		);
		expect(loadProjectModels(dir)).toEqual({ sonnet: "claude-sonnet-4-6" });
	});
});

describe("loadProjectProtectedWorktrees", () => {
	it("reads a string list from vars.yaml", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"protected_worktrees:\n  - legal-lake\n  - testing1\n",
		);
		expect(loadProjectProtectedWorktrees(dir)).toEqual([
			"legal-lake",
			"testing1",
		]);
	});

	it("returns [] for absent file or absent key", () => {
		const absent = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		expect(loadProjectProtectedWorktrees(absent)).toEqual([]);

		const noKey = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		writeFileSync(join(noKey, "vars.yaml"), "ticket: JUS-1\n");
		expect(loadProjectProtectedWorktrees(noKey)).toEqual([]);
	});

	it("tolerates a non-list value and skips non-string/empty entries", () => {
		const scalar = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		writeFileSync(
			join(scalar, "vars.yaml"),
			"protected_worktrees: legal-lake\n",
		);
		expect(loadProjectProtectedWorktrees(scalar)).toEqual([]);

		const mixed = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		writeFileSync(
			join(mixed, "vars.yaml"),
			"protected_worktrees:\n  - legal-lake\n  - ''\n  - 12345\n",
		);
		expect(loadProjectProtectedWorktrees(mixed)).toEqual(["legal-lake"]);

		const notMap = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		writeFileSync(join(notMap, "vars.yaml"), "[not, a, map]\n");
		expect(loadProjectProtectedWorktrees(notMap)).toEqual([]);
	});

	it("is not surfaced as a template var by loadProjectVars", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"ticket: JUS-1\nprotected_worktrees:\n  - legal-lake\n",
		);
		expect(loadProjectVars(dir)).toEqual({ ticket: "JUS-1" });
	});
});
