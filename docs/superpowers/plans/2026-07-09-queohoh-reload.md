# `queohoh reload` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `queohoh reload` CLI command that rebuilds the checkout the CLI lives in and restarts the daemon on the fresh build, refusing by default when tasks are running.

**Architecture:** New `packages/daemon/src/reload.ts` with a pure decision core (`repoRootFromCli`, `busyVerdict`, `runReload` orchestrator over an injectable `ReloadSteps` interface — unit-tested with fakes) plus a `defaultReloadSteps` factory wiring real fs/spawn/launchctl/ApiClient side effects. `cli.ts` gains the `reload` command. Spec: `docs/superpowers/specs/2026-07-09-queohoh-reload-design.md`.

**Tech Stack:** TypeScript (ESM, strict), commander, vitest, pnpm workspace.

## Global Constraints

- ESM imports use explicit `.js` suffixes; tab indentation (biome); no new dependencies.
- Commits: conventional prefix, **no Co-Authored-By trailers**.
- Build failure must abort BEFORE any daemon is killed — never trade a working daemon for a broken build.
- The launchd branch must only be taken when `QUEOHOH_STATE_DIR` is unset: a launchd-managed daemon always runs on the default state dir, and honoring the override keeps hermetic testing from kicking the real daemon.
- Test commands: `pnpm -F @queohoh/daemon test`; single file `pnpm -F @queohoh/daemon exec vitest run src/__tests__/reload.test.ts`.

---

### Task 1: reload decision core + unit tests

**Files:**
- Create: `packages/daemon/src/reload.ts`
- Test: `packages/daemon/src/__tests__/reload.test.ts`

**Interfaces:**
- Consumes: nothing from this feature (node:path/node:fs only).
- Produces (Task 2 relies on these exact names):
  - `repoRootFromCli(cliPath: string, exists?: (p: string) => boolean): string | null`
  - `busyVerdict(running: string[] | null, force: boolean): { proceed: boolean; message: string | null }`
  - `interface ReloadSteps { repoRoot(): string | null; runningTasks(): Promise<string[] | null>; build(repoRoot: string): Promise<number>; restart(): Promise<void>; verify(): Promise<boolean>; logTail(): string }`
  - `runReload(opts: { force: boolean }, steps: ReloadSteps, log: { info(msg: string): void; error(msg: string): void }): Promise<number>` — returns the process exit code.

- [ ] **Step 1: Write the failing tests** — create `packages/daemon/src/__tests__/reload.test.ts`:

```ts
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
```

Note: the `build failure → restart never called` test asserts `calls` equals `[]` — to keep that assertion exact, `build` in that test's override does NOT push to `calls`; it only returns 2. (Write the override exactly as shown above.)

- [ ] **Step 2: Run tests, verify they fail**

Run: `pnpm -F @queohoh/daemon exec vitest run src/__tests__/reload.test.ts`
Expected: FAIL — `../reload.js` does not exist.

- [ ] **Step 3: Implement the decision core** — create `packages/daemon/src/reload.ts`:

