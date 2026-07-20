import { mkdirSync, mkdtempSync, realpathSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { GlobalConfig, NewTaskInput } from "@queohoh/core";
import {
	BUILTIN_CATALOG,
	DEFAULT_PROVIDERS,
	DiscussStore,
	definitionExists,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionLineageStore,
	SessionRegistry,
} from "@queohoh/core";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiServer } from "../api.js";
import { ApiClient } from "../client.js";
import { DiscussService } from "../discuss-service.js";
import { Engine } from "../engine.js";
import { SettingsStore } from "../settings-store.js";

const cleanups: (() => Promise<void> | void)[] = [];
afterEach(async () => {
	while (cleanups.length) await cleanups.pop()?.();
});

function makeWorktree(): string {
	const wt = mkdtempSync(join(tmpdir(), "discuss-promote-wt-"));
	mkdirSync(wt, { recursive: true });
	// DiscussStore realpathSyncs worktrees; resolveCwd matches on path equality.
	// On macOS /var → /private/var, so return the canonical path.
	return realpathSync(wt);
}

function baseConfig(workspace: string, repoPath: string): GlobalConfig {
	return {
		workspace,
		projects: [{ name: "platform", path: repoPath }],
		maxConcurrentTasks: 3,
		archiveAfterDays: 7,
		vars: {},
		catalog: BUILTIN_CATALOG,
		defaultModels: ["claude/claude-opus-4.8"],
		providers: DEFAULT_PROVIDERS,
	};
}

