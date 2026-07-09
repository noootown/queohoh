import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { listDefinitions, loadDefinition } from "../definition.js";

function makeRepo(defs: Record<string, { config: string; prompt: string }>) {
	const projectDir = mkdtempSync(join(tmpdir(), "queohoh-repo-"));
	for (const [name, files] of Object.entries(defs)) {
		const dir = join(projectDir, "tasks", name);
		mkdirSync(dir, { recursive: true });
		writeFileSync(join(dir, "config.yaml"), files.config);
		writeFileSync(join(dir, "prompt.md"), files.prompt);
	}
	return projectDir;
}

const PR_REVIEW_CONFIG = `
discovery:
  command: gh pr list --json number,title
  item_key: "{{number}}"
args: [number]
worktree: "pr:{{number}}"
pre_run: mise run setup
model: opus
timeout: 45m
priority: high
`;

describe("loadDefinition", () => {
	it("loads a full definition with defaults applied", () => {
		const projectDir = makeRepo({
			"pr-review": {
				config: PR_REVIEW_CONFIG,
				prompt: "Review PR {{number}}.\n",
			},
		});
		const def = loadDefinition(projectDir, "platform", "pr-review");
		expect(def).toEqual({
			name: "pr-review",
			repo: "platform",
			discovery: {
				command: "gh pr list --json number,title",
				itemKey: "{{number}}",
			},
			args: ["number"],
			dedup: "skip_seen",
			worktree: "pr:{{number}}",
			preRun: "mise run setup",
			postRun: null,
			model: "opus",
			timeoutMs: 2_700_000,
			priority: "high",
			prompt: "Review PR {{number}}.\n",
		});
	});

	it("applies defaults for a minimal config", () => {
		const projectDir = makeRepo({
			tidy: { config: "{}", prompt: "Tidy up.\n" },
		});
		const def = loadDefinition(projectDir, "platform", "tidy");
		expect(def.dedup).toBe("skip_seen");
		expect(def.worktree).toBe("temp");
		expect(def.model).toBe("sonnet");
		expect(def.timeoutMs).toBe(1_800_000);
		expect(def.priority).toBe("normal");
		expect(def.discovery).toBeNull();
		expect(def.args).toEqual([]);
	});

	it("rejects a bad dedup value", () => {
		const projectDir = makeRepo({
			bad: { config: "dedup: sometimes", prompt: "x" },
		});
		expect(() => loadDefinition(projectDir, "platform", "bad")).toThrow();
	});

	it("rejects an unknown/typo'd config key", () => {
		const projectDir = makeRepo({
			typo: { config: "timout: 5m", prompt: "x" },
		});
		expect(() => loadDefinition(projectDir, "platform", "typo")).toThrow();
	});
});

describe("listDefinitions", () => {
	it("lists all definition folders", () => {
		const projectDir = makeRepo({
			a: { config: "{}", prompt: "a" },
			b: { config: "{}", prompt: "b" },
		});
		expect(listDefinitions(projectDir, "platform").map((d) => d.name)).toEqual([
			"a",
			"b",
		]);
	});

	it("returns [] when tasks dir is absent", () => {
		const projectDir = mkdtempSync(join(tmpdir(), "queohoh-empty-"));
		expect(listDefinitions(projectDir, "platform")).toEqual([]);
	});
});
