import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { ApiClient, daemonCliPath } from "@queohoh/daemon";

export type HealAction = "none" | "restart-now" | "defer";

/**
 * True when the daemon's reported build differs from what's on disk. A
 * pre-feature daemon sends `undefined`, which is always stale — it definitionally
 * predates the buildId field.
 */
export function isStale(
	snapshotBuildId: string | undefined,
	diskBuildId: string,
): boolean {
	return snapshotBuildId !== diskBuildId;
}

/**
 * Pure self-heal decision. Given the daemon's build (from its snapshot), the
 * current on-disk build, how many tasks are running, and the disk build we last
 * attempted to heal toward, decide what to do:
 *
 * - `none`     — up to date, OR we already tried to reach this disk build and it
 *                didn't take (stop, to avoid a restart loop).
 * - `defer`    — stale but a task is running; wait until idle.
 * - `restart-now` — stale and idle; restart the daemon.
 *
 * The loop guard keys on `diskBuildId` (the target): once we've attempted to
 * reach build X we won't retry X, but a fresh build (disk moves to Y) is a new
 * mismatch worth healing.
 */
export function decideHeal(params: {
	snapshotBuildId: string | undefined;
	diskBuildId: string;
	runningCount: number;
	lastHealedBuildId: string | null;
}): HealAction {
	const { snapshotBuildId, diskBuildId, runningCount, lastHealedBuildId } =
		params;
	if (!isStale(snapshotBuildId, diskBuildId)) return "none";
	if (lastHealedBuildId === diskBuildId) return "none"; // already tried this build
	if (runningCount > 0) return "defer";
	return "restart-now";
}

/** Pidfile sits beside the socket (see daemon paths.ts: both under `daemon/`). */
function pidPathFor(sockPath: string): string {
	return join(dirname(sockPath), "daemon.pid");
}

async function socketAnswers(sockPath: string): Promise<boolean> {
	const client = new ApiClient();
	try {
		await client.connect(sockPath);
		return (await client.call("ping")) === "pong";
	} catch {
		return false;
	} finally {
		client.close();
	}
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Thin orchestration (intentionally untested — child processes + sockets):
 * ask the daemon to shut down (falling back to SIGTERM for an old daemon that
 * lacks the RPC), wait for the socket to go quiet, then spawn a fresh detached
 * daemon. The existing reconnect loop in use-daemon.ts picks the new one up.
 *
 * Returns false without spawning if the daemon reports it is busy — we must
 * never force-kill a daemon that has a task running.
 */
export async function performHeal(opts: {
	sockPath: string;
}): Promise<boolean> {
	const { sockPath } = opts;
	let shutdownAccepted = false;
	const client = new ApiClient();
	try {
		await client.connect(sockPath);
		await client.call("shutdown");
		shutdownAccepted = true;
	} catch (err) {
		// A task raced in after our idle check — respect the busy guard, abort.
		if (err instanceof Error && err.message.includes("busy")) {
			client.close();
			return false;
		}
		// Otherwise: unknown-method (old daemon) or unreachable → pidfile fallback.
	} finally {
		client.close();
	}

	if (!shutdownAccepted) {
		try {
			const pid = Number(readFileSync(pidPathFor(sockPath), "utf-8").trim());
			if (Number.isInteger(pid) && pid > 0) process.kill(pid, "SIGTERM");
		} catch {
			// no pidfile / already gone — proceed to spawn regardless
		}
	}

	// Poll until the old socket stops answering (bounded ~5s).
	const deadline = Date.now() + 5000;
	while (Date.now() < deadline) {
		if (!(await socketAnswers(sockPath))) break;
		await sleep(150);
	}

	const child = spawn(process.execPath, [daemonCliPath(), "daemon"], {
		detached: true,
		stdio: "ignore",
	});
	child.unref();
	return true;
}
