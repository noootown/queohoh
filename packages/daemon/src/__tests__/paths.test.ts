import { homedir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	configPath,
	pidPath,
	runsPath,
	sessionLineagePath,
	sessionsPath,
	socketPath,
	statePath,
	workspaceEnvPath,
} from "../paths.js";

const ENV_KEYS = ["QUEOHOH_STATE_DIR", "QUEOHOH_CONFIG", "QUEOHOH_WORKSPACE"];
// Clear before each test too — ambient shell exports (e.g. path.zsh) must not
// leak into the "no env is set" case; afterEach alone leaves the first test
// polluted when the suite inherits process.env.
function clearEnv(): void {
	for (const k of ENV_KEYS) delete process.env[k];
}
beforeEach(clearEnv);
afterEach(clearEnv);

describe("paths", () => {
	it("defaults to XDG-ish locations when no env is set", () => {
		expect(statePath()).toBe(join(homedir(), ".local/state/queohoh"));
		expect(configPath()).toBe(join(homedir(), ".config/queohoh/config.yaml"));
		expect(workspaceEnvPath()).toBeNull();
	});

	it("respects QUEOHOH_STATE_DIR and QUEOHOH_CONFIG overrides", () => {
		process.env.QUEOHOH_STATE_DIR = "/tmp/qo-state";
		process.env.QUEOHOH_CONFIG = "/tmp/qo.yaml";
		expect(statePath()).toBe("/tmp/qo-state");
		expect(configPath()).toBe("/tmp/qo.yaml");
	});

	it("resolves config to $QUEOHOH_WORKSPACE/config.yaml when workspace env is set", () => {
		process.env.QUEOHOH_WORKSPACE = "~/my-workspace";
		expect(workspaceEnvPath()).toBe(join(homedir(), "my-workspace"));
		expect(configPath()).toBe(join(homedir(), "my-workspace/config.yaml"));
	});

	it("QUEOHOH_CONFIG wins over QUEOHOH_WORKSPACE", () => {
		process.env.QUEOHOH_WORKSPACE = "/ws";
		process.env.QUEOHOH_CONFIG = "/explicit/config.yaml";
		expect(configPath()).toBe("/explicit/config.yaml");
	});

	it("derives daemon file paths from state", () => {
		expect(socketPath("/s")).toBe("/s/daemon/daemon.sock");
		expect(pidPath("/s")).toBe("/s/daemon/daemon.pid");
		expect(sessionsPath("/s")).toBe("/s/daemon/sessions.json");
		expect(sessionLineagePath("/s")).toBe("/s/daemon/session-lineage.json");
		expect(runsPath("/s")).toBe("/s/runs");
	});
});
