import { existsSync, mkdirSync, watch, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import {
	buildSecretMap,
	createResolverIO,
	defaultExec,
	executeClaude,
	executeVerify,
	loadGlobalConfig,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionLineageStore,
	SessionRegistry,
} from "@queohoh/core";
import { ApiServer } from "./api.js";
import { Engine } from "./engine.js";
import { acquireLock, releaseLock } from "./lock.js";
import { normalizeDaemonPath, probeGh } from "./path-env.js";
import {
	configPath,
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

const STARTER_CONFIG = `# queohoh global config
# workspace: ~/workspace/queohoh
# projects:
#   - name: platform
#     path: ~/workspace/platform
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
	}
	const config = loadGlobalConfig(cfgPath);

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

	// Provider usage poller (design: provider-usage-header). onChange uses the
	// same late-bound broadcastRef so a completed fetch re-renders every TUI
	// without the poller knowing about ApiServer.
	const usagePoller = new UsagePoller({
		activeProvider: () => settings.activeProvider(),
		onChange: () => broadcastRef(),
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
