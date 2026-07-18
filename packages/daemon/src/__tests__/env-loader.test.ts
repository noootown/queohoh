// packages/daemon/src/__tests__/env-loader.test.ts
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildSecretMap } from "@queohoh/core";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { loadWorkspaceEnv } from "../env-loader.js";

let dir: string;
beforeEach(() => {
	dir = mkdtempSync(join(tmpdir(), "qoo-env-"));
});
afterEach(() => {
	rmSync(dir, { recursive: true, force: true });
});

describe("loadWorkspaceEnv", () => {
	it("loads keys from <workspace>/.env into the target env", () => {
		writeFileSync(join(dir, ".env"), "API_TOKEN=abc\nSERVICE_EMAIL=a@b.co\n");
		const env: NodeJS.ProcessEnv = {};
		const set = loadWorkspaceEnv(dir, env);
		expect(env.API_TOKEN).toBe("abc");
		expect(env.SERVICE_EMAIL).toBe("a@b.co");
		expect(set.sort()).toEqual(["API_TOKEN", "SERVICE_EMAIL"]);
	});

	it("does not overwrite a key already present in env (real env wins)", () => {
		writeFileSync(join(dir, ".env"), "API_TOKEN=fromfile\n");
		const env: NodeJS.ProcessEnv = { API_TOKEN: "fromenv" };
		const set = loadWorkspaceEnv(dir, env);
		expect(env.API_TOKEN).toBe("fromenv");
		expect(set).toEqual([]);
	});

	it("is a no-op when the file is missing", () => {
		const env: NodeJS.ProcessEnv = {};
		expect(loadWorkspaceEnv(dir, env)).toEqual([]);
		expect(Object.keys(env)).toEqual([]);
	});

	it("a loaded secret is picked up by buildSecretMap for redaction", () => {
		writeFileSync(join(dir, ".env"), "API_TOKEN=supersecret\n");
		const env: NodeJS.ProcessEnv = {};
		loadWorkspaceEnv(dir, env);
		const secrets = buildSecretMap(env);
		expect(secrets.has("supersecret")).toBe(true);
	});
});
