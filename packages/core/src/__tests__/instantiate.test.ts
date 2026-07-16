import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { TaskDefinition } from "../definition.js";
import { instantiateDefinition } from "../instantiate.js";
import type { Exec } from "../resolver-io.js";
import { QueueStore } from "../store.js";

function def(overrides: Partial<TaskDefinition> = {}): TaskDefinition {
	return {
		name: "pr-review",
		repo: "platform",
		discovery: { command: "gh pr list", itemKey: "{{number}}" },
		description: null,
		cron: null,
		args: [{ name: "number" }],
		dedup: "skip_seen",
		worktree: "pr:{{number}}",
		lane: null,
		preRun: null,
		postRun: null,
		verify: null,
		model: "opus",
		timeoutMs: 1_800_000,
		priority: "high",
		prompt: "Review PR {{number}} for {{github_user}}.\n",
		...overrides,
	};
}

function deps(store: QueueStore, stdout: string) {
	const exec: Exec = async () => ({ stdout, exitCode: 0 });
	return {
		store,
		exec,
		cwd: "/repo",
		source: "cron" as const,
		globalVars: { github_user: "noootown" },
	};
}

const freshStore = () =>
	new QueueStore(mkdtempSync(join(tmpdir(), "qo-inst-")));

describe("instantiateDefinition — discover", () => {
	it("creates one instance per discovered item with rendered fields", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def(),
			{ mode: "discover" },
			deps(store, '[{"number": 257}, {"number": 258}]'),
		);
		expect(created).toHaveLength(2);
		const first = created[0];
		expect(first?.definition).toBe("platform/pr-review");
		expect(first?.item).toEqual({ number: "257" });
		expect(first?.itemKey).toBe("257");
		expect(first?.target).toEqual({
			repo: "platform",
			ref: "pr:257",
			worktree: null,
		});
		expect(first?.priority).toBe("high");
		expect(first?.prompt).toBe("Review PR 257 for noootown.\n");
		expect(store.list()).toHaveLength(2);
	});

	it("dedups against existing instances", async () => {
		const store = freshStore();
		await instantiateDefinition(
			def(),
			{ mode: "discover" },
			deps(store, '[{"number": 257}]'),
		);
		const second = await instantiateDefinition(
			def(),
			{ mode: "discover" },
			deps(store, '[{"number": 257}, {"number": 300}]'),
		);
		expect(second.map((t) => t.itemKey)).toEqual(["300"]);
	});

	it("dedups against archived instances too", async () => {
		const store = freshStore();
		const [made] = await instantiateDefinition(
			def(),
			{ mode: "discover" },
			deps(store, '[{"number": 257}]'),
		);
		store.archive((made as { id: string }).id);
		const again = await instantiateDefinition(
			def(),
			{ mode: "discover" },
			deps(store, '[{"number": 257}]'),
		);
		expect(again).toEqual([]);
	});

	it("renders the discovery command with global + repo vars before exec", async () => {
		const store = freshStore();
		let capturedArgs: string[] = [];
		const exec: Exec = async (_cmd, args) => {
			capturedArgs = args;
			return { stdout: "[]", exitCode: 0 };
		};
		await instantiateDefinition(
			def({
				discovery: {
					command: "bash discover.sh {{github_user}} {{repo_slug}}",
					itemKey: "{{number}}",
				},
			}),
			{ mode: "discover" },
			{
				store,
				exec,
				cwd: "/repo",
				source: "cron",
				globalVars: { github_user: "noootown" },
				repoVars: { repo_slug: "org/repo" },
			},
		);
		expect(capturedArgs).toEqual(["-lc", "bash discover.sh noootown org/repo"]);
	});

	it("throws when definition has no discovery", async () => {
		const store = freshStore();
		await expect(
			instantiateDefinition(
				def({ discovery: null }),
				{ mode: "discover" },
				deps(store, "[]"),
			),
		).rejects.toThrow("definition pr-review has no discovery");
	});
});

