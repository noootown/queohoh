import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { BUILTIN_CATALOG, effectiveCatalog } from "../catalog.js";
import type { GlobalConfig } from "../config.js";
import {
	DEFAULT_PROVIDERS,
	globalWorkspaceDir,
	loadGlobalConfig,
	loadProjectDefaultBranch,
	loadProjectDefaultModels,
	loadProjectGithubId,
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
			catalog: BUILTIN_CATALOG,
			defaultModels: ["claude/opus", "grok/grok-4.5"],
			providers: DEFAULT_PROVIDERS,
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

	it("skips the reserved default_models key instead of exposing it as a var", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pv-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"ticket: JUS-1\ndefault_models:\n  - claude/opus\n",
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

describe("loadGlobalConfig — catalog overlay", () => {
	it("merges a catalog: overlay onto the built-in catalog", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-catalog-"));
		const path = join(dir, "config.yaml");
		writeFileSync(
			path,
			[
				"projects: []",
				"catalog:",
				"  - provider: grok",
				"    id: grok-4.5",
				"    label: grok-4.5",
				"    hidden: true",
				"  - provider: claude",
				"    id: claude-extra-9",
				"    label: extra",
			].join("\n"),
		);
		const config = loadGlobalConfig(path);
		const grokHead = config.catalog.find(
			(e) => e.provider === "grok" && e.id === "grok-4.5",
		);
		expect(grokHead?.hidden).toBe(true);
		const added = config.catalog.find((e) => e.id === "claude-extra-9");
		expect(added).toEqual({
			provider: "claude",
			id: "claude-extra-9",
			label: "extra",
		});
		// Re-grouped by provider precedence — a config reorder cannot interleave
		// providers: every claude entry precedes every grok entry.
		const providerOrder = config.catalog.map((e) => e.provider);
		expect(providerOrder.lastIndexOf("claude")).toBeLessThan(
			providerOrder.indexOf("grok"),
		);
	});

	it("defaults to the built-in catalog unchanged when catalog: is absent", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-catalog-bare-"));
		const path = join(dir, "config.yaml");
		writeFileSync(path, "projects: []\n");
		expect(loadGlobalConfig(path).catalog).toEqual(BUILTIN_CATALOG);
	});

	it("falls back to the built-in catalog and warns on a duplicate-label overlay", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-catalog-dup-"));
		const path = join(dir, "config.yaml");
		writeFileSync(
			path,
			[
				"projects: []",
				"catalog:",
				"  - provider: claude",
				"    id: claude-opus-4-8",
				"    label: sonnet", // collides with the built-in claude/sonnet label
			].join("\n"),
		);
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		const config = loadGlobalConfig(path);
		expect(config.catalog).toEqual(effectiveCatalog(undefined));
		expect(warn).toHaveBeenCalledWith(
			expect.stringContaining("catalog: duplicate label"),
		);
		warn.mockRestore();
	});

	it("falls back to the built-in catalog and warns when catalog: is malformed shape", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-catalog-shape-"));
		const path = join(dir, "config.yaml");
		writeFileSync(path, ["projects: []", "catalog:", "  foo: bar"].join("\n"));
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		const config = loadGlobalConfig(path);
		expect(config.catalog).toEqual(effectiveCatalog(undefined));
		expect(warn).toHaveBeenCalled();
		warn.mockRestore();
	});
});

describe("loadGlobalConfig — default_models", () => {
	it("defaults to claude/opus, grok/grok-4.5 when absent", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-dm-"));
		const path = join(dir, "config.yaml");
		writeFileSync(path, "projects: []\n");
		expect(loadGlobalConfig(path).defaultModels).toEqual([
			"claude/opus",
			"grok/grok-4.5",
		]);
	});

	it("parses a configured default_models: list", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-dm-set-"));
		const path = join(dir, "config.yaml");
		writeFileSync(
			path,
			[
				"projects: []",
				"default_models:",
				"  - grok/grok-4.5",
				"  - claude/sonnet",
			].join("\n"),
		);
		expect(loadGlobalConfig(path).defaultModels).toEqual([
			"grok/grok-4.5",
			"claude/sonnet",
		]);
	});
});

describe("loadGlobalConfig — providers[].models is deprecated", () => {
	it("warns and drops the models key, keeping enabled/bin", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-provmodels-"));
		const path = join(dir, "config.yaml");
		writeFileSync(
			path,
			[
				"projects: []",
				"providers:",
				"  - name: claude",
				"    enabled: true",
				"    models:",
				"      sonnet: claude-sonnet-4-6",
			].join("\n"),
		);
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		const config = loadGlobalConfig(path);
		const claude = config.providers.find((p) => p.name === "claude");
		expect(claude?.enabled).toBe(true);
		expect(claude).not.toHaveProperty("models");
		expect(warn).toHaveBeenCalledWith(
			expect.stringContaining("providers.claude.models"),
		);
		warn.mockRestore();
	});

	it("does not warn when no provider sets models", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-provmodels-none-"));
		const path = join(dir, "config.yaml");
		writeFileSync(
			path,
			[
				"projects: []",
				"providers:",
				"  - name: claude",
				"    enabled: true",
			].join("\n"),
		);
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		loadGlobalConfig(path);
		expect(warn).not.toHaveBeenCalled();
		warn.mockRestore();
	});
});

describe("loadProjectDefaultModels", () => {
	it("reads a default_models: list from vars.yaml", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pdm-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"default_models:\n  - claude/opus\n  - grok/grok-4.5\n",
		);
		expect(loadProjectDefaultModels(dir)).toEqual([
			"claude/opus",
			"grok/grok-4.5",
		]);
	});

	it("returns undefined for absent file, absent key, or a non-list value", () => {
		const absent = mkdtempSync(join(tmpdir(), "queohoh-pdm-"));
		expect(loadProjectDefaultModels(absent)).toBeUndefined();

		const noKey = mkdtempSync(join(tmpdir(), "queohoh-pdm-"));
		writeFileSync(join(noKey, "vars.yaml"), "ticket: JUS-1\n");
		expect(loadProjectDefaultModels(noKey)).toBeUndefined();

		const scalar = mkdtempSync(join(tmpdir(), "queohoh-pdm-"));
		writeFileSync(join(scalar, "vars.yaml"), "default_models: claude/opus\n");
		expect(loadProjectDefaultModels(scalar)).toBeUndefined();
	});

	it("skips non-string/empty entries, keeping valid ones", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pdm-mixed-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"default_models:\n  - claude/opus\n  - ''\n  - 5\n",
		);
		expect(loadProjectDefaultModels(dir)).toEqual(["claude/opus"]);
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