```ts
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";

/**
 * Derive the repo root from the built CLI's own path
 * (<root>/packages/daemon/dist/cli.js → four segments up) and sanity-check
 * that a pnpm workspace lives there.
 */
export function repoRootFromCli(
	cliPath: string,
	exists: (p: string) => boolean = existsSync,
): string | null {
	const root = resolve(cliPath, "../../../..");
	return exists(join(root, "pnpm-workspace.yaml")) ? root : null;
}

export interface BusyVerdict {
	proceed: boolean;
	message: string | null;
}

/** Decide whether reload may proceed given the daemon's running set. */
export function busyVerdict(
	running: string[] | null,
	force: boolean,
): BusyVerdict {
	if (running === null) {
		return {
			proceed: true,
			message: "daemon not reachable — nothing running",
		};
	}
	if (running.length === 0) return { proceed: true, message: null };
	if (force) {
		return {
			proceed: true,
			message: `--force: restarting with ${running.length} running task(s) (${running.join(", ")}) — they will be marked failed by the orphan sweep`,
		};
	}
	return {
		proceed: false,
		message: `refusing to reload: ${running.length} task(s) running (${running.join(", ")}). Re-run with --force to restart anyway.`,
	};
}

export interface ReloadSteps {
	repoRoot: () => string | null;
	runningTasks: () => Promise<string[] | null>;
	build: (repoRoot: string) => Promise<number>;
	restart: () => Promise<void>;
	verify: () => Promise<boolean>;
	logTail: () => string;
}

export interface ReloadLog {
	info: (msg: string) => void;
	error: (msg: string) => void;
}

/**
 * Orchestrate reload: locate root → busy guard → build → restart → verify.
 * Build failures abort before any daemon is touched. Returns the exit code.
 */
export async function runReload(
	opts: { force: boolean },
	steps: ReloadSteps,
	log: ReloadLog,
): Promise<number> {
	const root = steps.repoRoot();
	if (root === null) {
		log.error(
			"cannot locate repo root (no pnpm-workspace.yaml above the CLI) — is the binary inside a checkout?",
		);
		return 1;
	}

	const verdict = busyVerdict(await steps.runningTasks(), opts.force);
	if (verdict.message !== null) {
		(verdict.proceed ? log.info : log.error)(verdict.message);
	}
	if (!verdict.proceed) return 1;

	log.info(`building ${root}`);
	const buildExit = await steps.build(root);
	if (buildExit !== 0) {
		log.error(`build failed (exit ${buildExit}) — daemon left untouched`);
		return 1;
	}

	await steps.restart();
	if (!(await steps.verify())) {
		log.error("daemon did not become reachable after restart; last log lines:");
		log.error(steps.logTail());
		return 1;
	}
	log.info("daemon reloaded");
	return 0;
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `pnpm -F @queohoh/daemon test`
Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/reload.ts packages/daemon/src/__tests__/reload.test.ts
git commit -m "feat(daemon): reload decision core (busy guard, build-first ordering)"
```

---

### Task 2: real steps factory + CLI wiring + docs

**Files:**
- Modify: `packages/daemon/src/reload.ts` (append `defaultReloadSteps`)
- Modify: `packages/daemon/src/cli.ts` (add `reload` command)
- Modify: `docs/setup.md` (mention reload in section 3)

**Interfaces:**
- Consumes: `runReload`/`ReloadSteps` (Task 1); `ApiClient` from `./client.js`; `pidPath`, `socketPath`, `statePath` from `./paths.js`.
- Produces: `defaultReloadSteps(cliPath: string): ReloadSteps`; CLI command `queohoh reload [--force]`.

- [ ] **Step 1: Append the real steps factory to `packages/daemon/src/reload.ts`**

Extend the imports at the top of the file:

```ts
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, openSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { ApiClient } from "./client.js";
import { pidPath, socketPath, statePath } from "./paths.js";
```

Append at the end of the file:

```ts
const LAUNCHD_LABEL = "com.queohoh.daemon";

function pidAlive(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch {
		return false;
	}
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Run a command to completion with no output; resolve with its exit code. */
function execCode(cmd: string, args: string[]): Promise<number> {
	return new Promise((res) => {
		const child = spawn(cmd, args, { stdio: "ignore" });
		child.on("close", (code) => res(code ?? 1));
		child.on("error", () => res(1));
	});
}

export function defaultReloadSteps(cliPath: string): ReloadSteps {
	const state = statePath();
	const sock = socketPath(state);
	const logPath = join(state, "daemon/daemon.log");

	return {
		repoRoot: () => repoRootFromCli(cliPath),

		runningTasks: async () => {
			const client = new ApiClient();
			try {
				await client.connect(sock);
				const s = (await client.call("state")) as { running: string[] };
				return s.running;
			} catch {
				return null;
			} finally {
				client.close();
			}
		},

		build: (root) =>
			new Promise((res) => {
				const child = spawn("pnpm", ["-r", "build"], {
					cwd: root,
					stdio: "inherit",
				});
				child.on("close", (code) => res(code ?? 1));
				child.on("error", () => res(1));
			}),

		restart: async () => {
			// launchd governs only the default state dir; with QUEOHOH_STATE_DIR
			// overridden (hermetic tests, alt deployments) always take the
			// pidfile path so we never kick a daemon we aren't talking to.
			if (process.env.QUEOHOH_STATE_DIR === undefined) {
				const uid = process.getuid?.() ?? 0;
				const target = `gui/${uid}/${LAUNCHD_LABEL}`;
				if ((await execCode("launchctl", ["print", target])) === 0) {
					await execCode("launchctl", ["kickstart", "-k", target]);
					return;
				}
			}
			let pid: number | null = null;
			try {
				pid = Number.parseInt(
					readFileSync(pidPath(state), "utf-8").trim(),
					10,
				);
			} catch {}
			if (pid !== null && Number.isFinite(pid) && pidAlive(pid)) {
				process.kill(pid, "SIGTERM");
				for (let i = 0; i < 10 && pidAlive(pid); i++) await sleep(500);
				if (pidAlive(pid)) process.kill(pid, "SIGKILL");
			}
			mkdirSync(join(state, "daemon"), { recursive: true });
			const logFd = openSync(logPath, "a");
			spawn(process.execPath, [cliPath, "daemon"], {
				detached: true,
				stdio: ["ignore", logFd, logFd],
			}).unref();
		},

		verify: async () => {
			for (let i = 0; i < 10; i++) {
				const client = new ApiClient();
				try {
					await client.connect(sock);
					if ((await client.call("ping")) === "pong") return true;
				} catch {
				} finally {
					client.close();
				}
				await sleep(500);
			}
			return false;
		},

		logTail: () => {
			try {
				return readFileSync(logPath, "utf-8").split("\n").slice(-20).join("\n");
			} catch {
				return "(no daemon log)";
			}
		},
	};
}
```

