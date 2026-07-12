import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Exec, GlobalConfig, ResolverIO, RunResult } from "@queohoh/core";
import {
	createResolverIO,
	DEFAULT_MODEL_ALIASES,
	MainSessionStore,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionRegistry,
} from "@queohoh/core";
import { afterEach, describe, expect, it } from "vitest";
import { ApiServer } from "../api.js";
import { ApiClient } from "../client.js";
import { Engine } from "../engine.js";
import { configPath } from "../paths.js";

const cleanups: (() => Promise<void> | void)[] = [];
afterEach(async () => {
	while (cleanups.length) await cleanups.pop()?.();
});

async function setup(opts?: {
	worktrees?: { name: string; path: string; branch: string }[];
	execCalls?: { command: string; args: string[] }[];
	execExitCode?: number;
	executeClaude?: () => Promise<RunResult>;
	vars?: Record<string, string>;
	models?: Record<string, string>;
}) {
	const base = mkdtempSync(join(tmpdir(), "qo-api-"));
	const repoPath = join(base, "repo");
	const workspace = join(base, "ws");
	// definition fixture
	const defDir = join(workspace, "platform", "tasks", "greet");
	mkdirSync(defDir, { recursive: true });
	writeFileSync(
		join(defDir, "config.yaml"),
		'description: Greet someone by name.\nargs: [name]\ndedup: none\ncron: "*/15 * * * *"\n',
	);
	writeFileSync(join(defDir, "prompt.md"), "Say hi to {{name}}.\n");

	const store = new QueueStore(join(base, "state"));
	const runStore = new RunStore(join(base, "runs"));
	const registry = new SessionRegistry(join(base, "sessions.json"));
	const config: GlobalConfig = {
		workspace,
		projects: [{ name: "platform", path: repoPath }],
		maxConcurrentTasks: 3,
		archiveAfterDays: 7,
		vars: opts?.vars ?? {},
		models: opts?.models ?? {},
	};
	const okResult: RunResult = {
		exitCode: 0,
		timedOut: false,
		signal: null,
		sessionId: null,
		resultText: "ok",
		stderr: "",
		usage: { costUsd: 0, turns: 1, durationMs: 1 },
	};
	const exec: Exec = async (command, args) => {
		opts?.execCalls?.push({ command, args });
		return { stdout: "", exitCode: opts?.execExitCode ?? 0 };
	};
	const resolverIO: ResolverIO = {
		listWorktrees: async () => opts?.worktrees ?? [],
		prBranch: async () => null,
		spawnWorktree: async (_r, name) => ({
			name,
			path: `/wt/${name}`,
			branch: name,
		}),
		// Real removal implementation so the recording exec sees the actual
		// force-clean → `wt remove` → `git branch -D` command sequence.
		removeWorktree: createResolverIO(exec).removeWorktree,
	};
	const mainSessions = new MainSessionStore(join(base, "main-sessions.json"));
	const engine = new Engine({
		store,
		runStore,
		registry,
		config,
		resolverIO,
		exec,
		executeClaude: opts?.executeClaude ?? (async () => okResult),
		executeVerify: async () => ({
			exitCode: 0,
			timedOut: false,
			signal: null,
			output: "",
		}),
		redact: makeRedactor(new Map()),
		mainSessions,
	});
	let mutations = 0;
	const server = new ApiServer({
		engine,
		store,
		runStore,
		registry,
		config,
		mainSessions,
		onMutation: () => {
			mutations += 1;
		},
	});
	const sock = join(base, "d.sock");
	await server.listen(sock);
	const client = new ApiClient();
	await client.connect(sock);
	cleanups.push(() => client.close());
	cleanups.push(() => server.close());
	return {
		server,
		client,
		store,
		engine,
		mainSessions,
		workspace,
		repoPath,
		mutations: () => mutations,
	};
}

