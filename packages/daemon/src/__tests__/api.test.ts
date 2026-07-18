import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	utimesSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type {
	Exec,
	GlobalConfig,
	ProviderUsage,
	ResolverIO,
	RunResult,
} from "@queohoh/core";
import {
	BUILTIN_CATALOG,
	createResolverIO,
	DEFAULT_PROVIDERS,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionLineageStore,
	SessionRegistry,
} from "@queohoh/core";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiServer } from "../api.js";
import { ApiClient } from "../client.js";
import { Engine } from "../engine.js";
import { settingsPath } from "../paths.js";
import { SettingsStore } from "../settings-store.js";

const cleanups: (() => Promise<void> | void)[] = [];
afterEach(async () => {
	while (cleanups.length) await cleanups.pop()?.();
});

const okRunResult: RunResult = {
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

/** Writes a Claude Code on-disk transcript file with an explicit mtime, so
 * `listSessions`'s recency ordering (Source A) is deterministic in tests.
 * `mtimeMs` must be a multiple of 1000 — `utimesSync` takes whole-second Unix
 * timestamps, and every filesystem this runs on preserves 1s resolution
 * exactly (sub-second precision is not guaranteed everywhere). */
function writeClaudeSessionFile(
	dir: string,
	sessionId: string,
	mtimeMs: number,
	lines: unknown[] = [{ type: "user", message: { content: "hi" } }],
): void {
	const path = join(dir, `${sessionId}.jsonl`);
	writeFileSync(path, lines.map((l) => JSON.stringify(l)).join("\n"));
	utimesSync(path, mtimeMs / 1000, mtimeMs / 1000);
}

/** Patches a run's persisted `started_at`/`finished_at` timestamps directly
 * on disk. RunStore's public API always stamps real wall-clock time, so this
 * is the only way to make `listSessions`'s recency ordering (Source B)
 * deterministic in tests. */
function patchRunTimestamps(
	runStore: RunStore,
	taskId: string,
	overrides: { started_at?: string; finished_at?: string },
): void {
	const dataPath = join(runStore.runDir(taskId), "data.json");
	const existing = JSON.parse(readFileSync(dataPath, "utf-8"));
	writeFileSync(dataPath, JSON.stringify({ ...existing, ...overrides }));
}

async function setup(opts?: {
	worktrees?: { name: string; path: string; branch: string }[];
	execCalls?: { command: string; args: string[] }[];
	execExitCode?: number;
	/** Full exec override (wins over execCalls/execExitCode). A test injects one
	 * to route git/gh subcommands and drive worktree enrichment through the
	 * engine into the snapshot. */
	exec?: Exec;
	executeClaude?: () => Promise<RunResult>;
	vars?: Record<string, string>;
	claudeProjectsDir?: string;
	/** Override config.providers (e.g. seed a `bin` for settings RPC tests). */
	providers?: GlobalConfig["providers"];
	/** Pre-seed `<state>/daemon/settings.json` with this active_provider before
	 * the SettingsStore is constructed — exercises the config-load snap path. */
	activeProviderSeed?: string;
	/** Optional usage poller injected into ApiServer (provider-usage-header). */
	usagePoller?: {
		snapshot: () => ProviderUsage[];
	};
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
		catalog: BUILTIN_CATALOG,
		defaultModels: ["claude/opus", "grok/grok-4.5"],
		providers: opts?.providers ?? DEFAULT_PROVIDERS,
	};
	const stateDir = join(base, "state");
	if (opts?.activeProviderSeed !== undefined) {
		const sp = settingsPath(stateDir);
		mkdirSync(dirname(sp), { recursive: true });
		writeFileSync(
			sp,
			JSON.stringify({ active_provider: opts.activeProviderSeed }),
		);
	}
	const settings = new SettingsStore(stateDir, config.providers);
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
	const exec: Exec =
		opts?.exec ??
		(async (command, args) => {
			opts?.execCalls?.push({ command, args });
			return { stdout: "", exitCode: opts?.execExitCode ?? 0 };
		});
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
	const lineage = new SessionLineageStore(join(base, "session-lineage.json"));
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
		lineage,
	});
	let mutations = 0;
	const server = new ApiServer({
		engine,
		store,
		runStore,
		registry,
		config,
		settings,
		lineage,
		claudeProjectsDir: opts?.claudeProjectsDir,
		usagePoller: opts?.usagePoller,
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
		runStore,
		engine,
		lineage,
		settings,
		stateDir,
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

	it("carries prAuthor/prState (camelCase) for a merged PR through the snapshot", async () => {
		// Route the merged-PR list so the engine's enrichment stamps prAuthor —
		// then assert it survives JSON serialization to the wire as camelCase.
		const exec: Exec = async (command, args) => {
			if (command === "git" && args[2] === "log") {
				return { stdout: "1\tIan Chiu\ti@x\tabc123\n", exitCode: 0 };
			}
			if (command === "gh") {
				const stateIdx = args.indexOf("--state");
				const state = stateIdx >= 0 ? args[stateIdx + 1] : "";
				if (state === "merged") {
					return {
						stdout: JSON.stringify([
							{
								number: 55,
								headRefName: "wt-a",
								url: "https://github.com/o/r/pull/55",
								state: "MERGED",
								author: { name: "Tim Kuminecz", login: "tkuminecz" },
							},
						]),
						exitCode: 0,
					};
				}
				return { stdout: "[]", exitCode: 0 };
			}
			return { stdout: "", exitCode: 0 };
		};
		const { client, engine } = await setup({
			exec,
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		await engine.refreshGitEnrichment();
		const state = (await client.call("state")) as {
			worktrees: Record<string, { prAuthor?: string; prState?: string }[]>;
		};
		expect(state.worktrees.platform?.[0]).toMatchObject({
			prAuthor: "Tim Kuminecz",
			prState: "MERGED",
		});
	});

	it("does not surface gotoCommand on the state snapshot", async () => {
		// First-class TUI goto replaced workspace goto_command / init-tab; the
		// snapshot must not reintroduce the field.
		const { client } = await setup();
		const state = (await client.call("state")) as { gotoCommand?: string };
		expect(state.gotoCommand).toBeUndefined();
		expect("gotoCommand" in state).toBe(false);
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

	// A zero-arg def with a discovery block. Plain run (r) must create exactly ONE
	// task from the static prompt; discover (d) must fan out one task per item the
	// discovery command prints.
	function writeDiscoveryDef(workspace: string): void {
		const dir = join(workspace, "platform", "tasks", "sweep");
		mkdirSync(dir, { recursive: true });
		writeFileSync(
			join(dir, "config.yaml"),
			'discovery:\n  command: echo \'[{"n":"1"},{"n":"2"}]\'\n  item_key: "{{n}}"\ndedup: none\n',
		);
		writeFileSync(join(dir, "prompt.md"), "Static run.\n");
	}

	// The bug's regression fixture: zero args, no discovery — the shape of a plain
	// cron def (slack-react-release-notes / workspace-sanitize).
	function writePlainZeroArgDef(workspace: string): void {
		const dir = join(workspace, "platform", "tasks", "daily");
		mkdirSync(dir, { recursive: true });
		writeFileSync(join(dir, "config.yaml"), "dedup: none\n");
		writeFileSync(join(dir, "prompt.md"), "Do the daily thing.\n");
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
			usage: {
				costUsd: 0,
				turns: 1,
				durationMs: 1,
				inputTokens: null,
				outputTokens: null,
			},
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

	// The adhoc-create dialog (TUI) resolves its target combobox to a canonical
	// ref (`worktree:`/`pr:`/`ticket:`) and sends it as `params.ref` — the same
	// param runDefinition uses — instead of a bare `worktree`. Lock that contract.
	it("enqueue with a canonical pr ref sets the target ref", async () => {
		const { client } = await setup();
		const task = (await client.call("enqueue", {
			prompt: "fix it",
			repo: "platform",
			ref: "pr:42",
		})) as { target: { ref: string } };
		expect(task.target.ref).toBe("pr:42");
	});

	it("enqueue with a ticket ref sets the target ref", async () => {
		const { client } = await setup();
		const task = (await client.call("enqueue", {
			prompt: "fix it",
			repo: "platform",
			ref: "ticket:JUS-1756",
		})) as { target: { ref: string } };
		expect(task.target.ref).toBe("ticket:JUS-1756");
	});

	it("enqueue with resume_session_id pins the session", async () => {
		const { client, store } = await setup();
		await client.call("enqueue", {
			prompt: "continue",
			repo: "platform",
			ref: "worktree:wt-a",
			resume_session_id: "sess-1",
		});
		const task = store.list()[0];
		expect(task?.resumeSessionId).toBe("sess-1");
	});

	it("enqueue with session main is deprecated and stored as fresh", async () => {
		const { client, store } = await setup();
		await client.call("enqueue", {
			prompt: "p",
			repo: "platform",
			session: "main",
		});
		const task = store.list()[0];
		expect(task?.session).toBe("fresh");
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
			cronEnabled: boolean;
			description: string | null;
			model: string | string[] | null;
		}[];
		expect(defs).toEqual([
			{
				repo: "platform",
				name: "greet",
				scope: "project",
				args: [{ name: "name" }],
				hasDiscovery: false,
				cron: "*/15 * * * *",
				// Never toggled → armed (the TUI renders the Cron column bright).
				cronEnabled: true,
				description: "Greet someone by name.",
				// The greet fixture omits `model:`, so it resolves against
				// `default_models` at run time — the summary forwards the authored
				// value, which is `null` (there is no alias table to resolve against).
				model: null,
				// schema default when the def omits `worktree:` — the TUI's
				// worktree-scoped task menu keys off this field.
				worktree: "temp",
			},
		]);
	});

	it("definitions forwards an authored provider/label model ref as-is", async () => {
		const { client, workspace } = await setup();
		// A def authoring an explicit `provider/label` ref is forwarded verbatim —
		// there is no alias table to resolve against anymore (the flat catalog
		// replaced it).
		const dir = join(workspace, "platform", "tasks", "grokdef");
		mkdirSync(dir, { recursive: true });
		writeFileSync(join(dir, "config.yaml"), "model: grok/grok-4.5\n");
		writeFileSync(join(dir, "prompt.md"), "hi\n");
		const defs = (await client.call("definitions")) as {
			name: string;
			model: string | string[] | null;
		}[];
		expect(defs.find((d) => d.name === "grokdef")?.model).toBe("grok/grok-4.5");
	});

	it("set_cron_enabled pauses/resumes a def's cron, reflected in definitions", async () => {
		const { client } = await setup();
		const cronEnabled = async () =>
			(
				(await client.call("definitions")) as {
					name: string;
					cronEnabled: boolean;
				}[]
			).find((d) => d.name === "greet")?.cronEnabled;

		expect(await cronEnabled()).toBe(true);

		// Pause → returns the new ENABLED state (false) and dims in `definitions`.
		const paused = await client.call("set_cron_enabled", {
			repo: "platform",
			name: "greet",
			enabled: false,
		});
		expect(paused).toBe(false);
		expect(await cronEnabled()).toBe(false);

		// Resume → back to armed.
		const resumed = await client.call("set_cron_enabled", {
			repo: "platform",
			name: "greet",
			enabled: true,
		});
		expect(resumed).toBe(true);
		expect(await cronEnabled()).toBe(true);
	});

	it("set_cron_enabled rejects a missing repo/name", async () => {
		const { client } = await setup();
		await expect(
			client.call("set_cron_enabled", { name: "greet", enabled: false }),
		).rejects.toThrow(/repo and name are required/);
	});

	describe("settings", () => {
		it("returns the merged catalog, active_provider, global default_models, and no project overrides", async () => {
			const { client } = await setup();
			const settings = (await client.call("settings")) as {
				catalog: { provider: string; id: string; label: string }[];
				active_provider: string;
				default_models: {
					global: string[];
					projects: unknown[];
				};
				providers: { name: string; enabled: boolean }[];
			};
			// Full merged catalog (built-in, incl. hidden flags the TUI filters).
			expect(settings.catalog).toEqual(BUILTIN_CATALOG);
			// No settings.json seeded → precedence-first enabled provider (claude).
			expect(settings.active_provider).toBe("claude");
			expect(settings.default_models.global).toEqual([
				"claude/opus",
				"grok/grok-4.5",
			]);
			expect(settings.default_models.projects).toEqual([]);
			// Providers carry name/enabled (and optional bin when configured);
			// no per-provider model tiers — models live in the flat catalog.
			expect(settings.providers).toEqual(
				DEFAULT_PROVIDERS.map((p) => ({ name: p.name, enabled: p.enabled })),
			);
		});

		it("settings providers include optional bin", async () => {
			const { client } = await setup({
				providers: DEFAULT_PROVIDERS.map((p) =>
					p.name === "grok" ? { ...p, bin: "/tmp/grok-bin" } : p,
				),
			});
			const s = (await client.call("settings")) as {
				providers: { name: string; enabled: boolean; bin?: string }[];
			};
			const grok = s.providers.find((p) => p.name === "grok");
			expect(grok).toMatchObject({
				name: "grok",
				enabled: true,
				bin: "/tmp/grok-bin",
			});
			const claude = s.providers.find((p) => p.name === "claude");
			expect(claude?.bin).toBeUndefined();
		});

		it("lists a project's non-empty default_models override under projects", async () => {
			const { client, workspace } = await setup();
			writeFileSync(
				join(workspace, "platform", "vars.yaml"),
				"default_models:\n  - grok/grok-4.5\n  - claude/opus\n",
			);
			const settings = (await client.call("settings")) as {
				default_models: {
					global: string[];
					projects: {
						name: string;
						default_models: string[];
						source: string;
					}[];
				};
			};
			expect(settings.default_models.projects).toEqual([
				{
					name: "platform",
					default_models: ["grok/grok-4.5", "claude/opus"],
					source: join(workspace, "platform", "vars.yaml"),
				},
			]);
		});

		it("reflects the persisted active_provider", async () => {
			const { client } = await setup({ activeProviderSeed: "grok" });
			const settings = (await client.call("settings")) as {
				active_provider: string;
			};
			expect(settings.active_provider).toBe("grok");
		});
	});

	describe("providerUsage / providerUsages", () => {
		const claudeSample: ProviderUsage = {
			provider: "claude",
			text: "5h 12%",
			severity: "ok",
			fetchedAt: 1_700_000_000_000,
			stale: false,
		};
		const grokSample: ProviderUsage = {
			provider: "grok",
			text: "42% mo",
			severity: "warn",
			fetchedAt: 1_700_000_000_001,
			stale: false,
		};
		const samples = [claudeSample, grokSample];

		it("includes providerUsages + active providerUsage from the poller", async () => {
			// setup's default active provider is precedence-first enabled
			// (claude in the fixture config). providerUsage is the active entry.
			const usagePoller = {
				snapshot: () => samples,
			};
			const { client } = await setup({ usagePoller });
			const state = (await client.call("state")) as {
				providerUsage?: ProviderUsage | null;
				providerUsages?: ProviderUsage[];
			};
			expect(state.providerUsages).toEqual(samples);
			expect(state.providerUsage).toEqual(claudeSample);
		});

		it("omits usage fields when no poller is injected", async () => {
			const { client } = await setup();
			const state = (await client.call("state")) as {
				providerUsage?: ProviderUsage | null;
				providerUsages?: ProviderUsage[];
			};
			expect(state.providerUsage).toBeUndefined();
			expect(state.providerUsages).toBeUndefined();
		});

		it("set_active_provider re-derives providerUsage from the multi list", async () => {
			// Switch does not kick a poller refresh — it only flips activeProvider
			// and re-picks the single-chip back-compat field from providerUsages.
			const usagePoller = {
				snapshot: () => samples,
			};
			const { client } = await setup({ usagePoller });
			const snapshots: {
				providerUsage?: ProviderUsage | null;
				providerUsages?: ProviderUsage[];
				activeProvider?: string;
			}[] = [];
			await client.subscribe((state) =>
				snapshots.push(
					state as {
						providerUsage?: ProviderUsage | null;
						providerUsages?: ProviderUsage[];
						activeProvider?: string;
					},
				),
			);
			await client.call("set_active_provider", { provider: "grok" });
			await vi.waitFor(() => {
				const last = snapshots.at(-1);
				expect(last?.activeProvider).toBe("grok");
				expect(last?.providerUsages).toEqual(samples);
				expect(last?.providerUsage).toEqual(grokSample);
			});
		});
	});

	describe("set_active_provider", () => {
		it("switches to an enabled provider, persists, and broadcasts to subscribers", async () => {
			const { client, stateDir } = await setup();
			// subscribe delivers the state SNAPSHOT (frame.data), not the raw frame.
			const snapshots: { activeProvider?: string }[] = [];
			await client.subscribe((state) =>
				snapshots.push(state as { activeProvider?: string }),
			);
			const result = await client.call("set_active_provider", {
				provider: "grok",
			});
			expect(result).toBe("grok");
			// Persisted write-through: settings.json now names grok.
			const persisted = JSON.parse(
				readFileSync(settingsPath(stateDir), "utf-8"),
			);
			expect(persisted.active_provider).toBe("grok");
			// The state broadcast carries the new active provider.
			await vi.waitFor(() => {
				expect(snapshots.at(-1)?.activeProvider).toBe("grok");
			});
			// And the settings RPC now reflects it.
			const settings = (await client.call("settings")) as {
				active_provider: string;
			};
			expect(settings.active_provider).toBe("grok");
		});

		it("rejects a disabled provider without persisting", async () => {
			const { client, stateDir } = await setup();
			// codex is disabled in DEFAULT_PROVIDERS.
			await expect(
				client.call("set_active_provider", { provider: "codex" }),
			).rejects.toThrow(/disabled: codex/);
			expect(existsSync(settingsPath(stateDir))).toBe(false);
		});

		it("rejects an unknown provider", async () => {
			const { client } = await setup();
			await expect(
				client.call("set_active_provider", { provider: "nope" }),
			).rejects.toThrow(/unknown provider: nope/);
		});

		it("round-trips: a switch survives a fresh SettingsStore load", async () => {
			const { client, stateDir, server } = await setup();
			await client.call("set_active_provider", { provider: "grok" });
			await server.close();
			// A fresh store (a daemon restart / config load) reads the persisted value.
			const reloaded = new SettingsStore(stateDir, DEFAULT_PROVIDERS);
			expect(reloaded.activeProvider()).toBe("grok");
		});
	});

	describe("active_provider config-load snap", () => {
		it("snaps a persisted disabled provider to precedence-first enabled", async () => {
			// codex is disabled by default → the store snaps to claude on load.
			const { client } = await setup({ activeProviderSeed: "codex" });
			const settings = (await client.call("settings")) as {
				active_provider: string;
			};
			expect(settings.active_provider).toBe("claude");
		});

		it("snaps an unknown persisted provider to precedence-first enabled", async () => {
			const { client } = await setup({ activeProviderSeed: "made-up" });
			const settings = (await client.call("settings")) as {
				active_provider: string;
			};
			expect(settings.active_provider).toBe("claude");
		});
	});

	describe("enqueue model validation", () => {
		it("rejects a bare label with a did-you-mean provider/label suggestion", async () => {
			const { client } = await setup();
			await expect(
				client.call("enqueue", {
					repo: "platform",
					prompt: "p",
					model: "opus",
				}),
			).rejects.toThrow(/unknown model: opus \(did you mean claude\/opus\?\)/);
		});

		it("rejects a well-formed ref that names no catalog entry", async () => {
			const { client } = await setup();
			await expect(
				client.call("enqueue", {
					repo: "platform",
					prompt: "p",
					model: "claude/nope",
				}),
			).rejects.toThrow(/unknown model: claude\/nope/);
		});

		it("accepts a single valid provider/label ref", async () => {
			const { client } = await setup();
			const task = (await client.call("enqueue", {
				repo: "platform",
				prompt: "p",
				model: "claude/opus",
			})) as { model: string | string[] | null };
			expect(task.model).toBe("claude/opus");
		});

		it("accepts an ordered fallback list of refs", async () => {
			const { client } = await setup();
			const task = (await client.call("enqueue", {
				repo: "platform",
				prompt: "p",
				model: ["claude/opus", "grok/grok-4.5"],
			})) as { model: string | string[] | null };
			expect(task.model).toEqual(["claude/opus", "grok/grok-4.5"]);
		});

		it("rejects a list containing one unknown ref", async () => {
			const { client } = await setup();
			await expect(
				client.call("enqueue", {
					repo: "platform",
					prompt: "p",
					model: ["claude/opus", "grok/nope"],
				}),
			).rejects.toThrow(/unknown model: grok\/nope/);
		});

		it("rejects a list with a non-string element instead of silently filtering it", async () => {
			const { client } = await setup();
			await expect(
				client.call("enqueue", {
					repo: "platform",
					prompt: "p",
					model: [123],
				}),
			).rejects.toThrow(/invalid model list entry/);
		});

		it("rejects a list with an empty-string element", async () => {
			const { client } = await setup();
			await expect(
				client.call("enqueue", {
					repo: "platform",
					prompt: "p",
					model: ["claude/opus", ""],
				}),
			).rejects.toThrow(/invalid model list entry/);
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

	it("runDefinition with zero args on a no-discovery def plain-runs (regression: 'has no discovery')", async () => {
		const { client, workspace } = await setup();
		writePlainZeroArgDef(workspace);
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "daily",
			args: [],
		})) as { prompt: string }[];
		expect(created).toHaveLength(1);
		expect(created[0]?.prompt).toBe("Do the daily thing.\n");
	});

	it("runDefinition with zero args on a DISCOVERY def plain-runs (never discovers implicitly)", async () => {
		const { client, workspace } = await setup();
		writeDiscoveryDef(workspace);
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "sweep",
			args: [],
		})) as { prompt: string }[];
		// Discovery would fan out 2 tasks; a plain run creates exactly 1.
		expect(created).toHaveLength(1);
		expect(created[0]?.prompt).toBe("Static run.\n");
	});

	// TUI def-run picker peels the trailing model field and sends a 1-entry
	// exact `provider/label` as params.model. Without this path the pick was a
	// no-op: instantiate left task.model null and worker preferred def.model.
	it("runDefinition with model param stamps task.model (override for def-run picker)", async () => {
		const { client } = await setup();
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "greet",
			args: ["world"],
			model: "claude/opus",
		})) as { model: string | string[] | null }[];
		expect(created).toHaveLength(1);
		expect(created[0]?.model).toBe("claude/opus");
	});

	it("runDefinition without model leaves task.model null so def.model applies at spawn", async () => {
		const { client } = await setup();
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "greet",
			args: ["world"],
		})) as { model: string | string[] | null }[];
		expect(created).toHaveLength(1);
		expect(created[0]?.model).toBeNull();
	});

	it("discoverDefinition runs discovery and fans out one task per item", async () => {
		const { client, workspace } = await setup();
		writeDiscoveryDef(workspace);
		const created = (await client.call("discoverDefinition", {
			repo: "platform",
			name: "sweep",
		})) as { prompt: string; source: string; itemKey: string }[];
		expect(created).toHaveLength(2);
		expect(created.map((t) => t.itemKey).sort()).toEqual(["1", "2"]);
		expect(created[0]?.source).toBe("tui");
	});

	it("discoverDefinition on a def without discovery rejects", async () => {
		const { client, workspace } = await setup();
		writePlainZeroArgDef(workspace);
		await expect(
			client.call("discoverDefinition", { repo: "platform", name: "daily" }),
		).rejects.toThrow(/has no discovery/);
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
			model: string | string[] | null;
		};
		expect(def.prompt).toBe("Say hi to {{name}}.\n");
		expect(def.args).toEqual([{ name: "name" }]);
		expect(def.worktree).toBe("temp");
		// The greet fixture omits `model:`, so the authored value is null (it
		// resolves against `default_models` at run time — no alias table anymore).
		expect(def.model).toBeNull();
	});

	it("definition forwards the authored model ref as-is (no modelResolved)", async () => {
		const { client, workspace } = await setup();
		// A single `provider/label` ref is forwarded verbatim.
		const opusDir = join(workspace, "platform", "tasks", "opusdef");
		mkdirSync(opusDir, { recursive: true });
		writeFileSync(join(opusDir, "config.yaml"), "model: claude/opus\n");
		writeFileSync(join(opusDir, "prompt.md"), "hi\n");
		const opus = (await client.call("definition", {
			repo: "platform",
			name: "opusdef",
		})) as { model: string | string[] | null; modelResolved?: unknown };
		expect(opus.model).toBe("claude/opus");
		// There is no alias resolution anymore — the field is gone.
		expect(opus.modelResolved).toBeUndefined();

		// An ordered fallback list is preserved element-for-element.
		const listDir = join(workspace, "platform", "tasks", "listdef");
		mkdirSync(listDir, { recursive: true });
		writeFileSync(
			join(listDir, "config.yaml"),
			"model:\n  - claude/opus\n  - grok/grok-4.5\n",
		);
		writeFileSync(join(listDir, "prompt.md"), "hi\n");
		const list = (await client.call("definition", {
			repo: "platform",
			name: "listdef",
		})) as { model: string | string[] | null };
		expect(list.model).toEqual(["claude/opus", "grok/grok-4.5"]);
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

	it("retry re-queues a cancelled task (parity with failed)", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, { status: "cancelled", error: "cancelled by user" });
		const retried = (await client.call("retry", { id: t.id })) as {
			status: string;
		};
		expect(retried.status).toBe("queued");
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

	it("archive dismisses a terminal task; unarchive restores it with status intact", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, { status: "failed", error: "boom" });
		await client.call("archive", { id: t.id });
		expect(store.list()).toEqual([]);
		expect(store.listArchived().map((a) => a.id)).toEqual([t.id]);
		await client.call("unarchive", { id: t.id });
		expect(store.listArchived()).toEqual([]);
		expect(store.get(t.id)?.status).toBe("failed");
	});

	it("archive dismisses a needs-input task; unarchive restores it still needs-input", async () => {
		// A needs-input task is parked (never started), so archiving it hides no
		// live work and keeps its status intact — it round-trips exactly like a
		// terminal row (no cancellation).
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, { status: "needs-input", error: "not found" });
		await client.call("archive", { id: t.id });
		expect(store.list()).toEqual([]);
		expect(store.listArchived().map((a) => a.id)).toEqual([t.id]);
		await client.call("unarchive", { id: t.id });
		expect(store.listArchived()).toEqual([]);
		expect(store.get(t.id)?.status).toBe("needs-input");
	});

	it("archive refuses a live task (queued/running)", async () => {
		const { client, store } = await setup();
		for (const status of ["queued", "running"] as const) {
			const t = store.create({
				prompt: "p",
				repo: "platform",
				ref: "temp",
				source: "tui",
			});
			store.update(t.id, { status });
			await expect(client.call("archive", { id: t.id })).rejects.toThrow(
				`cannot archive task in status ${status}`,
			);
		}
	});

	it("unarchive rejects an id that is not in the archive", async () => {
		const { client } = await setup();
		await expect(client.call("unarchive", { id: "nope" })).rejects.toThrow(
			/task not found in archive/,
		);
	});

	it("retry re-queues every non-running status (done/skipped/queued included)", async () => {
		const { client, store } = await setup();
		for (const status of ["done", "skipped", "queued"] as const) {
			const t = store.create({
				prompt: "p",
				repo: "platform",
				ref: "temp",
				source: "tui",
			});
			store.update(t.id, { status, error: null });
			const retried = (await client.call("retry", { id: t.id })) as {
				status: string;
			};
			expect(retried.status).toBe("queued");
		}
	});

	it("retry rejects only a running task (its in-flight worker owns the status)", async () => {
		const { client, store } = await setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(t.id, { status: "running" });
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

	it("listSessions returns labeled sessions for a worktree", async () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		const wtPath = "/wt/platform.wt-a";
		const dir = join(projects, wtPath.replace(/[/.]/g, "-"));
		mkdirSync(dir, { recursive: true });
		writeFileSync(
			join(dir, "sess-titled.jsonl"),
			`${JSON.stringify({ type: "ai-title", aiTitle: "Fix the parser", sessionId: "sess-titled" })}\n`,
		);
		writeFileSync(
			join(dir, "sess-run.jsonl"),
			`${JSON.stringify({ type: "user", message: { content: "ignored" } })}\n`,
		);
		const { client, store, runStore } = await setup({
			claudeProjectsDir: projects,
			worktrees: [{ name: "platform.wt-a", path: wtPath, branch: "wt-a" }],
		});
		// Seed a run whose data.json maps sess-run → its task prompt.
		const task = store.create({
			prompt: "queohoh task prompt\nsecond line",
			repo: "platform",
			ref: "worktree:platform.wt-a",
			source: "mcp",
		});
		runStore.writeSnapshot(
			task.id,
			{
				task,
				definition: null,
				resolvedWorktree: wtPath,
				resolvedWorktreePath: wtPath,
				prompt: task.prompt,
				model: "sonnet",
			},
			(s) => s,
		);
		runStore.finishRun(
			task.id,
			{
				result: { ...okRunResult, sessionId: "sess-run" },
				outcome: "done",
				reason: null,
			},
			(s) => s,
		);

		const res = (await client.call("listSessions", {
			repo: "platform",
			worktree: "platform.wt-a",
		})) as { sessions: { session_id: string; label: string }[] };
		const byId = Object.fromEntries(
			res.sessions.map((s) => [s.session_id, s.label]),
		);
		expect(byId["sess-run"]).toBe("queohoh task prompt"); // run prompt beats jsonl content
		expect(byId["sess-titled"]).toBe("Fix the parser"); // ai-title fallback
		expect(res.sessions.length).toBe(2);
	});

	it("listSessions reports each session's model, mapped back to a provider/label ref", async () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		const wtPath = "/wt/platform.wt-a";
		const dir = join(projects, wtPath.replace(/[/.]/g, "-"));
		mkdirSync(dir, { recursive: true });
		// Two sessions; only sess-opus has a queohoh run (and thus a model) behind it.
		writeFileSync(
			join(dir, "sess-opus.jsonl"),
			`${JSON.stringify({ type: "user", message: { content: "hi" } })}\n`,
		);
		writeFileSync(
			join(dir, "sess-foreign.jsonl"),
			`${JSON.stringify({ type: "ai-title", aiTitle: "outside", sessionId: "sess-foreign" })}\n`,
		);
		const { client, store, runStore } = await setup({
			claudeProjectsDir: projects,
			worktrees: [{ name: "platform.wt-a", path: wtPath, branch: "wt-a" }],
		});
		const task = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:platform.wt-a",
			source: "mcp",
		});
		// Run data persists the RESOLVED id (as the worker does), not the alias.
		runStore.writeSnapshot(
			task.id,
			{
				task,
				definition: null,
				resolvedWorktree: wtPath,
				resolvedWorktreePath: wtPath,
				prompt: task.prompt,
				model: "claude-opus-4-8",
			},
			(s) => s,
		);
		runStore.finishRun(
			task.id,
			{
				result: { ...okRunResult, sessionId: "sess-opus" },
				outcome: "done",
				reason: null,
			},
			(s) => s,
		);
		const res = (await client.call("listSessions", {
			repo: "platform",
			worktree: "platform.wt-a",
		})) as {
			sessions: { session_id: string; model?: string; provider?: string }[];
		};
		const byModel = Object.fromEntries(
			res.sessions.map((s) => [s.session_id, s.model]),
		);
		expect(byModel["sess-opus"]).toBe("claude/opus"); // id maps back to its provider/label ref
		expect(byModel["sess-foreign"]).toBeUndefined(); // no run data -> no model
		// Provider segment of the mapped model ref (claude/opus → claude).
		const byProvider = Object.fromEntries(
			res.sessions.map((s) => [s.session_id, s.provider]),
		);
		expect(byProvider["sess-opus"]).toBe("claude");
		// Every row now carries a provider tag (union spec §5): an on-disk
		// session with no model and no lineage tag still defaults to "claude" —
		// that's the only kind of session Claude Code's transcript dir holds.
		expect(byProvider["sess-foreign"]).toBe("claude");
	});

	it("listSessions includes provider from lineage when model is unknown", async () => {
		// Session has no queohoh run model mapping, but lineage tags the provider
		// (e.g. a grok interactive session recorded at spawn).
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		const wtPath = "/wt/platform.wt-a";
		const dir = join(projects, wtPath.replace(/[/.]/g, "-"));
		mkdirSync(dir, { recursive: true });
		writeFileSync(
			join(dir, "sess-grok.jsonl"),
			`${JSON.stringify({ type: "ai-title", aiTitle: "grok chat", sessionId: "sess-grok" })}\n`,
		);
		const { client, lineage } = await setup({
			claudeProjectsDir: projects,
			worktrees: [{ name: "platform.wt-a", path: wtPath, branch: "wt-a" }],
		});
		lineage.recordProvider("sess-grok", "grok");
		const res = (await client.call("listSessions", {
			repo: "platform",
			worktree: "platform.wt-a",
		})) as {
			sessions: { session_id: string; model?: string; provider?: string }[];
		};
		expect(res.sessions).toHaveLength(1);
		expect(res.sessions[0]?.model).toBeUndefined();
		expect(res.sessions[0]?.provider).toBe("grok");
	});

	it("listSessions errors on an unknown worktree", async () => {
		const { client } = await setup({ worktrees: [] });
		await expect(
			client.call("listSessions", { repo: "platform", worktree: "nope" }),
		).rejects.toThrow();
	});

	it("listSessions unions claude on-disk sessions with the daemon's own run-store sessions across providers", async () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		const wtPath = "/wt/platform.wt-a";
		const dir = join(projects, wtPath.replace(/[/.]/g, "-"));
		mkdirSync(dir, { recursive: true });
		// Manual claude run in this worktree — only visible via the on-disk scan.
		writeClaudeSessionFile(dir, "sess-claude", 1_000_000);

		const { client, store, runStore } = await setup({
			claudeProjectsDir: projects,
			worktrees: [{ name: "platform.wt-a", path: wtPath, branch: "wt-a" }],
		});

		// A daemon-launched grok run — grok never writes a claude on-disk
		// transcript, so it's only visible via the run store.
		const grokTask = store.create({
			prompt: "grok task prompt",
			repo: "platform",
			ref: "worktree:platform.wt-a",
			source: "mcp",
		});
		runStore.writeSnapshot(
			grokTask.id,
			{
				task: grokTask,
				definition: null,
				resolvedWorktree: wtPath,
				resolvedWorktreePath: wtPath,
				prompt: grokTask.prompt,
				model: "grok-4",
				provider: "grok",
			},
			(s) => s,
		);
		runStore.finishRun(
			grokTask.id,
			{
				result: { ...okRunResult, sessionId: "sess-grok" },
				outcome: "done",
				reason: null,
			},
			(s) => s,
		);

		// A daemon-launched codex run — same story.
		const codexTask = store.create({
			prompt: "codex task prompt",
			repo: "platform",
			ref: "worktree:platform.wt-a",
			source: "mcp",
		});
		runStore.writeSnapshot(
			codexTask.id,
			{
				task: codexTask,
				definition: null,
				resolvedWorktree: wtPath,
				resolvedWorktreePath: wtPath,
				prompt: codexTask.prompt,
				model: "codex-1",
				provider: "codex",
			},
			(s) => s,
		);
		runStore.finishRun(
			codexTask.id,
			{
				result: { ...okRunResult, sessionId: "sess-codex" },
				outcome: "done",
				reason: null,
			},
			(s) => s,
		);

		const res = (await client.call("listSessions", {
			repo: "platform",
			worktree: "platform.wt-a",
		})) as { sessions: { session_id: string; provider?: string }[] };
		const byId = Object.fromEntries(
			res.sessions.map((s) => [s.session_id, s.provider]),
		);
		expect(byId["sess-claude"]).toBe("claude");
		expect(byId["sess-grok"]).toBe("grok");
		expect(byId["sess-codex"]).toBe("codex");
		expect(res.sessions).toHaveLength(3);
	});

	it("listSessions dedups a session id present in both sources, preferring run-store metadata but keeping the max mtime", async () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		const wtPath = "/wt/platform.wt-a";
		const dir = join(projects, wtPath.replace(/[/.]/g, "-"));
		mkdirSync(dir, { recursive: true });
		const diskMtimeMs = 5_000_000; // NEWER than the run-store record below.
		writeClaudeSessionFile(dir, "dup1", diskMtimeMs, [
			{ type: "ai-title", aiTitle: "disk title", sessionId: "dup1" },
		]);

		const { client, store, runStore } = await setup({
			claudeProjectsDir: projects,
			worktrees: [{ name: "platform.wt-a", path: wtPath, branch: "wt-a" }],
		});
		const task = store.create({
			prompt: "run-store prompt line",
			repo: "platform",
			ref: "worktree:platform.wt-a",
			source: "mcp",
		});
		runStore.writeSnapshot(
			task.id,
			{
				task,
				definition: null,
				resolvedWorktree: wtPath,
				resolvedWorktreePath: wtPath,
				prompt: task.prompt,
				model: "sonnet",
			},
			(s) => s,
		);
		runStore.finishRun(
			task.id,
			{
				result: { ...okRunResult, sessionId: "dup1" },
				outcome: "done",
				reason: null,
			},
			(s) => s,
		);
		// Older than the on-disk transcript's mtime.
		patchRunTimestamps(runStore, task.id, {
			finished_at: new Date(1_000_000).toISOString(),
		});

		const res = (await client.call("listSessions", {
			repo: "platform",
			worktree: "platform.wt-a",
		})) as {
			sessions: { session_id: string; mtime_ms: number; label: string }[];
		};
		const dup1 = res.sessions.filter((s) => s.session_id === "dup1");
		expect(dup1).toHaveLength(1); // appears once, not twice
		expect(dup1[0]?.label).toBe("run-store prompt line"); // run-store metadata wins
		expect(dup1[0]?.mtime_ms).toBe(diskMtimeMs); // max of the two mtimes survives
	});

	it("listSessions caps a provider at 5 sessions while another provider's sessions still appear", async () => {
		const wtPath = "/wt/platform.wt-a";
		const { client, store, runStore } = await setup({
			claudeProjectsDir: mkdtempSync(join(tmpdir(), "claude-projects-")),
			worktrees: [{ name: "platform.wt-a", path: wtPath, branch: "wt-a" }],
		});
		// 7 grok sessions — only the 5 most recent should survive.
		for (let i = 0; i < 7; i++) {
			const task = store.create({
				prompt: `grok run ${i}`,
				repo: "platform",
				ref: "worktree:platform.wt-a",
				source: "mcp",
			});
			runStore.writeSnapshot(
				task.id,
				{
					task,
					definition: null,
					resolvedWorktree: wtPath,
					resolvedWorktreePath: wtPath,
					prompt: task.prompt,
					model: "grok-4",
					provider: "grok",
				},
				(s) => s,
			);
			runStore.finishRun(
				task.id,
				{
					result: { ...okRunResult, sessionId: `sess-grok-${i}` },
					outcome: "done",
					reason: null,
				},
				(s) => s,
			);
			patchRunTimestamps(runStore, task.id, {
				finished_at: new Date(1_000_000 + i * 1000).toISOString(),
			});
		}
		// One codex session — must survive alongside grok's capped set.
		const codexTask = store.create({
			prompt: "codex run",
			repo: "platform",
			ref: "worktree:platform.wt-a",
			source: "mcp",
		});
		runStore.writeSnapshot(
			codexTask.id,
			{
				task: codexTask,
				definition: null,
				resolvedWorktree: wtPath,
				resolvedWorktreePath: wtPath,
				prompt: codexTask.prompt,
				model: "codex-1",
				provider: "codex",
			},
			(s) => s,
		);
		runStore.finishRun(
			codexTask.id,
			{
				result: { ...okRunResult, sessionId: "sess-codex" },
				outcome: "done",
				reason: null,
			},
			(s) => s,
		);
		patchRunTimestamps(runStore, codexTask.id, {
			finished_at: new Date(500_000).toISOString(),
		});

		const res = (await client.call("listSessions", {
			repo: "platform",
			worktree: "platform.wt-a",
		})) as { sessions: { session_id: string; provider?: string }[] };
		const grokIds = res.sessions
			.filter((s) => s.provider === "grok")
			.map((s) => s.session_id);
		expect(grokIds).toHaveLength(5);
		// The 5 MOST RECENT grok sessions survive (i = 2..6, highest finished_at).
		expect(new Set(grokIds)).toEqual(
			new Set([
				"sess-grok-6",
				"sess-grok-5",
				"sess-grok-4",
				"sess-grok-3",
				"sess-grok-2",
			]),
		);
		expect(res.sessions.some((s) => s.session_id === "sess-codex")).toBe(true);
	});

	it("listSessions merges providers into one recency-sorted list, interleaved not grouped by provider", async () => {
		const projects = mkdtempSync(join(tmpdir(), "claude-projects-"));
		const wtPath = "/wt/platform.wt-a";
		const dir = join(projects, wtPath.replace(/[/.]/g, "-"));
		mkdirSync(dir, { recursive: true });
		writeClaudeSessionFile(dir, "sess-claude", 3_000_000); // middle recency

		const { client, store, runStore } = await setup({
			claudeProjectsDir: projects,
			worktrees: [{ name: "platform.wt-a", path: wtPath, branch: "wt-a" }],
		});

		const grokTask = store.create({
			prompt: "grok",
			repo: "platform",
			ref: "worktree:platform.wt-a",
			source: "mcp",
		});
		runStore.writeSnapshot(
			grokTask.id,
			{
				task: grokTask,
				definition: null,
				resolvedWorktree: wtPath,
				resolvedWorktreePath: wtPath,
				prompt: grokTask.prompt,
				model: "grok-4",
				provider: "grok",
			},
			(s) => s,
		);
		runStore.finishRun(
			grokTask.id,
			{
				result: { ...okRunResult, sessionId: "sess-grok" },
				outcome: "done",
				reason: null,
			},
			(s) => s,
		);
		patchRunTimestamps(runStore, grokTask.id, {
			finished_at: new Date(4_000_000).toISOString(), // most recent overall
		});

		const codexTask = store.create({
			prompt: "codex",
			repo: "platform",
			ref: "worktree:platform.wt-a",
			source: "mcp",
		});
		runStore.writeSnapshot(
			codexTask.id,
			{
				task: codexTask,
				definition: null,
				resolvedWorktree: wtPath,
				resolvedWorktreePath: wtPath,
				prompt: codexTask.prompt,
				model: "codex-1",
				provider: "codex",
			},
			(s) => s,
		);
		runStore.finishRun(
			codexTask.id,
			{
				result: { ...okRunResult, sessionId: "sess-codex" },
				outcome: "done",
				reason: null,
			},
			(s) => s,
		);
		patchRunTimestamps(runStore, codexTask.id, {
			finished_at: new Date(2_000_000).toISOString(), // oldest overall
		});

		const res = (await client.call("listSessions", {
			repo: "platform",
			worktree: "platform.wt-a",
		})) as { sessions: { session_id: string }[] };
		// grok (4,000,000) > claude (3,000,000) > codex (2,000,000): sorted by
		// recency across all three providers, interleaved — not grouped by
		// provider (claude sits between the two run-store providers).
		expect(res.sessions.map((s) => s.session_id)).toEqual([
			"sess-grok",
			"sess-claude",
			"sess-codex",
		]);
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
				model: "claude/fable",
			})) as {
				target: { repo: string; ref: string };
				resumeSessionId: string;
				model: string;
			};
			expect(task.target.repo).toBe("platform");
			expect(task.target.ref).toBe("worktree:repo.fix-x");
			expect(task.resumeSessionId).toBe("sess-1");
			expect(task.model).toBe("claude/fable");
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
