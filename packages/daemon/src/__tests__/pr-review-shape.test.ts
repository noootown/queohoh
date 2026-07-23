import { chmodSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Exec, GlobalConfig, ResolverIO, RunResult } from "@queohoh/core";
import {
	BUILTIN_CATALOG,
	DEFAULT_PROVIDERS,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionLineageStore,
	SessionRegistry,
} from "@queohoh/core";
import { afterEach, describe, expect, it } from "vitest";
import { ApiServer } from "../api.js";
import { ApiClient } from "../client.js";
import { Engine } from "../engine.js";
import { SettingsStore } from "../settings-store.js";

const cleanups: (() => Promise<void> | void)[] = [];
afterEach(async () => {
	while (cleanups.length) await cleanups.pop()?.();
});

const DISCOVER_SH = `#!/bin/bash
# stub of agent247 discover.sh — emits one PR shaped like the real script's output.
# Validates its args so an unrendered template ({{github_username}} etc) fails loudly.
if [ "$1" != "ianchiu-jb" ] || [ "$2" != "justicebid/platform" ]; then
  echo "unrendered or wrong args: $1 $2" >&2; exit 1
fi
echo '[{"number": 1423, "title": "Fix auth", "url": "https://github.com/justicebid/platform/pull/1423", "additions": 10, "deletions": 2, "headRefName": "JUS-1423-fix-auth", "baseRefName": "main", "author_login": "kevin", "total_changes": 12, "worktree_path": "/x"}]'
`;

const CONFIG_YAML = `discovery:
  command: bash tasks/pr-review/discover.sh {{github_username}} {{platform_repo}} {{platform_repo_path}}
  item_key: "{{url}}"
dedup: skip_seen
worktree: "pr:{{number}}"
model: claude/claude-opus-4.8
timeout: 30m
priority: normal
`;

const PROMPT_MD = `You are reviewing PR #{{number}} on {{platform_repo}} as {{github_username}}.
Title: {{title}} ({{total_changes}} changes)
`;

async function setup() {
	const base = mkdtempSync(join(tmpdir(), "qo-prshape-"));
	const repoPath = join(base, "repo");
	mkdirSync(repoPath, { recursive: true });
	const taskDir = join(base, "ws", "platform", "tasks", "pr-review");
	mkdirSync(taskDir, { recursive: true });
	writeFileSync(join(taskDir, "config.yaml"), CONFIG_YAML);
	writeFileSync(join(taskDir, "prompt.md"), PROMPT_MD);
	writeFileSync(join(taskDir, "discover.sh"), DISCOVER_SH);
	chmodSync(join(taskDir, "discover.sh"), 0o755);
	writeFileSync(
		join(base, "ws", "platform", "vars.yaml"),
		"platform_repo: justicebid/platform\nplatform_repo_path: /repo/path\n",
	);

	const store = new QueueStore(join(base, "state"));
	const runStore = new RunStore(join(base, "runs"));
	const registry = new SessionRegistry(join(base, "sessions.json"));
	const lineage = new SessionLineageStore(join(base, "session-lineage.json"));
	const config: GlobalConfig = {
		projects: [{ name: "platform", path: repoPath }],
		workspace: join(base, "ws"),
		maxConcurrentTasks: 3,
		purgeAfterDays: 7,
		archiveAfterDays: 7,
		vars: { github_username: "ianchiu-jb" },
		catalog: BUILTIN_CATALOG,
		defaultModels: ["claude/claude-opus-4.8", "grok/grok-4.5"],
		providers: DEFAULT_PROVIDERS,
	};
	const okResult: RunResult = {
		exitCode: 0,
		timedOut: false,
		signal: null,
		sessionId: null,
		resultText: "ok",
		stderr: "",
		usage: {
			costUsd: 0,
			turns: 1,
			durationMs: 1,
			inputTokens: null,
			outputTokens: null,
		},
	};
	const exec: Exec = async () => ({ stdout: "", exitCode: 0 });
	const resolverIO: ResolverIO = {
		listWorktrees: async () => [],
		prBranch: async () => null,
		spawnWorktree: async (_r, name) => ({
			name,
			path: `/wt/${name}`,
			branch: name,
		}),
		removeWorktree: async () => {},
	};
	const engine = new Engine({
		store,
		runStore,
		registry,
		config,
		resolverIO,
		exec,
		executeClaude: async () => okResult,
		executeVerify: async () => ({
			exitCode: 0,
			timedOut: false,
			signal: null,
			output: "",
		}),
		redact: makeRedactor(new Map()),
		lineage,
	});
	const server = new ApiServer({
		engine,
		store,
		runStore,
		registry,
		config,
		settings: new SettingsStore(join(base, "state"), config.providers),
		lineage,
		onMutation: () => {},
	});
	const sock = join(base, "d.sock");
	await server.listen(sock);
	const client = new ApiClient();
	await client.connect(sock);
	cleanups.push(() => client.close());
	cleanups.push(() => server.close());
	return { client, store };
}

describe("agent247 pr-review port shape", () => {
	it("lists the definition from the workspace", async () => {
		const { client } = await setup();
		const defs = (await client.call("definitions")) as {
			repo: string;
			name: string;
			scope: string;
			args: unknown[];
			hasDiscovery: boolean;
			cron: string | null;
			cronEnabled: boolean;
			description: string | null;
			model: string;
		}[];
		expect(defs).toEqual([
			{
				repo: "platform",
				name: "pr-review",
				scope: "project",
				args: [],
				hasDiscovery: true,
				cron: null,
				// No cron on this def, and never toggled → the summary still reports
				// armed (the field is meaningful only when `cron` is set).
				cronEnabled: true,
				description: null,
				// summary forwards the authored `provider/label` ref as-is (there is
				// no alias table to resolve against anymore).
				model: "claude/claude-opus-4.8",
				worktree: "pr:{{number}}",
			},
		]);
	});

	it("discoverDefinition discovers via discover.sh (cwd = project workspace dir) and instantiates with rendered prompt, ref, key", async () => {
		const { client, store } = await setup();
		const created = (await client.call("discoverDefinition", {
			repo: "platform",
			name: "pr-review",
		})) as { prompt: string }[];
		expect(created).toHaveLength(1);
		const task = store.list()[0];
		expect(task?.definition).toBe("platform/pr-review");
		expect(task?.itemKey).toBe(
			"https://github.com/justicebid/platform/pull/1423",
		);
		expect(task?.target.ref).toBe("pr:1423");
		expect(task?.prompt).toContain(
			"reviewing PR #1423 on justicebid/platform as ianchiu-jb",
		);
		expect(task?.prompt).toContain("Fix auth (12 changes)");
		expect(task?.item?.headRefName).toBe("JUS-1423-fix-auth");
	});

	it("re-running dedups on url", async () => {
		const { client } = await setup();
		await client.call("discoverDefinition", {
			repo: "platform",
			name: "pr-review",
		});
		const second = (await client.call("discoverDefinition", {
			repo: "platform",
			name: "pr-review",
		})) as unknown[];
		expect(second).toEqual([]);
	});
});
