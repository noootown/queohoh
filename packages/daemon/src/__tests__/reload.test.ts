import { describe, expect, it } from "vitest";
import type { ReloadSteps } from "../reload.js";
import { busyVerdict, repoRootFromCli, runReload } from "../reload.js";

const silentLog = { info: () => {}, error: () => {} };

function makeSteps(overrides: Partial<ReloadSteps> = {}) {
	const calls: string[] = [];
	const steps: ReloadSteps = {
		repoRoot: () => "/repo",
		runningTasks: async () => [],
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

describe("busyVerdict", () => {
	it("idle → proceed with no message", () => {
		expect(busyVerdict([], false)).toEqual({ proceed: true, message: null });
	});

	it("daemon unreachable → proceed with an informational message", () => {
		const v = busyVerdict(null, false);
		expect(v.proceed).toBe(true);
		expect(v.message).toContain("not reachable");
	});

	it("busy without force → refuse, message names the task ids and --force", () => {
		const v = busyVerdict(["01A", "01B"], false);
		expect(v.proceed).toBe(false);
		expect(v.message).toContain("01A, 01B");
		expect(v.message).toContain("--force");
	});

	it("busy with force → proceed, message warns about the orphan sweep", () => {
		const v = busyVerdict(["01A"], true);
		expect(v.proceed).toBe(true);
		expect(v.message).toContain("orphan sweep");
	});
});

describe("runReload", () => {
	it("no repo root → exit 1, nothing else runs", async () => {
		const { steps, calls } = makeSteps({ repoRoot: () => null });
		expect(await runReload({ force: false }, steps, silentLog)).toBe(1);
		expect(calls).toEqual([]);
	});

	it("busy without force → exit 1, build and restart never called", async () => {
		const { steps, calls } = makeSteps({ runningTasks: async () => ["01A"] });
		expect(await runReload({ force: false }, steps, silentLog)).toBe(1);
		expect(calls).toEqual([]);
	});

	it("build failure → exit 1, restart never called", async () => {
		const { steps, calls } = makeSteps({
			build: async () => 2,
		});
		expect(await runReload({ force: false }, steps, silentLog)).toBe(1);
		expect(calls).toEqual([]);
	});

	it("happy path → build, restart, verify in order, exit 0", async () => {
		const { steps, calls } = makeSteps();
		expect(await runReload({ force: false }, steps, silentLog)).toBe(0);
		expect(calls).toEqual(["build", "restart"]);
	});

	it("task starts during the build → exit 1, restart never called", async () => {
		let checks = 0;
		const { steps, calls } = makeSteps({
			runningTasks: async () => (checks++ === 0 ? [] : ["01A"]),
		});
		expect(await runReload({ force: false }, steps, silentLog)).toBe(1);
		expect(calls).toEqual(["build"]);
	});

	it("task starts during the build but --force → still restarts", async () => {
		let checks = 0;
		const { steps, calls } = makeSteps({
			runningTasks: async () => (checks++ === 0 ? [] : ["01A"]),
		});
		expect(await runReload({ force: true }, steps, silentLog)).toBe(0);
		expect(calls).toEqual(["build", "restart"]);
	});

	it("busy with force → proceeds to build and restart", async () => {
		const { steps, calls } = makeSteps({ runningTasks: async () => ["01A"] });
		expect(await runReload({ force: true }, steps, silentLog)).toBe(0);
		expect(calls).toEqual(["build", "restart"]);
	});

	it("verify failure → exit 1 and the daemon log tail is surfaced", async () => {
		const errors: string[] = [];
		const { steps } = makeSteps({
			verify: async () => false,
			logTail: () => "boom line",
		});
		const code = await runReload({ force: false }, steps, {
			info: () => {},
			error: (m) => errors.push(m),
		});
		expect(code).toBe(1);
		expect(errors.join("\n")).toContain("boom line");
	});
});
