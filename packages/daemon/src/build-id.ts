import { readdirSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/** The directory this module is loaded from. In a built daemon that is `dist/`. */
function selfDir(): string {
	return dirname(fileURLToPath(import.meta.url));
}

/**
 * A build fingerprint: the newest `.js` sibling mtime (ms) of this module, as a
 * string. `tsc` here runs with no incremental/composite config, so it rewrites
 * the ENTIRE `dist/` tree on every `pnpm build` — every `.js` mtime advances
 * together — making "max mtime across dist/*.js" a reliable "which build is
 * this" marker, computed once at startup.
 *
 * The daemon and the TUI both call this against the SAME `@queohoh/daemon`
 * package dir, so they can never compute the fingerprint differently. Returns
 * `"0"` when the dir holds no `.js` files (e.g. running from TypeScript source
 * under vitest) — in that mode both sides resolve the same source module and
 * both get `"0"`, so they correctly agree instead of flagging false staleness.
 */
export function currentBuildId(dir: string = selfDir()): string {
	let newest = 0;
	try {
		for (const entry of readdirSync(dir)) {
			if (!entry.endsWith(".js")) continue;
			const m = statSync(join(dir, entry)).mtimeMs;
			if (m > newest) newest = m;
		}
	} catch {
		return "0";
	}
	return String(newest);
}

/**
 * Absolute path to the daemon's CLI entry (`dist/cli.js`) — the same file
 * `scripts/daemon-ensure.sh` runs as `node .../cli.js daemon`. The TUI resolves
 * this via the `@queohoh/daemon` package so a self-heal spawns the identical
 * launcher. Only meaningful when running from a build (dist); under vitest's
 * source alias it points at `cli.ts`, which the untested heal path never runs.
 */
export function daemonCliPath(): string {
	return fileURLToPath(new URL("./cli.js", import.meta.url));
}
