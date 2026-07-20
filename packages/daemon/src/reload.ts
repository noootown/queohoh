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

export interface ReloadSteps {
	repoRoot: () => string | null;
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
 * Orchestrate reload: locate root → build → restart → verify. Build failures
 * abort before any daemon is touched. Reload always proceeds regardless of
 * running tasks now — the detached shim survives a daemon restart, and the
 * returning daemon re-adopts in-flight runs via the adoption sweep, so there
 * is no longer anything to guard against. Returns the exit code.
 */
export async function runReload(
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

	log.info(`building ${root}`);
	const buildExit = await steps.build(root);
	if (buildExit !== 0) {
		log.error(`build failed (exit ${buildExit}) — daemon left untouched`);
		return 1;
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
/** Must match the unit basename written by `systemd:install` (cli.ts). */
const SYSTEMD_UNIT = "queohoh.daemon.service";

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

/**
 * Prefer a supervised restart (launchd on macOS, systemd --user on Linux)
 * when the default state dir is in use and that supervisor owns the daemon.
 * Fall back to pidfile SIGTERM + detached re-spawn for one-shot / bare starts
 * and for QUEOHOH_STATE_DIR overrides (hermetic tests, alt deployments) so we
 * never kick a daemon we aren't talking to.
 */
async function supervisedRestart(): Promise<boolean> {
	if (process.env.QUEOHOH_STATE_DIR !== undefined) return false;

	const uid = process.getuid?.() ?? 0;
	const launchdTarget = `gui/${uid}/${LAUNCHD_LABEL}`;
	if ((await execCode("launchctl", ["print", launchdTarget])) === 0) {
		await execCode("launchctl", ["kickstart", "-k", launchdTarget]);
		return true;
	}

	// is-active returns 0 only when the unit is running; inactive/missing unit
	// or absent systemctl all fall through to the pidfile path.
	if (
		(await execCode("systemctl", ["--user", "is-active", SYSTEMD_UNIT])) === 0
	) {
		await execCode("systemctl", ["--user", "restart", SYSTEMD_UNIT]);
		return true;
	}

	return false;
}

export function defaultReloadSteps(cliPath: string): ReloadSteps {
	const state = statePath();
	const sock = socketPath(state);
	const logPath = join(state, "daemon/daemon.log");

	return {
		repoRoot: () => repoRootFromCli(cliPath),

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
			if (await supervisedRestart()) return;

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
