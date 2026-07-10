import { spawn } from "node:child_process";
import { existsSync, mkdirSync, openSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { ApiClient } from "./client.js";
import { pidPath, socketPath, statePath } from "./paths.js";

/**
 * Derive the repo root from the built CLI's own path
 * (<root>/packages/daemon/dist/cli.js → four segments up) and sanity-check
 * that a pnpm workspace lives there.
 */
export function repoRootFromCli(
	cliPath: string,
	exists: (p: string) => boolean = existsSync,
): string | null {
	const root = resolve(cliPath, "../../../..");
	return exists(join(root, "pnpm-workspace.yaml")) ? root : null;
}

export interface BusyVerdict {
	proceed: boolean;
	message: string | null;
}

/** Decide whether reload may proceed given the daemon's running set. */
export function busyVerdict(
	running: string[] | null,
	force: boolean,
): BusyVerdict {
	if (running === null) {
		return {
			proceed: true,
			message: "daemon not reachable — nothing running",
		};
	}
	if (running.length === 0) return { proceed: true, message: null };
	if (force) {
		return {
			proceed: true,
			message: `--force: restarting with ${running.length} running task(s) (${running.join(", ")}) — they will be marked failed by the orphan sweep`,
		};
	}
	return {
		proceed: false,
		message: `refusing to reload: ${running.length} task(s) running (${running.join(", ")}). Re-run with --force to restart anyway.`,
	};
}

export interface ReloadSteps {
	repoRoot: () => string | null;
	runningTasks: () => Promise<string[] | null>;
	build: (repoRoot: string) => Promise<number>;
	restart: () => Promise<void>;
	verify: () => Promise<boolean>;
	logTail: () => string;
}

export interface ReloadLog {
	info: (msg: string) => void;
	error: (msg: string) => void;
}

/**
 * Orchestrate reload: locate root → busy guard → build → restart → verify.
 * Build failures abort before any daemon is touched. Returns the exit code.
 */
export async function runReload(
	opts: { force: boolean },
	steps: ReloadSteps,
	log: ReloadLog,
): Promise<number> {
	const root = steps.repoRoot();
	if (root === null) {
		log.error(
			"cannot locate repo root (no pnpm-workspace.yaml above the CLI) — is the binary inside a checkout?",
		);
		return 1;
	}

	const verdict = busyVerdict(await steps.runningTasks(), opts.force);
	if (verdict.message !== null) {
		(verdict.proceed ? log.info : log.error)(verdict.message);
	}
	if (!verdict.proceed) return 1;

	log.info(`building ${root}`);
	const buildExit = await steps.build(root);
	if (buildExit !== 0) {
		log.error(`build failed (exit ${buildExit}) — daemon left untouched`);
		return 1;
	}

	if (!opts.force) {
		const postBuild = busyVerdict(await steps.runningTasks(), false);
		if (!postBuild.proceed) {
			log.error(postBuild.message ?? "refusing to reload: tasks running");
			log.error(
				"build succeeded — re-run reload once idle and it will restart quickly.",
			);
			return 1;
		}
	}

	await steps.restart();
	if (!(await steps.verify())) {
		log.error("daemon did not become reachable after restart; last log lines:");
		log.error(steps.logTail());
		return 1;
	}
	log.info("daemon reloaded");
	return 0;
}

const LAUNCHD_LABEL = "com.queohoh.daemon";

function pidAlive(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch {
		return false;
	}
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Run a command to completion with no output; resolve with its exit code. */
function execCode(cmd: string, args: string[]): Promise<number> {
	return new Promise((res) => {
		const child = spawn(cmd, args, { stdio: "ignore" });
		child.on("close", (code) => res(code ?? 1));
		child.on("error", () => res(1));
	});
}

export function defaultReloadSteps(cliPath: string): ReloadSteps {
	const state = statePath();
	const sock = socketPath(state);
	const logPath = join(state, "daemon/daemon.log");

	return {
		repoRoot: () => repoRootFromCli(cliPath),

		runningTasks: async () => {
			const client = new ApiClient();
			try {
				await client.connect(sock);
				const s = (await client.call("state")) as { running: string[] };
				return s.running;
			} catch {
				return null;
			} finally {
				client.close();
			}
		},

		build: (root) =>
			new Promise((res) => {
				const child = spawn("pnpm", ["-r", "build"], {
					cwd: root,
					stdio: "inherit",
				});
				child.on("close", (code) => res(code ?? 1));
				child.on("error", () => res(1));
			}),

		restart: async () => {
			// launchd governs only the default state dir; with QUEOHOH_STATE_DIR
			// overridden (hermetic tests, alt deployments) always take the
			// pidfile path so we never kick a daemon we aren't talking to.
			if (process.env.QUEOHOH_STATE_DIR === undefined) {
				const uid = process.getuid?.() ?? 0;
				const target = `gui/${uid}/${LAUNCHD_LABEL}`;
				if ((await execCode("launchctl", ["print", target])) === 0) {
					await execCode("launchctl", ["kickstart", "-k", target]);
					return;
				}
			}
			let pid: number | null = null;
			try {
				pid = Number.parseInt(readFileSync(pidPath(state), "utf-8").trim(), 10);
			} catch {}
			if (pid !== null && Number.isFinite(pid) && pid > 0 && pidAlive(pid)) {
				process.kill(pid, "SIGTERM");
				for (let i = 0; i < 10 && pidAlive(pid); i++) await sleep(500);
				if (pidAlive(pid)) process.kill(pid, "SIGKILL");
			}
			mkdirSync(join(state, "daemon"), { recursive: true });
			const logFd = openSync(logPath, "a");
			spawn(process.execPath, [cliPath, "daemon"], {
				detached: true,
				stdio: ["ignore", logFd, logFd],
			}).unref();
		},

		verify: async () => {
			for (let i = 0; i < 10; i++) {
				const client = new ApiClient();
				try {
					await client.connect(sock);
					if ((await client.call("ping")) === "pong") return true;
				} catch {
				} finally {
					client.close();
				}
				await sleep(500);
			}
			return false;
		},

		logTail: () => {
			try {
				return readFileSync(logPath, "utf-8").split("\n").slice(-20).join("\n");
			} catch {
				return "(no daemon log)";
			}
		},
	};
}
