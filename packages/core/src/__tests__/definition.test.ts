import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	definitionExists,
	listDefinitions,
	loadDefinition,
} from "../definition.js";

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
description: Review an open PR end to end.
discovery:
  command: gh pr list --json number,title
  item_key: "{{number}}"
cron: "30 13 * * *"
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
			description: "Review an open PR end to end.",
			discovery: {
				command: "gh pr list --json number,title",
				itemKey: "{{number}}",
			},
			cron: "30 13 * * *",
			args: [{ name: "number" }],
			dedup: "skip_seen",
			worktree: "pr:{{number}}",
			preRun: "mise run setup",
			postRun: null,
			verify: null,
			model: "opus",
			timeoutMs: 2_700_000,
			priority: "high",
			prompt: "Review PR {{number}}.\n",
		});
	});

	it("loads a verify (done-condition) command", () => {
		const projectDir = makeRepo({
			"pr-ready": {
				config:
					"verify: gh pr view --json labels -q '.labels[].name' | grep -qx ready-for-review",
				prompt: "Flip the PR to ready.\n",
			},
		});
		const def = loadDefinition(projectDir, "platform", "pr-ready");
		expect(def.verify).toBe(
			"gh pr view --json labels -q '.labels[].name' | grep -qx ready-for-review",
		);
	});

	it("defaults verify to null when absent", () => {
		const projectDir = makeRepo({
			tidy: { config: "{}", prompt: "Tidy up.\n" },
		});
		expect(loadDefinition(projectDir, "platform", "tidy").verify).toBeNull();
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
		expect(def.cron).toBeNull();
		expect(def.description).toBeNull();
		expect(def.args).toEqual([]);
	});

	it("parses a top-level description string", () => {
		const projectDir = makeRepo({
			squash: {
				config: "description: Squash a branch into the target.",
				prompt: "x",
			},
		});
		const def = loadDefinition(projectDir, "platform", "squash");
		expect(def.description).toBe("Squash a branch into the target.");
	});

	it("rejects an empty description string", () => {
		const projectDir = makeRepo({
			bad: { config: 'description: ""', prompt: "x" },
		});
		expect(() => loadDefinition(projectDir, "platform", "bad")).toThrow();
	});

	it("parses a top-level cron schedule string", () => {
		const projectDir = makeRepo({
			nightly: { config: 'cron: "0 9 * * 1-5"', prompt: "x" },
		});
		const def = loadDefinition(projectDir, "platform", "nightly");
		expect(def.cron).toBe("0 9 * * 1-5");
	});

	it("rejects an empty cron string", () => {
		const projectDir = makeRepo({
			bad: { config: 'cron: ""', prompt: "x" },
		});
		expect(() => loadDefinition(projectDir, "platform", "bad")).toThrow();
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

describe("loadDefinition — rich args", () => {
	it("normalizes shorthand strings and object entries to ArgSpec[]", () => {
		const projectDir = makeRepo({
			rich: {
				config: `
args:
  - pr
  - name: mode
    default: ready
    options: [ready, create]
    description: hand off or keep WIP
`,
				prompt: "x",
			},
		});
		const def = loadDefinition(projectDir, "platform", "rich");
		expect(def.args).toEqual([
			{ name: "pr" },
			{
				name: "mode",
				default: "ready",
				options: ["ready", "create"],
				description: "hand off or keep WIP",
			},
		]);
	});

	it("rejects duplicate arg names", () => {
		const projectDir = makeRepo({
			dup: { config: "args: [pr, pr]", prompt: "x" },
		});
		expect(() => loadDefinition(projectDir, "platform", "dup")).toThrow(
			/duplicate arg name: pr/,
		);
	});

	it("rejects a default that is not a member of options", () => {
		const projectDir = makeRepo({
			bad: {
				config: `
args:
  - name: mode
    default: nope
    options: [ready, create]
`,
				prompt: "x",
			},
		});
		expect(() => loadDefinition(projectDir, "platform", "bad")).toThrow(
			/default "nope" not in options/,
		);
	});

	it("rejects an unknown key inside an arg object", () => {
		const projectDir = makeRepo({
			badkey: {
				config: "args:\n  - name: mode\n    typo: 1\n",
				prompt: "x",
			},
		});
		expect(() => loadDefinition(projectDir, "platform", "badkey")).toThrow();
	});

	it("accepts type: worktree and rejects type+options together", () => {
		const projectDir = makeRepo({
			targeted: {
				config: "args:\n  - name: pr\n    type: worktree\n",
				prompt: "x",
			},
		});
		const def = loadDefinition(projectDir, "platform", "targeted");
		expect(def.args).toEqual([{ name: "pr", type: "worktree" }]);

		const badProjectDir = makeRepo({
			bad: {
				config: "args:\n  - name: pr\n    type: worktree\n    options: [a]\n",
				prompt: "x",
			},
		});
		expect(() => loadDefinition(badProjectDir, "platform", "bad")).toThrow(
			/type.*worktree.*options/i,
		);
	});

	it("accepts type: branch and type: text", () => {
		const projectDir = makeRepo({
			branchy: {
				config:
					"args:\n  - name: target\n    type: branch\n    default: main\n  - name: situation\n    type: text\n",
				prompt: "x",
			},
		});
		const def = loadDefinition(projectDir, "platform", "branchy");
		expect(def.args).toEqual([
			{ name: "target", type: "branch", default: "main" },
			{ name: "situation", type: "text" },
		]);
	});

	it("rejects type: branch combined with options", () => {
		const badProjectDir = makeRepo({
			bad: {
				config: "args:\n  - name: target\n    type: branch\n    options: [a]\n",
				prompt: "x",
			},
		});
		expect(() => loadDefinition(badProjectDir, "platform", "bad")).toThrow(
			/type.*branch.*options/i,
		);
	});
});

describe("definitionExists", () => {
	it("is true for a present definition and false otherwise", () => {
		const projectDir = makeRepo({ a: { config: "{}", prompt: "a" } });
		expect(definitionExists(projectDir, "a")).toBe(true);
		expect(definitionExists(projectDir, "missing")).toBe(false);
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
