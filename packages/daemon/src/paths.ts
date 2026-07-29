import { homedir } from "node:os";
import { join } from "node:path";

/** Expand a leading `~/` to the home directory; leave absolute/relative paths alone. */
function expandTilde(p: string): string {
	if (p === "~") return homedir();
	if (p.startsWith("~/")) return join(homedir(), p.slice(2));
	return p;
}

/**
 * Machine state dir (queue, runs, socket). Override with `QUEOHOH_STATE_DIR`.
 * Stays out of the config workspace on purpose — runtime state is not config.
 */
export function statePath(): string {
	const override = process.env.QUEOHOH_STATE_DIR;
	if (override) return expandTilde(override);
	return join(homedir(), ".local/state/queohoh");
}

/**
 * Config workspace root — the private tree that holds `config.yaml`, task
 * definitions, and per-project vars. Set via `QUEOHOH_WORKSPACE` (the preferred
 * public-product discovery path: env only, no hard-coded personal paths in the
 * daemon). `null` when unset.
 */
export function workspaceEnvPath(): string | null {
	const w = process.env.QUEOHOH_WORKSPACE;
	if (!w || w.trim() === "") return null;
	return expandTilde(w.trim());
}

/**
 * Path to the operator's global config file.
 *
 * Resolution order (first hit wins):
 * 1. `QUEOHOH_CONFIG` — explicit file override (tests, multi-profile).
 * 2. `$QUEOHOH_WORKSPACE/config.yaml` — env-only workspace discovery (preferred).
 * 3. `~/.config/queohoh/config.yaml` — legacy XDG path for pre-env installs.
 *
 * Public docs should only mention (1)/(2); (3) stays as a soft fallback so an
 * existing machine keeps booting without an env export.
 */
export function configPath(): string {
	const explicit = process.env.QUEOHOH_CONFIG;
	if (explicit && explicit.trim() !== "") return expandTilde(explicit.trim());
	const ws = workspaceEnvPath();
	if (ws) return join(ws, "config.yaml");
	return join(homedir(), ".config/queohoh/config.yaml");
}

export const socketPath = (state: string) => join(state, "daemon/daemon.sock");
export const pidPath = (state: string) => join(state, "daemon/daemon.pid");
export const sessionsPath = (state: string) =>
	join(state, "daemon/sessions.json");
export const settingsPath = (state: string): string =>
	join(state, "daemon/settings.json");
export const sessionLineagePath = (state: string): string =>
	join(state, "daemon/session-lineage.json");
export const runsPath = (state: string) => join(state, "runs");
