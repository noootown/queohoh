import { homedir } from "node:os";
import { join } from "node:path";

export function statePath(): string {
	return (
		process.env.QUEOHOH_STATE_DIR ?? join(homedir(), ".local/state/queohoh")
	);
}

export function configPath(): string {
	return (
		process.env.QUEOHOH_CONFIG ?? join(homedir(), ".config/queohoh/config.yaml")
	);
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