- [ ] **Step 2: Wire the CLI command** — in `packages/daemon/src/cli.ts`, add to the imports:

```ts
import { defaultReloadSteps, runReload } from "./reload.js";
```

and add after the `status` command registration:

```ts
program
	.command("reload")
	.description(
		"rebuild this checkout and restart the daemon on the fresh build",
	)
	.option(
		"--force",
		"restart even if tasks are running (they will be marked failed)",
		false,
	)
	.action(async (opts: { force: boolean }) => {
		const cliPath = fileURLToPath(import.meta.url);
		process.exitCode = await runReload(
			{ force: opts.force },
			defaultReloadSteps(cliPath),
			{ info: console.log, error: console.error },
		);
	});
```

(`fileURLToPath` is already imported in cli.ts.)

- [ ] **Step 3: Update `docs/setup.md`** — in section "## 3. Run the daemon", extend the code block:

```bash
queohoh daemon              # foreground (first run writes a starter config)
queohoh launchd:install     # keep-alive via launchd (prints the bootstrap command)
queohoh status              # check it's up
queohoh reload              # after changing daemon code: rebuild + restart
                            # (refuses if tasks are running; --force overrides)
```

- [ ] **Step 4: Build, typecheck, full daemon suite**

Run: `pnpm -F @queohoh/daemon build && pnpm -F @queohoh/daemon test && pnpm -r typecheck`
Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/reload.ts packages/daemon/src/cli.ts docs/setup.md
git commit -m "feat(daemon): queohoh reload command (rebuild + restart with busy guard)"
```

---

### Task 3: hermetic end-to-end verification

**Files:** none — verification only, no commits.

The point: exercise the real `defaultReloadSteps` against a throwaway state dir, never touching `~/.local/state/queohoh` or launchd (the `QUEOHOH_STATE_DIR` override forces the pidfile branch by design).

- [ ] **Step 1: Fresh-start path** (no daemon → build, start detached, verify)

```bash
cd /Users/noootown/Downloads/agent247/queohoh.qoo-skill
export QUEOHOH_STATE_DIR=$(mktemp -d)/state
export QUEOHOH_CONFIG=$(mktemp -d)/config.yaml
node packages/daemon/dist/cli.js reload; echo "exit=$?"
```

Expected: build output streams, then `daemon reloaded`, `exit=0`.

- [ ] **Step 2: Daemon reachable + restart path** (running daemon → new pid after reload)

```bash
OLD_PID=$(cat "$QUEOHOH_STATE_DIR/daemon/daemon.pid")
node packages/daemon/dist/cli.js status >/dev/null && echo reachable
node packages/daemon/dist/cli.js reload; echo "exit=$?"
NEW_PID=$(cat "$QUEOHOH_STATE_DIR/daemon/daemon.pid")
echo "old=$OLD_PID new=$NEW_PID"
```

Expected: `reachable`, `exit=0`, and `NEW_PID != OLD_PID`.

- [ ] **Step 3: Cleanup**

```bash
kill "$(cat "$QUEOHOH_STATE_DIR/daemon/daemon.pid")" 2>/dev/null || true
unset QUEOHOH_STATE_DIR QUEOHOH_CONFIG
```

- [ ] **Step 4: Report** — exit codes observed, pid rotation confirmed, any surprises (e.g. the busy path can't be hermetically exercised without a fake long task — that's covered by unit tests).
