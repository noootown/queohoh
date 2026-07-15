import { execFileSync } from "node:child_process";
import {
	chmodSync,
	copyFileSync,
	existsSync,
	mkdtempSync,
	readFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { RunStore } from "@queohoh/core";
import { describe, expect, it } from "vitest";

const FAKE = join(
	dirname(fileURLToPath(import.meta.url)),
	"..",
	"..",
	"..",
	"core",
	"src",
	"__tests__",
	"fixtures",
	"fake-claude.mjs",
);

const SHIM = fileURLToPath(new URL("../../dist/shim.js", import.meta.url));

describe("shim round-trip", () => {
	it("runs executeClaude and writes result.json + events + transcript", () => {
		if (!existsSync(SHIM)) {
			// dist not built in this environment; the mise `check` gate builds
			// first (`pnpm -r build`), so this only short-circuits ad-hoc runs.
			return;
		}

		// `claude` is resolved off PATH by executeClaude (no path separator in
		// the default bin name), and spawn.json carries no claudeBin override —
		// so exercising the real shim means putting a file literally named
		// `claude` on PATH, not just pointing PATH at the fixtures dir.
		const binDir = mkdtempSync(join(tmpdir(), "qo-shim-bin-"));
		const claudeBin = join(binDir, "claude");
		copyFileSync(FAKE, claudeBin);
		chmodSync(claudeBin, 0o755);

		const runsDir = mkdtempSync(join(tmpdir(), "qo-shim-runs-"));
		const taskId = "01SHIMTEST0000000000000000";
		const runStore = new RunStore(runsDir);
		const runDir = runStore.runDir(taskId);
		runStore.writeSpawnJson(taskId, {
			prompt: "do the thing",
			model: "opus",
			cwd: runDir,
			timeoutMs: 30_000,
			eventsPath: runStore.eventsPath(taskId),
			transcriptPath: runStore.transcriptPath(taskId),
		});

		execFileSync(process.execPath, [SHIM, runDir], {
			env: {
				...process.env,
				PATH: `${binDir}:${process.env.PATH}`,
			},
			stdio: "ignore",
		});

		const result = runStore.readResultJson(taskId);
		expect(result?.exitCode).toBe(0);
		expect(result?.sessionId).toBe("sess-123");
		expect(existsSync(runStore.eventsPath(taskId))).toBe(true);
		expect(readFileSync(runStore.transcriptPath(taskId), "utf-8")).toContain(
			"### Tool: Bash",
		);
		// spawn.json consumed: the shim unlinks it right after reading, since it
		// holds the unredacted prompt.
		expect(existsSync(runStore.spawnJsonPath(taskId))).toBe(false);
	});
});