describe("ApiServer", () => {
	it("ping/pong", async () => {
		const { client } = await setup();
		expect(await client.call("ping")).toBe("pong");
	});

	it("state snapshot exposes projects and worktrees", async () => {
		const { client, engine } = await setup();
		await engine.tick();
		const state = (await client.call("state")) as {
			projects: { name: string }[];
			worktrees: Record<string, unknown[]>;
			maxConcurrent: number;
		};
		expect(state.projects).toEqual([{ name: "platform" }]);
		expect(state.worktrees).toEqual(engine.worktreesByRepo());
		expect(state.maxConcurrent).toBe(3);
	});

	it("omits githubId when the project has no vars.yaml setting", async () => {
		const { client } = await setup();
		const state = (await client.call("state")) as {
			projects: { name: string; githubId?: string }[];
		};
		// Additive/optional: absent setting → undefined → dropped from the JSON
		// frame, so old TUIs see the same shape they always did.
		expect(state.projects).toEqual([{ name: "platform" }]);
	});

	it("exposes githubId from the project's vars.yaml github_id in the snapshot", async () => {
		const { client, workspace } = await setup();
		writeFileSync(
			join(workspace, "platform", "vars.yaml"),
			"github_id: noootown\n",
		);
		const state = (await client.call("state")) as {
			projects: { name: string; githubId?: string }[];
		};
		expect(state.projects).toEqual([
			{ name: "platform", githubId: "noootown" },
		]);
	});

	it("enqueue creates an adhoc task and reports state", async () => {
		const { client } = await setup();
		const task = (await client.call("enqueue", {
			prompt: "fix it",
			repo: "platform",
		})) as { id: string; target: { ref: string } };
		expect(task.target.ref).toBe("temp");
		const state = (await client.call("state")) as { tasks: { id: string }[] };
		expect(state.tasks.map((t) => t.id)).toContain(task.id);
	});

	it("enqueue_chain creates linked tasks sharing one target", async () => {
		const { client } = await setup();
		const created = (await client.call("enqueue_chain", {
			repo: "platform",
			ref: "temp",
			priority: "high",
			steps: [
				{ definition: "greet", args: ["Ada"] },
				{ prompt: "then celebrate\n" },
			],
		})) as {
			id: string;
			chainId: string;
			chainSeq: number;
			definition: string | null;
			prompt: string;
			priority: string;
			target: { repo: string; ref: string; worktree: string | null };
		}[];
		expect(created).toHaveLength(2);
		const [head, tail] = created;
		// Linked: shared chainId, ascending seq, one head.
		expect(head?.chainId).toBeTruthy();
		expect(head?.chainId).toBe(tail?.chainId);
		expect(head?.chainSeq).toBe(0);
		expect(tail?.chainSeq).toBe(1);
		// Definition step rendered its prompt; prompt step passed through verbatim.
		expect(head?.definition).toBe("platform/greet");
		expect(head?.prompt).toBe("Say hi to Ada.\n");
		expect(tail?.definition).toBeNull();
		expect(tail?.prompt).toBe("then celebrate\n");
		// Shared unresolved target + chain priority applied to both.
		expect(head?.target).toEqual({
			repo: "platform",
			ref: "temp",
			worktree: null,
		});
		expect(tail?.target).toEqual({
			repo: "platform",
			ref: "temp",
			worktree: null,
		});
		expect(head?.priority).toBe("high");
		expect(tail?.priority).toBe("high");
	});

	it("enqueue carries a verify command onto the task", async () => {
		const { client } = await setup();
		const task = (await client.call("enqueue", {
			prompt: "fix it",
			repo: "platform",
			verify: "test -f dist/cli.js",
		})) as { verify: string | null };
		expect(task.verify).toBe("test -f dist/cli.js");
	});

	it("enqueue_chain threads per-step verify onto each member", async () => {
		const { client } = await setup();
		const created = (await client.call("enqueue_chain", {
			repo: "platform",
			ref: "temp",
			steps: [
				{ prompt: "build\n", verify: "test -f dist/cli.js" },
				{ prompt: "no check\n" },
			],
		})) as { verify: string | null }[];
		expect(created[0]?.verify).toBe("test -f dist/cli.js");
		expect(created[1]?.verify).toBeNull();
	});

	it("enqueue_chain rejects a step lacking both definition and prompt", async () => {
		const { client } = await setup();
		await expect(
			client.call("enqueue_chain", { repo: "platform", steps: [{}] }),
		).rejects.toThrow(/must have either/);
	});

	it("enqueue_chain rejects an empty steps list", async () => {
		const { client } = await setup();
		await expect(
			client.call("enqueue_chain", { repo: "platform", steps: [] }),
		).rejects.toThrow(/at least one step/);
	});

	// A `worktree: auto` def extracts the first PR/ticket URL from its arg values
	// and targets that branch. `ref` must be able to override that when the URL is
	// reference material, not the destination.
	function writeAutoDef(workspace: string): void {
		const dir = join(workspace, "platform", "tasks", "autofix");
		mkdirSync(dir, { recursive: true });
		writeFileSync(
			join(dir, "config.yaml"),
			"args: [situation]\ndedup: none\nworktree: auto\n",
		);
		writeFileSync(join(dir, "prompt.md"), "Fix: {{situation}}\n");
	}

	it("run_task_definition: ref overrides a worktree:auto def's URL extraction", async () => {
		const { client, workspace } = await setup();
		writeAutoDef(workspace);
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "autofix",
			args: ["see https://github.com/o/r/pull/42 for context"],
			ref: "temp",
		})) as { target: { ref: string } }[];
		expect(created[0]?.target.ref).toBe("temp");
	});

	it("run_task_definition: without ref, a worktree:auto def extracts the URL target", async () => {
		const { client, workspace } = await setup();
		writeAutoDef(workspace);
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "autofix",
			args: ["see https://github.com/o/r/pull/42 for context"],
		})) as { target: { ref: string } }[];
		expect(created[0]?.target.ref).toBe("pr:42");
	});

	it("run_task_definition: worktree param beats ref (precedence cwd > worktree > ref)", async () => {
		const { client, workspace } = await setup();
		writeAutoDef(workspace);
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "autofix",
			args: ["anything"],
			worktree: "feat-a",
			ref: "temp",
		})) as { target: { ref: string } }[];
		expect(created[0]?.target.ref).toBe("worktree:feat-a");
	});

	it("enqueue_chain: a worktree:auto definition step inherits the chain ref, not its URL", async () => {
		const { client, workspace } = await setup();
		writeAutoDef(workspace);
		const created = (await client.call("enqueue_chain", {
			repo: "platform",
			ref: "temp",
			steps: [
				{
					definition: "autofix",
					args: ["see https://github.com/o/r/pull/42 for context"],
				},
				{ prompt: "then pr-ready\n" },
			],
		})) as { target: { ref: string } }[];
		// Both members land on the chain's shared target, not pr:42.
		expect(created.map((t) => t.target.ref)).toEqual(["temp", "temp"]);
	});

	it("state snapshot exposes mainSessions (empty by default, filled after set)", async () => {
		const { client, mainSessions } = await setup();
		const empty = (await client.call("state")) as {
			mainSessions: Record<string, string>;
		};
		expect(empty.mainSessions).toEqual({});
		mainSessions.set("platform:JUS-1", "sess-1");
		const filled = (await client.call("state")) as {
			mainSessions: Record<string, string>;
		};
		expect(filled.mainSessions).toEqual({ "platform:JUS-1": "sess-1" });
	});

	it("state snapshot carries a string buildId", async () => {
		const { client } = await setup();
		const state = (await client.call("state")) as { buildId: unknown };
		// Under vitest the daemon resolves to TS source (no dist/*.js), so buildId
		// is "0" — but it must always be a string so the TUI can compare it.
		expect(typeof state.buildId).toBe("string");
	});

	it("shutdown refuses while a task is running, then succeeds when idle", async () => {
		let release: () => void = () => {};
		const parked = new Promise<void>((r) => {
			release = r;
		});
		const okResult: RunResult = {
			exitCode: 0,
			timedOut: false,
			signal: null,
			sessionId: null,
			resultText: "ok",
			stderr: "",
			usage: { costUsd: 0, turns: 1, durationMs: 1 },
		};
		const { client, store, engine } = await setup({
			worktrees: [{ name: "JUS-1", path: "/wt/JUS-1", branch: "JUS-1" }],
			executeClaude: async () => {
				await parked;
				return okResult;
			},
		});
		// Start a worker that parks in executeClaude (resolve tick, then start tick).
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		await engine.tick(); // resolve
		await engine.tick(); // start → executeClaude parks on `parked`
		expect(engine.runningTaskIds()).toHaveLength(1);
		await expect(client.call("shutdown")).rejects.toThrow(/busy/);
		// Let the worker finish; now idle → shutdown is accepted.
		release();
		await engine.drain();
		expect(engine.runningTaskIds()).toEqual([]);
		expect(await client.call("shutdown")).toBe(true);
	});

	it("enqueue with worktree sets the target ref", async () => {
		const { client } = await setup();
		const task = (await client.call("enqueue", {
			prompt: "fix it",
			repo: "platform",
			worktree: "wt-a",
		})) as { target: { ref: string } };
		expect(task.target.ref).toBe("worktree:wt-a");
	});

	it("enqueue with session main sets the task session field", async () => {
		const { client } = await setup();
		const task = (await client.call("enqueue", {
			prompt: "fix it",
			repo: "platform",
			session: "main",
		})) as { session: string };
		expect(task.session).toBe("main");
	});

	it("enqueue defaults session to fresh", async () => {
		const { client } = await setup();
		const task = (await client.call("enqueue", {
			prompt: "fix it",
			repo: "platform",
		})) as { session: string };
		expect(task.session).toBe("fresh");
	});

	it("definitions lists per-repo task definitions with scope + ArgSpec args", async () => {
		const { client } = await setup();
		const defs = (await client.call("definitions")) as {
			repo: string;
			name: string;
			scope: string;
			args: { name: string }[];
			hasDiscovery: boolean;
			cron: string | null;
			description: string | null;
			model: string;
		}[];
		expect(defs).toEqual([
			{
				repo: "platform",
				name: "greet",
				scope: "project",
				args: [{ name: "name" }],
				hasDiscovery: false,
				cron: "*/15 * * * *",
				description: "Greet someone by name.",
				// summary carries the RESOLVED id (built-in default sonnet alias).
				model: "claude-sonnet-5",
			},
		]);
	});

	it("definitions resolves the model alias against the per-project table", async () => {
		const { client, workspace } = await setup();
		// Project-local override: sonnet → a custom id via vars.yaml models block.
		writeFileSync(
			join(workspace, "platform", "vars.yaml"),
			"models:\n  sonnet: my-custom-sonnet\n",
		);
		const defs = (await client.call("definitions")) as { model: string }[];
		expect(defs[0]?.model).toBe("my-custom-sonnet");
	});

	describe("settings", () => {
		it("returns defaults, an empty global, and no projects when nothing is overridden", async () => {
			const { client } = await setup();
			const settings = (await client.call("settings")) as {
				models: {
					defaults: Record<string, string>;
					global: { entries: Record<string, string>; source: string };
					projects: unknown[];
				};
			};
			expect(settings.models.defaults).toEqual(DEFAULT_MODEL_ALIASES);
			expect(settings.models.global).toEqual({
				entries: {},
				source: configPath(),
			});
			expect(settings.models.projects).toEqual([]);
		});

		it("carries the global override and only overriding project blocks", async () => {
			const { client, workspace } = await setup({
				models: { sonnet: "claude-sonnet-global" },
			});
			writeFileSync(
				join(workspace, "platform", "vars.yaml"),
				"models:\n  opus: claude-opus-project\n",
			);
			const settings = (await client.call("settings")) as {
				models: {
					defaults: Record<string, string>;
					global: { entries: Record<string, string>; source: string };
					projects: {
						repo: string;
						entries: Record<string, string>;
						source: string;
					}[];
				};
			};
			expect(settings.models.defaults).toEqual(DEFAULT_MODEL_ALIASES);
			expect(settings.models.global.entries).toEqual({
				sonnet: "claude-sonnet-global",
			});
			expect(settings.models.projects).toEqual([
				{
					repo: "platform",
					entries: { opus: "claude-opus-project" },
					source: join(workspace, "platform", "vars.yaml"),
				},
			]);
		});
	});

	it("definitions merges global defs and lets a project-local name shadow them", async () => {
		const { client, workspace } = await setup();
		// A global def unique to the workspace, plus one that shares the local name.
		const globalOnly = join(workspace, "global", "tasks", "squash-merge");
		mkdirSync(globalOnly, { recursive: true });
		writeFileSync(join(globalOnly, "config.yaml"), "worktree: repo\n");
		writeFileSync(join(globalOnly, "prompt.md"), "squash\n");
		const globalGreet = join(workspace, "global", "tasks", "greet");
		mkdirSync(globalGreet, { recursive: true });
		writeFileSync(join(globalGreet, "config.yaml"), "args: [shadowed]\n");
		writeFileSync(join(globalGreet, "prompt.md"), "global greet\n");

		const defs = (await client.call("definitions")) as {
			name: string;
			scope: string;
			args: { name: string }[];
		}[];
		const byName = Object.fromEntries(defs.map((d) => [d.name, d]));
		// project-local greet shadows the global one (scope stays "project").
		expect(byName.greet).toMatchObject({
			scope: "project",
			args: [{ name: "name" }],
		});
		// squash-merge exists only globally.
		expect(byName["squash-merge"]).toMatchObject({ scope: "global" });
	});

	it("runDefinition resolves a global definition (project has none by that name)", async () => {
		const { client, workspace } = await setup();
		const dir = join(workspace, "global", "tasks", "wave");
		mkdirSync(dir, { recursive: true });
		writeFileSync(join(dir, "config.yaml"), "args: [who]\ndedup: none\n");
		writeFileSync(join(dir, "prompt.md"), "wave at {{who}}\n");
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "wave",
			args: ["mars"],
		})) as { prompt: string; definition: string }[];
		expect(created[0]?.prompt).toBe("wave at mars\n");
		expect(created[0]?.definition).toBe("platform/wave");
	});

	it("runDefinition injects builtin project/repo_path vars that explicit config vars override", async () => {
		const { client, workspace, repoPath } = await setup({
			vars: { project: "OVERRIDDEN" },
		});
		const dir = join(workspace, "platform", "tasks", "builtins");
		mkdirSync(dir, { recursive: true });
		writeFileSync(join(dir, "config.yaml"), "args: [x]\ndedup: none\n");
		writeFileSync(
			join(dir, "prompt.md"),
			"project={{project}} path={{repo_path}} x={{x}}\n",
		);
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "builtins",
			args: ["y"],
		})) as { prompt: string }[];
		// {{project}} resolves to the explicit config var (override wins);
		// {{repo_path}} falls through to the builtin (the project's code path).
		expect(created[0]?.prompt).toBe(
			`project=OVERRIDDEN path=${repoPath} x=y\n`,
		);
	});

	it("runDefinition with args instantiates", async () => {
		const { client } = await setup();
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "greet",
			args: ["world"],
		})) as { prompt: string }[];
		expect(created).toHaveLength(1);
		expect(created[0]?.prompt).toBe("Say hi to world.\n");
	});

	it("runDefinition attributes source: mcp when requested", async () => {
		const { client } = await setup();
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "greet",
			args: ["world"],
			source: "mcp",
		})) as { source: string }[];
		expect(created).toHaveLength(1);
		expect(created[0]?.source).toBe("mcp");
	});

	it("runDefinition defaults source to tui", async () => {
		const { client } = await setup();
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "greet",
			args: ["world"],
		})) as { source: string }[];
		expect(created[0]?.source).toBe("tui");
	});

	it("runDefinition with worktree overrides the task ref", async () => {
		const { client } = await setup();
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "greet",
			args: ["world"],
			worktree: "wt-x",
		})) as { target: { ref: string } }[];
		expect(created).toHaveLength(1);
		expect(created[0]?.target.ref).toBe("worktree:wt-x");
	});

	it("runDefinition ignores the worktree override for a `worktree: repo` def", async () => {
		const { client, workspace } = await setup();
		// A def pinned to the primary checkout: the picker passed its worktree only
		// as arg context, so the run must stay `repo`, not be pinned to the worktree.
		const dir = join(workspace, "platform", "tasks", "pinned");
		mkdirSync(dir, { recursive: true });
		writeFileSync(
			join(dir, "config.yaml"),
			"args: [source]\ndedup: none\nworktree: repo\n",
		);
		writeFileSync(join(dir, "prompt.md"), "squash {{source}}\n");
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "pinned",
			args: ["feat-x"],
			worktree: "platform.feat-x",
		})) as { target: { ref: string } }[];
		expect(created).toHaveLength(1);
		expect(created[0]?.target.ref).toBe("repo");
	});

	it("definition returns the full loaded definition", async () => {
		const { client } = await setup();
		const def = (await client.call("definition", {
			repo: "platform",
			name: "greet",
		})) as {
			prompt: string;
			args: { name: string }[];
			worktree: string;
			model: string;
		};
		expect(def.prompt).toBe("Say hi to {{name}}.\n");
		expect(def.args).toEqual([{ name: "name" }]);
		expect(def.worktree).toBe("temp");
		expect(def.model).toBe("sonnet");
	});

	it("definition resolves the model alias into modelResolved, preserving the authored model", async () => {
		const { client, workspace } = await setup();
		// A def authored with the `opus` alias resolves to its built-in id, while
		// the authored `model` field stays "opus".
		const opusDir = join(workspace, "platform", "tasks", "opusdef");
		mkdirSync(opusDir, { recursive: true });
		writeFileSync(join(opusDir, "config.yaml"), "model: opus\n");
		writeFileSync(join(opusDir, "prompt.md"), "hi\n");
		const opus = (await client.call("definition", {
			repo: "platform",
			name: "opusdef",
		})) as { model: string; modelResolved: string };
		expect(opus.model).toBe("opus");
		expect(opus.modelResolved).toBe("claude-opus-4-8");

		// The greet fixture defaults to the `sonnet` alias.
		const greet = (await client.call("definition", {
			repo: "platform",
			name: "greet",
		})) as { model: string; modelResolved: string };
		expect(greet.model).toBe("sonnet");
		expect(greet.modelResolved).toBe("claude-sonnet-5");

		// A def already naming a full/unknown model id passes through unchanged.
		const fullDir = join(workspace, "platform", "tasks", "fulldef");
		mkdirSync(fullDir, { recursive: true });
		writeFileSync(join(fullDir, "config.yaml"), "model: claude-custom-9\n");
		writeFileSync(join(fullDir, "prompt.md"), "hi\n");
		const full = (await client.call("definition", {
			repo: "platform",
			name: "fulldef",
		})) as { model: string; modelResolved: string };
		expect(full.model).toBe("claude-custom-9");
		expect(full.modelResolved).toBe("claude-custom-9");
	});

	it("definition rejects unknown repo", async () => {
		const { client } = await setup();
		await expect(
			client.call("definition", { repo: "nope", name: "greet" }),
		).rejects.toThrow(/unknown repo: nope/);
	});

	it("definition rejects unknown name in a known repo", async () => {
		const { client } = await setup();
		await expect(
			client.call("definition", { repo: "platform", name: "nope" }),
		).rejects.toThrow();
	});

	it("retry re-queues a failed task; skip archives it", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, { status: "failed", error: "boom" });
		const retried = (await client.call("retry", { id: t.id })) as {
			status: string;
			error: null;
		};
		expect(retried.status).toBe("queued");
		store.update(t.id, { status: "failed", error: "boom again" });
		await client.call("skip", { id: t.id });
		expect(store.list()).toEqual([]);
	});

	it("retry re-queues a verify-failed task; skip archives it (parity with failed)", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, {
			status: "verify-failed",
			error: "verify failed (exit 2)",
		});
		const retried = (await client.call("retry", { id: t.id })) as {
			status: string;
		};
		expect(retried.status).toBe("queued");
		store.update(t.id, { status: "verify-failed", error: "again" });
		await client.call("skip", { id: t.id });
		expect(store.list()).toEqual([]);
	});

	it("skip CANCELS a queued task (status cancelled, stays visible), not failed/archived", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		const updated = (await client.call("skip", { id: t.id })) as {
			status: string;
			error: string | null;
			finishedAt: string | null;
		};
		expect(updated.status).toBe("cancelled");
		expect(updated.error).toBe("cancelled by user");
		expect(updated.finishedAt).toMatch(/^\d{4}-\d{2}-\d{2}T.*Z$/);
		// Not archived — it remains in the live list as a cancelled row.
		expect(store.list().map((x) => x.id)).toEqual([t.id]);
	});

	it("skip cancels a needs-input task too", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:gone",
			source: "tui",
		});
		store.update(t.id, { status: "needs-input", error: "not found" });
		const updated = (await client.call("skip", { id: t.id })) as {
			status: string;
		};
		expect(updated.status).toBe("cancelled");
		expect(store.get(t.id)?.status).toBe("cancelled");
	});

	it("skip archives an already-cancelled task (dismiss its terminal row)", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, { status: "cancelled", error: "cancelled by user" });
		await client.call("skip", { id: t.id });
		expect(store.list()).toEqual([]);
	});

	it("retry rejects tasks that are not failed/needs-input", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		await expect(client.call("retry", { id: t.id })).rejects.toThrow(
			/cannot retry/,
		);
	});

	it("stop rejects tasks that are not running", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, { status: "failed", error: "boom" });
		await expect(client.call("stop", { id: t.id })).rejects.toThrow(
			/cannot stop task in status failed/,
		);
	});

	it("stop on a running task with no tracked child surfaces the engine error", async () => {
		// A task marked running but whose child was never tracked (e.g. it started
		// under a previous daemon) has no pid — stopTask throws, and the RPC relays
		// that message rather than silently succeeding.
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, { status: "running", error: null });
		await expect(client.call("stop", { id: t.id })).rejects.toThrow(
			/no running child tracked/,
		);
	});

	it("setWorktree answers needs-input and re-queues", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:gone",
			source: "tui",
		});
		store.update(t.id, { status: "needs-input", error: "not found" });
		const updated = (await client.call("setWorktree", {
			id: t.id,
			worktree: "main",
		})) as { status: string; target: { worktree: string } };
		expect(updated.status).toBe("queued");
		expect(updated.target.worktree).toBe("main");
	});

	it("setWorktree rejects tasks that are not needs-input", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		await expect(
			client.call("setWorktree", { id: t.id, worktree: "main" }),
		).rejects.toThrow(/cannot set worktree/);
	});

	it("unknown method errors", async () => {
		const { client } = await setup();
		await expect(client.call("nope")).rejects.toThrow("unknown method: nope");
	});

	it("subscribe pushes state on broadcast", async () => {
		const { server, client } = await setup();
		const states: unknown[] = [];
		await client.subscribe((s) => states.push(s));
		server.broadcast();
		await new Promise((r) => setTimeout(r, 100));
		expect(states.length).toBeGreaterThanOrEqual(1);
	});

	it("onClose fires when the server destroys the connection", async () => {
		const { server, client } = await setup();
		const closed = new Promise<void>((resolve) => client.onClose(resolve));
		await server.close();
		await closed;
		expect(true).toBe(true);
	});

	describe("removeWorktree", () => {
		const WT = [
			{ name: "platform.fix-x", path: "/wt/platform.fix-x", branch: "fix-x" },
		];

		it("force-cleans and removes an idle worktree via wt remove and reports mutation", async () => {
			const execCalls: { command: string; args: string[] }[] = [];
			const { client, mutations } = await setup({ worktrees: WT, execCalls });
			const before = mutations();
			expect(
				await client.call("removeWorktree", {
					repo: "platform",
					name: "fix-x",
				}),
			).toBe(true);
			expect(execCalls).toContainEqual({
				command: "wt",
				args: ["remove", "fix-x", "--yes"],
			});
			expect(mutations()).toBe(before + 1);
		});

		it("matches the full worktree name too", async () => {
			const execCalls: { command: string; args: string[] }[] = [];
			const { client } = await setup({ worktrees: WT, execCalls });
			await client.call("removeWorktree", {
				repo: "platform",
				name: "platform.fix-x",
			});
			expect(execCalls.some((c) => c.command === "wt")).toBe(true);
		});

		it("rejects when a task is running on the worktree's lane", async () => {
			const { client, store } = await setup({ worktrees: WT });
			const task = store.create({
				prompt: "p",
				repo: "platform",
				ref: "worktree:platform.fix-x",
				source: "mcp",
				priority: "normal",
				session: "fresh",
			});
			store.update(task.id, {
				status: "running",
				target: { ...task.target, worktree: "platform.fix-x" },
			});
			await expect(
				client.call("removeWorktree", { repo: "platform", name: "fix-x" }),
			).rejects.toThrow(/busy/);
		});

		it("rejects unknown worktree and unknown repo", async () => {
			const { client } = await setup({ worktrees: WT });
			await expect(
				client.call("removeWorktree", { repo: "platform", name: "nope" }),
			).rejects.toThrow(/not found/);
			await expect(
				client.call("removeWorktree", { repo: "ghost", name: "fix-x" }),
			).rejects.toThrow(/unknown repo/);
		});

		it("surfaces wt remove failure as an error", async () => {
			const { client } = await setup({ worktrees: WT, execExitCode: 128 });
			await expect(
				client.call("removeWorktree", { repo: "platform", name: "fix-x" }),
			).rejects.toThrow(/failed to remove worktree/);
		});
	});

	describe("createWorktree", () => {
		it("delegates to the engine, returns the path, and reports mutation", async () => {
			const { client, mutations } = await setup();
			const before = mutations();
			// The reply carries the created worktree's path so the TUI can open
			// a tmux window there.
			expect(
				await client.call("createWorktree", {
					repo: "platform",
					name: "feature-x",
				}),
			).toEqual({ path: "/wt/feature-x" });
			expect(mutations()).toBe(before + 1);
		});

		it("rejects an existing branch and an unknown repo", async () => {
			const { client } = await setup({
				worktrees: [
					{ name: "platform.feature-x", path: "/wt/x", branch: "feature-x" },
				],
			});
			await expect(
				client.call("createWorktree", { repo: "platform", name: "feature-x" }),
			).rejects.toThrow(/already exists/);
			await expect(
				client.call("createWorktree", { repo: "ghost", name: "feature-x" }),
			).rejects.toThrow(/unknown repo/);
		});
	});

	describe("enqueue with cwd / resume_session_id / model", () => {
		const WT = [
			{ name: "repo", path: "/wt/repo", branch: "main" },
			{ name: "repo.fix-x", path: "/wt/repo.fix-x", branch: "fix-x" },
		];

		it("resolves cwd to repo + worktree and stamps resume/model", async () => {
			const { client } = await setup({ worktrees: WT });
			const task = (await client.call("enqueue", {
				prompt: "continue",
				cwd: "/wt/repo.fix-x/src/deep",
				resume_session_id: "sess-1",
				model: "claude-fable-5",
			})) as {
				target: { repo: string; ref: string };
				resumeSessionId: string;
				model: string;
			};
			expect(task.target.repo).toBe("platform");
			expect(task.target.ref).toBe("worktree:repo.fix-x");
			expect(task.resumeSessionId).toBe("sess-1");
			expect(task.model).toBe("claude-fable-5");
		});

		it("prefers the longest matching worktree path", async () => {
			// /wt/repo is a prefix of /wt/repo.fix-x only path-segment-wise;
			// use a genuinely nested pair to prove longest-match.
			const nested = [
				{ name: "outer", path: "/wt/outer", branch: "main" },
				{ name: "inner", path: "/wt/outer/inner", branch: "b" },
			];
			const { client } = await setup({ worktrees: nested });
			const task = (await client.call("enqueue", {
				prompt: "p",
				cwd: "/wt/outer/inner/src",
			})) as { target: { ref: string } };
			expect(task.target.ref).toBe("worktree:inner");
		});

		it("unresolvable cwd fails with config.yaml guidance", async () => {
			const { client } = await setup({ worktrees: WT });
			await expect(
				client.call("enqueue", { prompt: "p", cwd: "/elsewhere/repo" }),
			).rejects.toThrow(/config\.yaml/);
		});

		it("enqueue without repo and without cwd is rejected", async () => {
			const { client } = await setup();
			await expect(client.call("enqueue", { prompt: "p" })).rejects.toThrow(
				/repo or cwd/,
			);
		});
	});

	describe("runDefinition with cwd / resume_session_id", () => {
		const WT = [
			{ name: "repo.fix-x", path: "/wt/repo.fix-x", branch: "fix-x" },
		];

		it("targets the resolved worktree and stamps resumeSessionId", async () => {
			const { client } = await setup({ worktrees: WT });
			const created = (await client.call("runDefinition", {
				repo: "platform",
				name: "greet",
				args: ["world"],
				cwd: "/wt/repo.fix-x",
				resume_session_id: "sess-2",
			})) as { target: { ref: string }; resumeSessionId: string }[];
			expect(created).toHaveLength(1);
			expect(created[0]?.target.ref).toBe("worktree:repo.fix-x");
			expect(created[0]?.resumeSessionId).toBe("sess-2");
		});

		it("cwd resolving to a different repo is rejected", async () => {
			// setup registers only "platform"; resolveCwd maps any listed worktree
			// to it, so simulate mismatch by passing an unknown repo name.
			const { client } = await setup({ worktrees: WT });
			await expect(
				client.call("runDefinition", {
					repo: "ghost",
					name: "greet",
					args: ["world"],
					cwd: "/wt/repo.fix-x",
				}),
			).rejects.toThrow(/unknown repo/);
		});
	});
});
