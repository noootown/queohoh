import {
	existsSync,
	mkdirSync,
	readFileSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { dirname } from "node:path";

function defaultIsPidAlive(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch {
		return false;
	}
}

export function acquireLock(
	pidFile: string,
	opts?: { isPidAlive?: (pid: number) => boolean },
): boolean {
	const isPidAlive = opts?.isPidAlive ?? defaultIsPidAlive;
	if (existsSync(pidFile)) {
		const raw = readFileSync(pidFile, "utf-8").trim();
		const pid = Number(raw);
		if (Number.isInteger(pid) && pid > 0 && isPidAlive(pid)) {
			return false;
		}
	}
	mkdirSync(dirname(pidFile), { recursive: true });
	writeFileSync(pidFile, String(process.pid));
	return true;
}

export function releaseLock(pidFile: string): void {
	try {
		unlinkSync(pidFile);
	} catch {}
}
