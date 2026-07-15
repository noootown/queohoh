#!/usr/bin/env node
import { unlinkSync } from "node:fs";
import { basename, dirname } from "node:path";
import {
	buildSecretMap,
	executeClaude,
	makeRedactor,
	RunStore,
} from "@queohoh/core";

async function main(): Promise<void> {
	const runDir = process.argv[2];
	if (!runDir) {
		console.error("shim: missing run dir argument");
		process.exit(2);
	}
	// The run dir is `<runsDir>/<taskId>`: reconstruct the RunStore + taskId
	// from it rather than threading a second argument.
	const runStore = new RunStore(dirname(runDir));
	const taskId = basename(runDir);
	const spec = runStore.readSpawnJson(taskId);
	if (!spec) {
		console.error("shim: no spawn.json in run dir");
		process.exit(2);
	}
	// Consume the spec immediately: it holds the unredacted prompt.
	try {
		unlinkSync(runStore.spawnJsonPath(taskId));
	} catch {}

	const redact = makeRedactor(buildSecretMap(process.env));
	let claudePid: number | null = null;
	// A daemon Stop SIGTERMs the shim; forward to claude's own process group so
	// the whole tree dies. executeClaude's close handler then records the signal.
	process.on("SIGTERM", () => {
		if (claudePid !== null) {
			try {
				process.kill(-claudePid, "SIGTERM");
			} catch {}
		}
	});

	const result = await executeClaude({
		...spec,
		redact,
		onSpawned: (pid) => {
			claudePid = pid;
		},
	});
	runStore.writeResultJson(taskId, result);
	process.exit(0);
}

void main();
