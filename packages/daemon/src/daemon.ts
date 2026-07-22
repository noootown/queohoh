import { existsSync, mkdirSync, watch, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import {
	buildSecretMap,
	createResolverIO,
	DiscussStore,
	defaultExec,
	definitionExists,
	executeClaude,
	executeVerify,
	globalWorkspaceDir,
	instantiateDefinition,
	loadGlobalConfig,
	loadProjectVars,
	makeRedactor,
	projectWorkspaceDir,
	QueueStore,
	RunStore,
	resolveDefinition,
	SessionLineageStore,
	SessionRegistry,
} from "@queohoh/core";
import { ApiServer } from "./api.js";
import { DiscussService } from "./discuss-service.js";
import { Engine } from "./engine.js";
import { loadWorkspaceEnv } from "./env-loader.js";
import { acquireLock, releaseLock } from "./lock.js";
import { normalizeDaemonPath, probeGh } from "./path-env.js";
import {
	configPath,
	discussPath,
	pidPath,
	runsPath,
	sessionLineagePath,
	sessionsPath,
	socketPath,
	statePath,
} from "./paths.js";
import { SettingsStore } from "./settings-store.js";
import { makeShimSpawner } from "./shim-host.js";
import { UsagePoller } from "./usage-poller.js";

const STARTER_CONFIG = `# queohoh config (lives in your private config workspace)
#
# Point the daemon at this file via:
#   export QUEOHOH_WORKSPACE=~/path/to/this-directory
# (config is then $QUEOHOH_WORKSPACE/config.yaml). Optional overrides:
#   QUEOHOH_CONFIG=/path/to/config.yaml
#   QUEOHOH_STATE_DIR=~/.local/state/queohoh
#
# workspace: .                    # or an absolute path; defaults relative to this file's tree
# projects:
#   - name: my-app
#     path: ~/code/my-app
# max_concurrent_tasks: 5   # per project
# archive_after_days: 7
# vars: {}
`;

export async function startDaemon(): Promise<{ stop: () => Promise<void> }> {
	const state = statePath();
	const cfgPath = configPath();
	if (!existsSync(cfgPath)) {
		mkdirSync(dirname(cfgPath), { recursive: true });
		writeFileSync(cfgPath, STARTER_CONFIG);
		console.log(`created starter config at ${cfgPath}`);
		if (!process.env.QUEOHOH_WORKSPACE && !process.env.QUEOHOH_CONFIG) {
			console.log(
				"tip: set QUEOHOH_WORKSPACE to your config workspace so discovery is env-only",
			);
		}
	}
	const config = loadGlobalConfig(cfgPath);

	// Load workspace secrets into process.env BEFORE buildSecretMap so they are
	// redacted like any other env secret, and BEFORE any run is spawned (runs
	// inherit process.env). Under launchd the daemon env is otherwise minimal.
	const loadedEnvKeys = loadWorkspaceEnv(config.workspace);
	if (loadedEnvKeys.length > 0) {
		console.log(
			`loaded ${loadedEnvKeys.length} var(s) from ${config.workspace}/.env`,
		);
	}

	// Widen PATH BEFORE anything shells out. A minimal-PATH launch (launchd, a
	// bare execFile, a stripped test shell) leaves `gh` invisible to the daemon's
	// direct `execFile` calls; borrowing the login shell's PATH restores it. Then
	// probe `gh` once so a still-missing binary fails loudly instead of dribbling
	// one debug line per enrichment sweep. Both run before the first engine tick.
	// See path-env.ts for the path_helper rescue asymmetry this compensates for.
	await normalizeDaemonPath(defaultExec);
	await probeGh(defaultExec);

	const pid = pidPath(state);
	if (!acquireLock(pid)) {
		console.error("queohoh daemon already running");
		process.exit(1);
	}

	const store = new QueueStore(state);
	const runStore = new RunStore(runsPath(state));
	const registry = new SessionRegistry(sessionsPath(state));
	const lineage = new SessionLineageStore(sessionLineagePath(state));
	const redact = makeRedactor(buildSecretMap(process.env));
	const resolverIO = createResolverIO(defaultExec);
	// Persisted operator settings (active provider). Constructed here so both the
	// Engine (reads it to head each run's fallback chain) and the ApiServer
	// (reads/mutates it via the `settings` / `set_active_provider` RPCs) share one
	// instance. Snaps a disabled/unknown persisted provider to precedence-first
	// enabled at construction (a config load) and logs it.
	const settings = new SettingsStore(state, config.providers);

	// Late-bound broadcast to resolve the Engine/UsagePoller↔ApiServer cycle
	// without reaching into private state: both are built first with onChange
	// that defers to broadcastRef, which we point at the server once it exists.
	let broadcastRef: () => void = () => {};

	const engine = new Engine({
		store,
		runStore,
		registry,
		config,
		resolverIO,
		exec: defaultExec,
		executeClaude,
		executeVerify,
		redact,
		lineage,
		onChange: () => broadcastRef(),
		// Read the active provider fresh at each run so a `set_active_provider`
		// re-heads the NEXT run's fallback chain.
		activeProvider: () => settings.activeProvider(),
		// Read the paused-cron set fresh each tick so a `set_cron_enabled` toggle
		// gates the very next cron evaluation.
		isCronDisabled: (key) => settings.isCronDisabled(key),
		// Detached per-run shim: a daemon reload/crash never kills a live run, and
		// the adoption sweep re-adopts it on return. executeClaude stays wired as
		// the in-process fallback the Engine builds when no spawnShim is present.
		spawnShim: makeShimSpawner({ runStore }),
	});

	// Provider usage poller (design: provider-usage-header). Polls EVERY enabled
	// provider on a 60s interval so the TUI header can show all chips (active
	// colored, inactive grey). onChange uses the same late-bound broadcastRef so
	// a completed fetch re-renders every TUI without the poller knowing about
	// ApiServer. providers() is re-read each tick so enable/disable takes effect.
	const usagePoller = new UsagePoller({
		providers: () =>
			config.providers.filter((p) => p.enabled).map((p) => p.name),
		onChange: () => broadcastRef(),
	});

	// Reserved review sessions (juice AI review). In-process turns — not the
	// QUEUE shim — so a discuss chat never competes with agent tasks for
	// concurrency slots and survives as its own store under state/discuss/.
	// queue deps power promote_fix / promote_pr_reply into the normal agent
	// queue (full tools, worktree from DiscussMeta).
	const discuss = new DiscussService({
		store: new DiscussStore(discussPath(state)),
		lineage,
		settings,
		config,
		redact,
		queue: {
			create: (input) => store.create(input),
			resolveCwd: (cwd) => engine.resolveCwd(cwd),
			tryRunDefinition: async ({ repo, name, args }) => {
				const project = config.projects.find((p) => p.name === repo);
				if (!project) return null;
				const projectDir = projectWorkspaceDir(config, repo);
				const globalDir = globalWorkspaceDir(config);
				// Example defs must be copied into the workspace (see examples/README).
				// Missing → promote falls back to an ad-hoc prompt with the same rules.
				if (
					!definitionExists(projectDir, name) &&
					!definitionExists(globalDir, name)
				) {
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
							// Operator-initiated promote is "run NOW" — never silent-dedup.
							bypassDedup: true,
						},
					);
					return created[0] ? { id: created[0].id } : null;
				} catch (err) {
					console.warn(
						`[discuss] tryRunDefinition ${repo}/${name} failed:`,
						err instanceof Error ? err.message : err,
					);
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
		usagePoller,
		discuss,
		onMutation: () => {
			void engine.tick().then(() => server.broadcast());
		},
		// Self-heal: the TUI calls `shutdown` when the on-disk build is newer than
		// this process. Exit cleanly so a fresh daemon (spawned by the TUI) takes
		// over; launchd/daemon-ensure will also happily re-launch us.
		onShutdown: () => {
			void stop().then(() => process.exit(0));
		},
	});

	broadcastRef = () => server.broadcast();

	await server.listen(socketPath(state));
	// Start after listen so the first onChange (immediate refresh) can broadcast
	// to any client that connects during the first probe.
	usagePoller.start();

	// Watch the tasks dir — a dropped file IS an enqueue.
	let debounce: NodeJS.Timeout | null = null;
	const watcher = watch(join(state, "tasks"), () => {
		if (debounce) clearTimeout(debounce);
		debounce = setTimeout(() => {
			// External write (or our own rename) — drop list caches so the next
			// snapshot re-reads. Writes through QueueStore already invalidate.
			store.reload();
			void engine.tick().then(() => server.broadcast());
		}, 250);
	});

	const interval = setInterval(() => {
		void engine.tick().then(() => server.broadcast());
	}, 2000);
	interval.unref();

	await engine.tick();
	console.log(`queohoh daemon up — socket ${socketPath(state)}`);

	const stop = async () => {
		usagePoller.stop();
		watcher.close();
		clearInterval(interval);
		await server.close();
		releaseLock(pid);
	};
	process.on("SIGTERM", () => void stop().then(() => process.exit(0)));
	process.on("SIGINT", () => void stop().then(() => process.exit(0)));
	return { stop };
}