function okRun() {
	return {
		exitCode: 0,
		timedOut: false,
		signal: null,
		sessionId: "prov-1",
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
}

describe("DiscussService.promoteFix", () => {
	it("enqueues ad-hoc task with transcript tail + note on discuss worktree", async () => {
		const base = mkdtempSync(join(tmpdir(), "qo-promote-fix-"));
		const wt = makeWorktree();
		const discussStore = new DiscussStore(join(base, "discuss"));
		const config = baseConfig(join(base, "ws"), join(base, "repo"));
		const settings = new SettingsStore(join(base, "state"), config.providers);

		const created: NewTaskInput[] = [];
		const create = vi.fn((input: NewTaskInput) => {
			created.push(input);
			return { id: "01TASKFIX00000000000000000" };
		});
		const resolveCwd = vi.fn(async () => ({
			repo: "platform",
			worktree: "platform.feat-x",
		}));

		const discuss = new DiscussService({
			store: discussStore,
			lineage: new SessionLineageStore(join(base, "lineage.json")),
			settings,
			config,
			redact: makeRedactor(new Map()),
			queue: { create, resolveCwd },
		});

		const meta = discuss.ensure(wt);
		// Marker near the end so the 8k tail always includes it.
		const marker = "MUST_FIX_NULL_CHECK_IN_FOO";
		discussStore.appendTranscript(
			meta.sessionId,
			`User: the null check is wrong\nAssistant: agree — fix ${marker}\n`,
		);

		const result = await discuss.promoteFix(meta.sessionId, "also add a test");
		expect(result.task_id).toBe("01TASKFIX00000000000000000");
		expect(create).toHaveBeenCalledOnce();
		expect(resolveCwd).toHaveBeenCalledWith(meta.worktree);

		const input = created[0];
		expect(input).toBeDefined();
		expect(input?.prompt).toContain(marker);
		expect(input?.prompt).toContain("also add a test");
		expect(input?.repo).toBe("platform");
		expect(input?.ref).toBe("worktree:platform.feat-x");
		// Full agent queue task (not discuss reserved session).
		expect(input?.session ?? "fresh").toBe("fresh");
	});

	it("trims transcript to last ~8k chars in the promote prompt", async () => {
		const base = mkdtempSync(join(tmpdir(), "qo-promote-8k-"));
		const wt = makeWorktree();
		const discussStore = new DiscussStore(join(base, "discuss"));
		const config = baseConfig(join(base, "ws"), join(base, "repo"));
		const settings = new SettingsStore(join(base, "state"), config.providers);

		const created: NewTaskInput[] = [];
		const discuss = new DiscussService({
			store: discussStore,
			lineage: new SessionLineageStore(join(base, "lineage.json")),
			settings,
			config,
			redact: makeRedactor(new Map()),
			queue: {
				create: (input) => {
					created.push(input);
					return { id: "01TASK8K000000000000000000" };
				},
				resolveCwd: async () => ({ repo: "platform", worktree: "wt" }),
			},
		});

		const meta = discuss.ensure(wt);
		const early = "EARLY_MARKER_SHOULD_DROP";
		const late = "LATE_MARKER_MUST_KEEP";
		discussStore.appendTranscript(
			meta.sessionId,
			`${early}${"x".repeat(9000)}${late}`,
		);

		await discuss.promoteFix(meta.sessionId);
		const prompt = created[0]?.prompt;
		expect(prompt).toContain(late);
		expect(prompt).not.toContain(early);
	});

	it("throws when queue deps are not wired", async () => {
		const base = mkdtempSync(join(tmpdir(), "qo-promote-noq-"));
		const wt = makeWorktree();
		const discussStore = new DiscussStore(join(base, "discuss"));
		const config = baseConfig(join(base, "ws"), join(base, "repo"));
		const discuss = new DiscussService({
			store: discussStore,
			lineage: new SessionLineageStore(join(base, "lineage.json")),
			settings: new SettingsStore(join(base, "state"), config.providers),
			config,
			redact: makeRedactor(new Map()),
		});
		const meta = discuss.ensure(wt);
		await expect(discuss.promoteFix(meta.sessionId)).rejects.toThrow(/queue/i);
	});
});

describe("DiscussService.promotePrReply", () => {
	it("uses tryRunDefinition when pr-reply is available", async () => {
		const base = mkdtempSync(join(tmpdir(), "qo-promote-prdef-"));
		const wt = makeWorktree();
		const discussStore = new DiscussStore(join(base, "discuss"));
		const config = baseConfig(join(base, "ws"), join(base, "repo"));

		const tryRunDefinition = vi.fn(async () => ({
			id: "01TASKPRREPLY0000000000000",
		}));
		const create = vi.fn();

		const discuss = new DiscussService({
			store: discussStore,
			lineage: new SessionLineageStore(join(base, "lineage.json")),
			settings: new SettingsStore(join(base, "state"), config.providers),
			config,
			redact: makeRedactor(new Map()),
			queue: {
				create,
				resolveCwd: async () => ({
					repo: "platform",
					worktree: "platform.pr-42",
				}),
				tryRunDefinition,
			},
		});

		const meta = discuss.ensure(wt);
		discussStore.appendTranscript(meta.sessionId, "discussion about the PR\n");

		const result = await discuss.promotePrReply(
			meta.sessionId,
			"The guard already covers this.",
			42,
		);
		expect(result.task_id).toBe("01TASKPRREPLY0000000000000");
		expect(tryRunDefinition).toHaveBeenCalledWith(
			expect.objectContaining({
				repo: "platform",
				name: "pr-reply",
				args: ["The guard already covers this.", "42"],
			}),
		);
		expect(create).not.toHaveBeenCalled();
	});

	it("falls back to ad-hoc enqueue with body + gh instructions when def missing", async () => {
		const base = mkdtempSync(join(tmpdir(), "qo-promote-pradhoc-"));
		const wt = makeWorktree();
		const discussStore = new DiscussStore(join(base, "discuss"));
		const config = baseConfig(join(base, "ws"), join(base, "repo"));

		const created: NewTaskInput[] = [];
		const discuss = new DiscussService({
			store: discussStore,
			lineage: new SessionLineageStore(join(base, "lineage.json")),
			settings: new SettingsStore(join(base, "state"), config.providers),
			config,
			redact: makeRedactor(new Map()),
			queue: {
				create: (input) => {
					created.push(input);
					return { id: "01TASKADHOC000000000000000" };
				},
				resolveCwd: async () => ({
					repo: "platform",
					worktree: "platform.pr-7",
				}),
				tryRunDefinition: async () => null,
			},
		});

		const meta = discuss.ensure(wt);
		const result = await discuss.promotePrReply(
			meta.sessionId,
			"Nit: drop the redundant check.",
			7,
		);
		expect(result.task_id).toBe("01TASKADHOC000000000000000");
		const prompt = created[0]?.prompt;
		expect(prompt).toContain("Nit: drop the redundant check.");
		expect(prompt).toContain("gh pr comment");
		expect(prompt).toMatch(/7/);
		expect(created[0]?.repo).toBe("platform");
		expect(created[0]?.ref).toBe("worktree:platform.pr-7");
	});

	it("with path+line enqueues inline review-comment task (not conversation)", async () => {
		const base = mkdtempSync(join(tmpdir(), "qo-promote-inline-"));
		const wt = makeWorktree();
		const discussStore = new DiscussStore(join(base, "discuss"));
		const config = baseConfig(join(base, "ws"), join(base, "repo"));

		const created: NewTaskInput[] = [];
		const tryRunDefinition = vi.fn(async () => ({
			id: "01SHOULDNOTUSEPRREPLYDEF000",
		}));
		const discuss = new DiscussService({
			store: discussStore,
			lineage: new SessionLineageStore(join(base, "lineage.json")),
			settings: new SettingsStore(join(base, "state"), config.providers),
			config,
			redact: makeRedactor(new Map()),
			queue: {
				create: (input) => {
					created.push(input);
					return { id: "01TASKINLINE00000000000000" };
				},
				resolveCwd: async () => ({
					repo: "platform",
					worktree: "platform.pr-12",
				}),
				// Def would post conversation — inline path must skip it.
				tryRunDefinition,
			},
		});

		const meta = discuss.ensure(wt);
		const result = await discuss.promotePrReply(
			meta.sessionId,
			"## Comment to post (shape into a PR review comment)\n\ntest\n",
			12,
			{
				path: "apps/portal/tests/list.spec.ts",
				line: 130,
				side: "new",
			},
		);
		expect(result.task_id).toBe("01TASKINLINE00000000000000");
		expect(tryRunDefinition).not.toHaveBeenCalled();
		const prompt = created[0]?.prompt ?? "";
		expect(prompt).toContain("INLINE");
		expect(prompt).toContain("apps/portal/tests/list.spec.ts");
		expect(prompt).toContain("line: 130");
		expect(prompt).toContain("side: RIGHT");
		expect(prompt).toContain("pulls/$PR/comments");
		expect(prompt).toContain("test");
		// Must not instruct conversation-level comment as the write path.
		expect(prompt).not.toMatch(/The ONLY write is `gh pr comment`/);
	});
});

describe("extractCommentBody / inline prompt helpers", () => {
	it("extracts operator section from juice structured draft", async () => {
		const { extractCommentBody, buildAdHocInlineCommentPrompt } =
			await import("../discuss-service.js");
		const draft = [
			"## Focus line",
			"",
			"`@a.ts#L10 (new)`",
			"",
			"## Comment to post (shape into a PR review comment)",
			"",
			"leave a test comment",
			"",
		].join("\n");
		expect(extractCommentBody(draft)).toBe("leave a test comment");
		const prompt = buildAdHocInlineCommentPrompt(draft, "9", {
			path: "a.ts",
			line: 10,
			side: "RIGHT",
		});
		expect(prompt).toContain("leave a test comment");
		expect(prompt).toContain("-F line=10");
		expect(prompt).toContain("-f side=RIGHT");
	});
});

describe("discuss_promote_* RPCs", () => {
	async function setupApi(opts?: {
		worktreeName?: string;
		/** Install pr-reply under platform project tasks (project-local). */
		installPrReply?: boolean;
	}) {
		const base = mkdtempSync(join(tmpdir(), "qo-promote-api-"));
		const stateDir = join(base, "state");
		const workspace = join(base, "ws");
		const repoPath = join(base, "repo");
		const wtPath = makeWorktree();
		mkdirSync(repoPath, { recursive: true });
		mkdirSync(workspace, { recursive: true });

		if (opts?.installPrReply) {
			const defDir = join(workspace, "platform", "tasks", "pr-reply");
			mkdirSync(defDir, { recursive: true });
			writeFileSync(
				join(defDir, "config.yaml"),
				`description: Post a GitHub PR conversation comment from a review draft.
args:
  - name: body
    description: markdown body of the comment
    type: text
  - name: pr
    description: PR number (optional if gh can detect from branch)
    default: ""
dedup: none
worktree: repo
timeout: 10m
`,
			);
			writeFileSync(
				join(defDir, "prompt.md"),
				`Post this PR comment body via gh.

## Body
{{body}}

## PR
{{pr}}

Run: gh pr comment {{pr}} --body "$(cat <<'EOF'\\n{{body}}\\nEOF\\n)"
`,
			);
			expect(definitionExists(join(workspace, "platform"), "pr-reply")).toBe(
				true,
			);
		}

		const store = new QueueStore(stateDir);
		const runStore = new RunStore(join(base, "runs"));
		const registry = new SessionRegistry(join(base, "sessions.json"));
		const lineage = new SessionLineageStore(join(base, "session-lineage.json"));
		const config = baseConfig(workspace, repoPath);
		const settings = new SettingsStore(stateDir, config.providers);
		const discussStore = new DiscussStore(join(base, "discuss"));

		const wtName = opts?.worktreeName ?? "platform.feat";
		const engine = new Engine({
			store,
			runStore,
			registry,
			config,
			resolverIO: {
				listWorktrees: async () => [
					{
						name: wtName,
						path: wtPath,
						branch: "feat",
					},
				],
				prBranch: async () => null,
				spawnWorktree: async (_r, name) => ({
					name,
					path: `/wt/${name}`,
					branch: name,
				}),
				removeWorktree: async () => {},
			},
			exec: async () => ({ stdout: "", exitCode: 0 }),
			executeClaude: async () => okRun(),
			executeVerify: async () => ({
				exitCode: 0,
				timedOut: false,
				signal: null,
				output: "",
			}),
			redact: makeRedactor(new Map()),
			lineage,
		});

		// Production-shaped wiring: DiscussService gets real store + resolveCwd
		// + tryRunDefinition (mirrors daemon.ts).
		const {
			definitionExists: defExists,
			globalWorkspaceDir,
			instantiateDefinition,
			loadProjectVars,
			projectWorkspaceDir,
			resolveDefinition,
			defaultExec,
		} = await import("@queohoh/core");

		const discuss = new DiscussService({
			store: discussStore,
			lineage,
			settings,
			config,
			redact: makeRedactor(new Map()),
			queue: {
				create: (input) => store.create(input),
				resolveCwd: (cwd) => engine.resolveCwd(cwd),
				tryRunDefinition: async ({ repo, name, args }) => {
					const project = config.projects.find((p) => p.name === repo);
					if (!project) return null;
					const projectDir = projectWorkspaceDir(config, repo);
					const globalDir = globalWorkspaceDir(config);
					if (!defExists(projectDir, name) && !defExists(globalDir, name)) {
						return null;
					}
					try {
						const def = resolveDefinition(config, repo, name);
						const created = await instantiateDefinition(
							def,
							{ mode: "args", values: args },
							{
								store,
								exec: defaultExec,
								cwd: projectDir,
								source: "tui",
								globalVars: {
									project: repo,
									repo_path: project.path,
									...config.vars,
								},
								repoVars: loadProjectVars(projectDir),
								bypassDedup: true,
							},
						);
						return created[0] ? { id: created[0].id } : null;
					} catch {
						return null;
					}
				},
			},
		});

		const server = new ApiServer({
			engine,
			store,
			runStore,
			registry,
			config,
			settings,
			lineage,
			discuss,
			onMutation: () => {},
		});
		const sock = join(base, "d.sock");
		await server.listen(sock);
		const client = new ApiClient();
		await client.connect(sock);
		cleanups.push(() => client.close());
		cleanups.push(() => server.close());

		return { client, store, discussStore, wtPath, wtName };
	}

	it("discuss_promote_fix creates a queued task from transcript", async () => {
		const { client, store, discussStore, wtPath } = await setupApi();
		const ensured = (await client.call("discuss_ensure", {
			worktree: wtPath,
		})) as { session_id: string };
		discussStore.appendTranscript(
			ensured.session_id,
			"User: fix the race\nAssistant: will guard with a mutex\n",
		);

		const result = (await client.call("discuss_promote_fix", {
			session_id: ensured.session_id,
			note: "prefer std::mutex",
		})) as { task_id: string };
		expect(result.task_id).toBeTruthy();

		const task = store.get(result.task_id);
		expect(task).toBeDefined();
		expect(task?.status).toBe("queued");
		expect(task?.prompt).toContain("mutex");
		expect(task?.prompt).toContain("prefer std::mutex");
		expect(task?.target.ref).toMatch(/^worktree:/);
	});

	it("discuss_promote_pr_reply enqueues via installed pr-reply def", async () => {
		const { client, store, discussStore, wtPath } = await setupApi({
			installPrReply: true,
		});
		const ensured = (await client.call("discuss_ensure", {
			worktree: wtPath,
		})) as { session_id: string };
		discussStore.appendTranscript(ensured.session_id, "some discuss\n");

		const result = (await client.call("discuss_promote_pr_reply", {
			session_id: ensured.session_id,
			draft: "The early return already handles this.",
			pr: 99,
		})) as { task_id: string };
		expect(result.task_id).toBeTruthy();

		const task = store.get(result.task_id);
		expect(task).toBeDefined();
		expect(task?.definition).toBe("platform/pr-reply");
		expect(task?.prompt).toContain("The early return already handles this.");
		expect(task?.item?.pr).toBe("99");
	});
});
