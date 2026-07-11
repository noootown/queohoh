import { mkdirSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

function defaultIsPidAlive(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch {
		return false;
	}
}

/**
 * Take the single-daemon pidfile lock. The common path is an ATOMIC exclusive
 * create (`wx`) — two daemons racing to start can never both win it (the old
 * check-then-write version had a TOCTOU window that let simultaneous launches
 * both pass). When the file already exists it is either a live daemon (lose) or
 * a stale leftover: stale takeover rewrites the file, then re-reads it to
 * confirm we are the pid that landed — if a concurrent racer overwrote us, we
 * concede rather than run alongside it.
 */
export function acquireLock(
	pidFile: string,
	opts?: { isPidAlive?: (pid: number) => boolean },
): boolean {
	const isPidAlive = opts?.isPidAlive ?? defaultIsPidAlive;
	mkdirSync(dirname(pidFile), { recursive: true });
	try {
		writeFileSync(pidFile, String(process.pid), { flag: "wx" });
		return true;
	} catch {
		// exists (or unwritable) — fall through to the live/stale check
	}
	let raw: string;
	try {
		raw = readFileSync(pidFile, "utf-8").trim();
	} catch {
		// vanished between the wx failure and the read (owner released) — retry
		// the atomic create once; a second failure means an active racer: concede.
		try {
			writeFileSync(pidFile, String(process.pid), { flag: "wx" });
			return true;
		} catch {
			return false;
		}
	}
	const pid = Number(raw);
	if (Number.isInteger(pid) && pid > 0 && isPidAlive(pid)) {
		return false;
	}
	// Stale pidfile (dead or garbage pid): overwrite, then verify we won — two
	// stale-takeover racers both reach this write; the read-back picks one winner.
	try {
		writeFileSync(pidFile, String(process.pid));
		return readFileSync(pidFile, "utf-8").trim() === String(process.pid);
	} catch {
		return false;
	}
}

export function releaseLock(pidFile: string): void {
	try {
		unlinkSync(pidFile);
	} catch {}
}
