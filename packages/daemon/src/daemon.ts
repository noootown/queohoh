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
import {
	configPath,
	pidPath,
	runsPath,
	sessionLineagePath,
	sessionsPath,
	socketPath,
	statePath,
} from "./paths.js";

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

	// Late-bound broadcast to resolve the Engine<->ApiServer cycle without
	// reaching into private state: the Engine is built first with an onChange
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
	});

	const server = new ApiServer({
		engine,
		store,
		runStore,
		registry,
		config,
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
		watcher.close();
		clearInterval(interval);
		await server.close();
		releaseLock(pid);
	};
	process.on("SIGTERM", () => void stop().then(() => process.exit(0)));
	process.on("SIGINT", () => void stop().then(() => process.exit(0)));
	return { stop };
}