describe("instantiateDefinition — args", () => {
	it("zips values onto declared arg names, skipping discovery", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def(),
			{ mode: "args", values: ["257"] },
			deps(store, "SHOULD NOT RUN"),
		);
		expect(created).toHaveLength(1);
		expect(created[0]?.item).toEqual({ number: "257" });
		expect(created[0]?.target.ref).toBe("pr:257");
	});

	it("uses refOverride verbatim instead of the rendered worktree template", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def({ worktree: "temp" }),
			{ mode: "args", values: ["257"] },
			{ ...deps(store, "[]"), refOverride: "worktree:wt-plan-a" },
		);
		expect(created).toHaveLength(1);
		expect(created[0]?.target.ref).toBe("worktree:wt-plan-a");
	});

	it("canonicalizes a pasted URL ref rendered from the worktree template", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def({
				discovery: null,
				args: [{ name: "ref", default: "temp" }],
				dedup: "none",
				worktree: "{{ref}}",
				prompt: "fix it\n",
			}),
			{ mode: "args", values: ["https://github.com/acme/widgets/pull/1821"] },
			deps(store, "[]"),
		);
		expect(created[0]?.target.ref).toBe("pr:1821");
	});

	it("keeps an unparseable rendered ref verbatim for later resolution", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def({
				discovery: null,
				args: [{ name: "ref", default: "temp" }],
				dedup: "none",
				worktree: "{{ref}}",
				prompt: "fix it\n",
			}),
			{ mode: "args", values: ["not-a-ref"] },
			deps(store, "[]"),
		);
		expect(created[0]?.target.ref).toBe("not-a-ref");
	});

	it("derives a PR ref from a situation arg under worktree auto", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def({
				discovery: null,
				args: [{ name: "situation" }],
				dedup: "none",
				worktree: "auto",
				prompt: "{{situation}}\n",
			}),
			{
				mode: "args",
				values: ["fix https://github.com/acme/widgets/pull/1821 please"],
			},
			deps(store, "[]"),
		);
		expect(created[0]?.target.ref).toBe("pr:1821");
	});

	it("derives a ticket ref from a Linear URL under worktree auto", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def({
				discovery: null,
				args: [{ name: "situation" }],
				dedup: "none",
				worktree: "auto",
				prompt: "{{situation}}\n",
			}),
			{
				mode: "args",
				values: ["see https://linear.app/jb/issue/JUS-123-fix-it"],
			},
			deps(store, "[]"),
		);
		expect(created[0]?.target.ref).toBe("ticket:JUS-123");
	});

	it("derives a ticket ref from a leading bare ticket under worktree auto", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def({
				discovery: null,
				args: [{ name: "situation" }],
				dedup: "none",
				worktree: "auto",
				prompt: "{{situation}}\n",
			}),
			{ mode: "args", values: ["JUS-1821: rework the extraction"] },
			deps(store, "[]"),
		);
		expect(created[0]?.target.ref).toBe("ticket:JUS-1821");
	});

	it("falls back to temp when auto finds no ref in plain prose", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def({
				discovery: null,
				args: [{ name: "situation" }],
				dedup: "none",
				worktree: "auto",
				prompt: "{{situation}}\n",
			}),
			{ mode: "args", values: ["just make the tests pass"] },
			deps(store, "[]"),
		);
		expect(created[0]?.target.ref).toBe("temp");
	});

	it("lets refOverride win over worktree auto", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def({
				discovery: null,
				args: [{ name: "situation" }],
				dedup: "none",
				worktree: "auto",
				prompt: "{{situation}}\n",
			}),
			{
				mode: "args",
				values: ["fix https://github.com/acme/widgets/pull/1821"],
			},
			{ ...deps(store, "[]"), refOverride: "worktree:wt-plan-a" },
		);
		expect(created[0]?.target.ref).toBe("worktree:wt-plan-a");
	});

	it("throws when a required arg has no value and no default", async () => {
		const store = freshStore();
		await expect(
			instantiateDefinition(
				def(),
				{ mode: "args", values: [] },
				deps(store, "[]"),
			),
		).rejects.toThrow("missing required arg: number");
	});

	it("throws when more values than declared args are given", async () => {
		const store = freshStore();
		await expect(
			instantiateDefinition(
				def(),
				{ mode: "args", values: ["257", "extra"] },
				deps(store, "[]"),
			),
		).rejects.toThrow("too many args: expected at most 1 (number), got 2");
	});

	it("fills trailing args from their defaults when values are shorter", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def({
				discovery: null,
				args: [
					{ name: "source" },
					{ name: "target", default: "main" },
					{ name: "mode", default: "ready", options: ["ready", "create"] },
				],
				dedup: "none",
				worktree: "temp",
				prompt: "{{source}} -> {{target}} ({{mode}})\n",
			}),
			{ mode: "args", values: ["feature-x"] },
			deps(store, "[]"),
		);
		expect(created).toHaveLength(1);
		expect(created[0]?.item).toEqual({
			source: "feature-x",
			target: "main",
			mode: "ready",
		});
		expect(created[0]?.prompt).toBe("feature-x -> main (ready)\n");
	});

	it("rejects a value outside a declared options set", async () => {
		const store = freshStore();
		await expect(
			instantiateDefinition(
				def({
					args: [{ name: "mode", options: ["ready", "create"] }],
					prompt: "{{mode}}\n",
				}),
				{ mode: "args", values: ["nope"] },
				deps(store, "[]"),
			),
		).rejects.toThrow('arg mode: "nope" not in options (ready, create)');
	});

	it("accepts an explicit value that overrides a default", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def({
				discovery: null,
				args: [{ name: "target", default: "main" }],
				dedup: "none",
				worktree: "temp",
				prompt: "{{target}}\n",
			}),
			{ mode: "args", values: ["develop"] },
			deps(store, "[]"),
		);
		expect(created[0]?.item).toEqual({ target: "develop" });
	});

	it("args mode still dedups", async () => {
		const store = freshStore();
		await instantiateDefinition(
			def(),
			{ mode: "args", values: ["257"] },
			deps(store, "[]"),
		);
		const again = await instantiateDefinition(
			def(),
			{ mode: "args", values: ["257"] },
			deps(store, "[]"),
		);
		expect(again).toEqual([]);
	});

	it("stamps resumeSessionId on every created task when provided", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def(),
			{ mode: "args", values: ["257"] },
			{ ...deps(store, "[]"), resumeSessionId: "sess-pin" },
		);
		expect(created).toHaveLength(1);
		expect(created[0]?.resumeSessionId).toBe("sess-pin");
	});

	it("leaves resumeSessionId null when not provided", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def(),
			{ mode: "args", values: ["258"] },
			deps(store, "[]"),
		);
		expect(created[0]?.resumeSessionId).toBeNull();
	});
});

describe("instantiateDefinition — cron dedup coercion", () => {
	const discoveryless = () =>
		def({ discovery: null, args: [], dedup: "skip_seen", worktree: "repo" });

	it("fires a discovery-less skip_seen def more than once when source is cron", async () => {
		const store = freshStore();
		const d = deps(store, ""); // deps() defaults source to "cron"
		const first = await instantiateDefinition(
			discoveryless(),
			{ mode: "args", values: [] },
			d,
		);
		const second = await instantiateDefinition(
			discoveryless(),
			{ mode: "args", values: [] },
			d,
		);
		expect(first).toHaveLength(1);
		expect(second).toHaveLength(1); // NOT deduped away — cursor owns fire-timing
	});

	it("still dedups a discovery-less skip_seen def when source is NOT cron", async () => {
		const store = freshStore();
		const d = { ...deps(store, ""), source: "tui" as const };
		const first = await instantiateDefinition(
			discoveryless(),
			{ mode: "args", values: [] },
			d,
		);
		const second = await instantiateDefinition(
			discoveryless(),
			{ mode: "args", values: [] },
			d,
		);
		expect(first).toHaveLength(1);
		expect(second).toHaveLength(0); // skip_seen blocks the repeat
	});
});
