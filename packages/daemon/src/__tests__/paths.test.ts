import { homedir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
	configPath,
	pidPath,
	runsPath,
	sessionLineagePath,
	sessionsPath,
	socketPath,
	statePath,
} from "../paths.js";

const ENV_KEYS = ["QUEOHOH_STATE_DIR", "QUEOHOH_CONFIG"];
afterEach(() => {
	for (const k of ENV_KEYS) delete process.env[k];
});

describe("paths", () => {
	it("defaults to XDG-ish locations", () => {
		expect(statePath()).toBe(join(homedir(), ".local/state/queohoh"));
		expect(configPath()).toBe(join(homedir(), ".config/queohoh/config.yaml"));
	});

	it("respects env overrides", () => {
		process.env.QUEOHOH_STATE_DIR = "/tmp/qo-state";
		process.env.QUEOHOH_CONFIG = "/tmp/qo.yaml";
		expect(statePath()).toBe("/tmp/qo-state");
		expect(configPath()).toBe("/tmp/qo.yaml");
	});

	it("derives daemon file paths from state", () => {
		expect(socketPath("/s")).toBe("/s/daemon/daemon.sock");
		expect(pidPath("/s")).toBe("/s/daemon/daemon.pid");
		expect(sessionsPath("/s")).toBe("/s/daemon/sessions.json");
		expect(sessionLineagePath("/s")).toBe("/s/daemon/session-lineage.json");
		expect(runsPath("/s")).toBe("/s/runs");
	});
});
