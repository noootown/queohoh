import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import type {
	ClaudeExecutor,
	Redactor,
	RunResult,
	RunStore,
	SpawnSpec,
} from "@queohoh/core";
import { executeClaude as _executeClaude } from "@queohoh/core";

/** Spawns a run and resolves when it settles. Returns null when the run
 * produced no result.json (the supervisor died) — the caller then settles
 * the task as `worker died`. onPid reports the process to signal for a Stop
 * (the shim pid in production; the claude child pid in-process). */
export type ShimSpawner = (
	taskId: string,
	spec: SpawnSpec,
	onPid: (pid: number) => void,
) => Promise<RunResult | null>;

/** Production spawner: writes `spawn.json`, then spawns `dist/shim.js` as a
 * detached, unref'd node subprocess — its own process group and event-loop
 * lifetime independent of the daemon's, so a daemon reload/crash never kills
 * the run. Resolves via the `close` event while the daemon is alive; a
 * daemon that dies and comes back has no handle to await and re-adopts the
 * run through the engine's adoption sweep instead. */
export function makeShimSpawner(opts: {
	runStore: RunStore;
	execPath?: string;
	shimCliPath?: string;
}): ShimSpawner {
	const execPath = opts.execPath ?? process.execPath;
	const shimCli =
		opts.shimCliPath ?? fileURLToPath(new URL("./shim.js", import.meta.url));
	return (taskId, spec, onPid) => {
		// spawn.json first: the shim reads it on boot.
		opts.runStore.writeSpawnJson(taskId, spec);
		const child = spawn(execPath, [shimCli, opts.runStore.runDir(taskId)], {
			detached: true, // own process group; survives daemon death
			stdio: "ignore",
			env: process.env,
		});
		child.unref(); // do not keep the daemon's event loop alive for it
		if (child.pid) onPid(child.pid);
		return new Promise<RunResult | null>((resolve) => {
			// While the daemon is alive it is the shim's parent, so `close` fires
			// on exit. A returning daemon has no handle and adopts via the sweep.
			child.on("close", () => resolve(opts.runStore.readResultJson(taskId)));
			child.on("error", () => resolve(null));
		});
	};
}

/** Default/test spawner: runs executeClaude IN-PROCESS (no detachment). Used
 * when no real ShimSpawner is injected. onPid receives the claude child pid. */
export function inProcessSpawner(
	executeClaude: ClaudeExecutor = _executeClaude,
	redact: Redactor = (s) => s,
): ShimSpawner {
	return (_taskId, spec, onPid) =>
		executeClaude({ ...spec, redact, onSpawned: onPid });
}
