import { describe, expect, it } from "vitest";
import type { ReloadSteps } from "../reload.js";
import { repoRootFromCli, runReload } from "../reload.js";

const silentLog = { info: () => {}, error: () => {} };

function makeSteps(overrides: Partial<ReloadSteps> = {}) {
	const calls: string[] = [];
	const steps: ReloadSteps = {
		repoRoot: () => "/repo",
		build: async () => {
			calls.push("build");
			return 0;
		},
		restart: async () => {
			calls.push("restart");
		},
		verify: async () => true,
		logTail: () => "",
		...overrides,
	};
	return { steps, calls };
}

describe("repoRootFromCli", () => {
	it("resolves four segments up from dist/cli.js when the workspace marker exists", () => {
		const root = repoRootFromCli(
			"/repo/packages/daemon/dist/cli.js",
			(p) => p === "/repo/pnpm-workspace.yaml",
		);
		expect(root).toBe("/repo");
	});

	it("returns null when pnpm-workspace.yaml is missing at the derived root", () => {
		expect(
			repoRootFromCli("/elsewhere/packages/daemon/dist/cli.js", () => false),
		).toBeNull();
	});
});

describe("runReload", () => {
	it("no repo root → exit 1, nothing else runs", async () => {
		const { steps, calls } = makeSteps({ repoRoot: () => null });
		expect(await runReload(steps, silentLog)).toBe(1);
		expect(calls).toEqual([]);
	});

	it("build failure → exit 1, restart never called", async () => {
		const { steps, calls } = makeSteps({
			build: async () => 2,
		});
		expect(await runReload(steps, silentLog)).toBe(1);
		expect(calls).toEqual([]);
	});

	it("happy path → build, restart, verify in order, exit 0", async () => {
		const { steps, calls } = makeSteps();
		expect(await runReload(steps, silentLog)).toBe(0);
		expect(calls).toEqual(["build", "restart"]);
	});

	it("verify failure → exit 1 and the daemon log tail is surfaced", async () => {
		const errors: string[] = [];
		const { steps } = makeSteps({
			verify: async () => false,
			logTail: () => "boom line",
		});
		const code = await runReload(steps, {
			info: () => {},
			error: (m) => errors.push(m),
		});
		expect(code).toBe(1);
		expect(errors.join("\n")).toContain("boom line");
	});
});
