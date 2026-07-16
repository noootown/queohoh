import { homedir } from "node:os";
import type { Exec } from "@queohoh/core";

/**
 * Why this module exists — the minimal-PATH launch scenario.
 *
 * When the daemon is launched from something other than the user's interactive
 * login shell (launchd, a bare `execFile`, a stripped-down CI/test shell), it
 * inherits a minimal PATH that typically lacks `/opt/homebrew/bin`. Every DIRECT
 * tool call the daemon makes goes through `defaultExec` (a plain `execFile` with
 * NO shell — see `packages/core/src/resolver-io.ts`), so command resolution uses
 * `process.env.PATH` verbatim with no rescue. On such a launch `gh` is invisible:
 * `ghPrMap` fails every enrichment sweep, and gh-backed discovery scripts fail.
 *
 * The asymmetry that hides this: task DISCOVERY runs through `/bin/bash -lc`
 * (`packages/core/src/discovery.ts`), and a login shell on macOS runs
 * `path_helper` (via `/etc/paths.d/homebrew`) which restores `/opt/homebrew/bin`.
 * So discovery quietly survives a minimal PATH while the bare `execFile` calls
 * do not — the same gh binary is reachable from one code path and not the other.
 *
 * The fix (below) borrows the login shell's own PATH once at startup and merges
 * it into `process.env.PATH`, so every subsequent bare `execFile` sees the same
 * directories the `-lc` discovery path already enjoyed.
 */

/** The one-time warning emitted when `gh` is still unresolvable after the PATH
 * merge. Exported so the startup probe and its test assert on the same string. */
export const GH_MISSING_WARNING =
	"[queohoh] gh not found on PATH — PR enrichment and gh-backed discovery will fail; install gh or fix the daemon's PATH";

/**
 * Merge the login shell's PATH into the daemon's current PATH, pure and
 * order-preserving. EXISTING entries come first (the inherited PATH may hold a
 * deliberate override that must keep winning over a login-shell default of the
 * same tool), then any login-shell entry not already present is appended.
 *
 * Each entry is trimmed so a trailing newline from `echo "$PATH"` never rides
 * along on the last segment, and empty segments (leading/trailing/doubled
 * colons) are dropped. Dedup is first-occurrence-wins across both inputs.
 */
export function mergePathEntries(current: string, loginShell: string): string {
	const seen = new Set<string>();
	const merged: string[] = [];
	const add = (raw: string): void => {
		const entry = raw.trim();
		// Drop empty segments (a trailing `:` or the newline-only tail) and any
		// entry we've already taken — the first occurrence keeps its position.
		if (entry.length === 0 || seen.has(entry)) return;
		seen.add(entry);
		merged.push(entry);
	};
	for (const entry of current.split(":")) add(entry);
	for (const entry of loginShell.split(":")) add(entry);
	return merged.join(":");
}

/** A mutable PATH carrier — defaults to `process.env` in production, but a plain
 * object in tests so the merge can be asserted without touching real env. */
interface PathEnv {
	PATH?: string;
}

/**
 * Resolve the login shell's PATH via `/bin/bash -lc 'echo "$PATH"'` (the same
 * `-lc` invocation that lets discovery survive a minimal PATH) and merge it into
 * `env.PATH`. Injectable `exec` so the effect is unit-testable without spawning
 * bash. On ANY failure — non-zero exit or a throwing exec — the current PATH is
 * left untouched (a merge is a best-effort improvement, never a hard dependency).
 *
 * Must run BEFORE the engine starts ticking so the first enrichment sweep and
 * the first discovery already see the widened PATH.
 */
export async function normalizeDaemonPath(
	exec: Exec,
	opts: { env?: PathEnv } = {},
): Promise<void> {
	const env = opts.env ?? process.env;
	try {
		const { stdout, exitCode } = await exec(
			"/bin/bash",
			["-lc", 'echo "$PATH"'],
			// cwd is irrelevant to `echo "$PATH"`; home is a stable, always-present
			// directory so the spawn itself never fails on a missing cwd.
			{ cwd: homedir() },
		);
		if (exitCode !== 0) return; // bash unhappy — keep the current PATH as-is.
		const merged = mergePathEntries(env.PATH ?? "", stdout);
		if (merged.length > 0) env.PATH = merged;
	} catch {
		// Spawn failed outright (no /bin/bash, injected reject) — keep PATH as-is.
	}
}

/**
 * One-time startup probe: run `gh --version` through the (now PATH-normalized)
 * exec plumbing. If it fails, emit ONE prominent warning and return false — the
 * daemon must NOT crash, since enrichment/discovery are optional and it has
 * non-gh duties. Returns true when gh is reachable.
 */
export async function probeGh(
	exec: Exec,
	opts: { warn?: (message: string) => void } = {},
): Promise<boolean> {
	const warn = opts.warn ?? console.warn;
	try {
		const { exitCode } = await exec("gh", ["--version"], { cwd: homedir() });
		if (exitCode === 0) return true;
	} catch {
		// fall through to the single warning below
	}
	warn(GH_MISSING_WARNING);
	return false;
}
