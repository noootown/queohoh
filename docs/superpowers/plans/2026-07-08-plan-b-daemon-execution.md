# queohoh Plan B — Daemon & Execution Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make queued tasks actually run: discovery/dedup instantiation, the Claude runner with full run observability (events, transcript, report, cost, snapshot — ported from agent247), the end-to-end worker with hooks and the completion contract, session registry, and the daemon (tick loop, orphan sweep, unix-socket API) with launchd keep-alive.

**Architecture:** Library code (redaction, discovery, dedup, instantiation, runner, run store, hooks, worker, session registry) lands in `packages/core` with injected I/O so it stays unit-testable; `packages/daemon` is the thin long-running shell: tick loop + socket server + CLI. Files remain source of truth; the daemon rehydrates from disk on restart.

**Tech Stack:** TypeScript strict ESM, Node >= 22, pnpm, vitest, zod v4, js-yaml, ulid. No new runtime deps except `commander` (daemon CLI).

**Spec:** `docs/superpowers/specs/2026-07-08-queohoh-slice1-design.md` · **Builds on:** Plan A (`@queohoh/core` — QueueStore, schedule, resolveTarget, createResolverIO, loadDefinition, render, laneKey…)

## Global Constraints

- Node >= 22, `"type": "module"`, TS strict. Run lint fixes via `mise x node@22 -- pnpm lint` (write-mode biome dies under Node 25).
- All zod schemas on user-authored files are `.strict()`.
- Discovery items are flat `Record<string, string>` — **stringify every value** before storing on a task (`String(v)`).
- `resolveTarget` is PARTIAL: `io.spawnWorktree` rejections propagate — every daemon-side call site must catch and map to task `failed`/`needs-input`.
- Var precedence (low → high): global vars → repo vars → item vars → reserved vars. Template syntax `{{key}}`.
- Runner: `claude -p <prompt> --output-format stream-json --verbose --model <model>`, detached process group, SIGTERM on timeout then SIGKILL after 5s. Timeouts floor at 1000ms.
- Everything persisted under `runs/` passes through the redactor before hitting disk.
- Run dir layout per task: `runs/<task-id>/{data.json,prompt.rendered.md,transcript.md,events.jsonl,report.md,worker.json}`.
- Completion contract (`done` requires ALL): exit code 0, not timed out, worktree `git status --porcelain` empty after run. Otherwise `failed` with reason in `task.error`.
- `post_run` hook always runs (finally semantics); its failure is logged, never fatal. `pre_run` failure → `failed`, Claude never spawns.
- Daemon paths: state dir `~/.local/state/queohoh` (env override `QUEOHOH_STATE_DIR`), config `~/.config/queohoh/config.yaml` (env override `QUEOHOH_CONFIG`), socket `<state>/daemon/daemon.sock`, pidfile `<state>/daemon/daemon.pid`.
- Socket protocol: newline-delimited JSON. Request `{id, method, params?}` → `{id, result}` or `{id, error}`. `subscribe` marks the connection to receive `{event: "state", data}` pushes on every state change.
- Interactive sessions expire after 300s without heartbeat; worker sessions expire when their pid is dead.
- Commit after every green test cycle. No Co-Authored-By trailers.

---

### Task 1: Redaction (agent247 port)

**Files:**
- Create: `packages/core/src/redact.ts`
- Test: `packages/core/src/__tests__/redact.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `buildSecretMap(envEntries: Record<string, string | undefined>): Map<string, string>` (ALL_CAPS keys only, empty/undefined values skipped); `redact(text: string, secrets: Map<string, string>): string` (longer values replaced first, `[REDACTED:KEY_NAME]`); `makeRedactor(secrets: Map<string, string>): (s: string) => string`.

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/redact.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { buildSecretMap, makeRedactor, redact } from "../redact.js";

describe("buildSecretMap", () => {
	it("collects ALL_CAPS keys with values, skips others", () => {
		const map = buildSecretMap({
			GITHUB_TOKEN: "ghp_abc123",
			lower_key: "notsecret",
			EMPTY: "",
			MISSING: undefined,
			_UNDERSCORE_OK: "val",
		});
		expect(map.get("ghp_abc123")).toBe("GITHUB_TOKEN");
		expect(map.get("val")).toBe("_UNDERSCORE_OK");
		expect(map.has("notsecret")).toBe(false);
		expect(map.size).toBe(2);
	});
});

describe("redact", () => {
	it("replaces longer values first", () => {
		const secrets = new Map([
			["abc", "SHORT"],
			["abc123", "LONG"],
		]);
		expect(redact("token abc123 and abc", secrets)).toBe(
			"token [REDACTED:LONG] and [REDACTED:SHORT]",
		);
	});

	it("no-ops on empty map", () => {
		expect(redact("hello", new Map())).toBe("hello");
	});
});

describe("makeRedactor", () => {
	it("returns a bound redact function", () => {
		const r = makeRedactor(new Map([["sekrit", "KEY"]]));
		expect(r("say sekrit")).toBe("say [REDACTED:KEY]");
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test`
Expected: FAIL — cannot find module `../redact.js`.

- [ ] **Step 3: Implement**

`packages/core/src/redact.ts`:

```ts
/** Build a Map<secretValue, keyName> from ALL_CAPS keys only. */
export function buildSecretMap(
	envEntries: Record<string, string | undefined>,
): Map<string, string> {
	const map = new Map<string, string>();
	for (const [key, value] of Object.entries(envEntries)) {
		if (!value) continue;
		if (/^[A-Z_][A-Z0-9_]*$/.test(key)) {
			map.set(value, key);
		}
	}
	return map;
}

/** Replace secret values with [REDACTED:KEY_NAME]. Longer values replaced first. */
export function redact(text: string, secrets: Map<string, string>): string {
	if (secrets.size === 0) return text;
	const sorted = [...secrets.entries()].sort(
		(a, b) => b[0].length - a[0].length,
	);
	let result = text;
	for (const [value, key] of sorted) {
		result = result.replaceAll(value, `[REDACTED:${key}]`);
	}
	return result;
}

export type Redactor = (s: string) => string;

export function makeRedactor(secrets: Map<string, string>): Redactor {
	return (s) => redact(s, secrets);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/redact.ts packages/core/src/__tests__/redact.test.ts
git commit -m "feat(core): redaction utilities (agent247 port)"
```

---

### Task 2: Discovery, archive listing, dedup

**Files:**
- Create: `packages/core/src/discovery.ts`, `packages/core/src/dedup.ts`
- Modify: `packages/core/src/store.ts` (add `listArchived()`)
- Test: `packages/core/src/__tests__/discovery.test.ts`, `packages/core/src/__tests__/dedup.test.ts`, extend `packages/core/src/__tests__/store.test.ts`

**Interfaces:**
- Consumes: `Exec` (Plan A Task 11), `TaskInstance`, `QueueStore`.
- Produces:
  - `discoverItems(command: string, exec: Exec, opts: { cwd: string }): Promise<Record<string, string>[]>` — runs via `exec("/bin/bash", ["-lc", command], {cwd})`, parses stdout as JSON array of objects, **stringifies every value**, throws `Error("discovery command failed (exit N)")` on nonzero exit and `Error("discovery command must return a JSON array")` on non-array.
  - `QueueStore.listArchived(): TaskInstance[]` — same contract as `list()` but over `archive/` (junk-tolerant via `invalidFiles` NOT shared — archived junk is ignored silently).
  - `type DedupMode = "skip_seen" | "retry_errored" | "none"` and `filterNewItems(items, opts: { definition: string; itemKeyTemplate: string; mode: DedupMode; existing: TaskInstance[] }): { item: Record<string, string>; itemKey: string }[]` — itemKey = `render(itemKeyTemplate, {}, {}, item)`; `skip_seen` drops keys with ANY existing instance of the same definition; `retry_errored` drops keys whose existing same-definition instances include a non-`failed` status (failed-only keys are retried); `none` keeps all but still computes keys. Items whose rendered key still contains `{{` throw `Error("item_key did not resolve: <key>")`.

- [ ] **Step 1: Write the failing discovery test**

`packages/core/src/__tests__/discovery.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { discoverItems } from "../discovery.js";
import type { Exec } from "../resolver-io.js";

function execReturning(stdout: string, exitCode = 0): Exec {
	return async () => ({ stdout, exitCode });
}

describe("discoverItems", () => {
	it("parses a JSON array and stringifies values", async () => {
		const exec = execReturning('[{"number": 1423, "title": "fix auth"}]');
		const items = await discoverItems("gh pr list --json number,title", exec, {
			cwd: "/repo",
		});
		expect(items).toEqual([{ number: "1423", title: "fix auth" }]);
	});

	it("throws on nonzero exit", async () => {
		const exec = execReturning("", 1);
		await expect(discoverItems("boom", exec, { cwd: "/repo" })).rejects.toThrow(
			"discovery command failed (exit 1)",
		);
	});

	it("throws on non-array JSON", async () => {
		const exec = execReturning('{"not": "array"}');
		await expect(discoverItems("x", exec, { cwd: "/repo" })).rejects.toThrow(
			"discovery command must return a JSON array",
		);
	});

	it("passes the command through bash -lc with cwd", async () => {
		let seen: { command: string; args: string[]; cwd: string } | null = null;
		const exec: Exec = async (command, args, opts) => {
			seen = { command, args, cwd: opts.cwd };
			return { stdout: "[]", exitCode: 0 };
		};
		await discoverItems("echo '[]'", exec, { cwd: "/repo" });
		expect(seen).toEqual({
			command: "/bin/bash",
			args: ["-lc", "echo '[]'"],
			cwd: "/repo",
		});
	});
});
```

- [ ] **Step 2: Run to verify failure, then implement discovery**

Run: `pnpm -F @queohoh/core test` — FAIL (module not found). Then create `packages/core/src/discovery.ts`:

```ts
import type { Exec } from "./resolver-io.js";

export async function discoverItems(
	command: string,
	exec: Exec,
	opts: { cwd: string },
): Promise<Record<string, string>[]> {
	const { stdout, exitCode } = await exec("/bin/bash", ["-lc", command], {
		cwd: opts.cwd,
	});
	if (exitCode !== 0) {
		throw new Error(`discovery command failed (exit ${exitCode})`);
	}
	const parsed: unknown = JSON.parse(stdout.trim());
	if (!Array.isArray(parsed)) {
		throw new Error("discovery command must return a JSON array");
	}
	return parsed.map((raw) => {
		const item: Record<string, string> = {};
		for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
			item[k] = String(v);
		}
		return item;
	});
}
```

Run: `pnpm -F @queohoh/core test` — discovery tests PASS.

- [ ] **Step 3: Add listArchived to QueueStore (test first)**

Append to `packages/core/src/__tests__/store.test.ts`:

```ts
	it("listArchived returns archived tasks", () => {
		const store = freshStore();
		const t = store.create({ prompt: "x", repo: "r", ref: "temp", source: "tui" });
		store.archive(t.id);
		expect(store.listArchived().map((a) => a.id)).toEqual([t.id]);
	});
```

Run to see it fail, then add to `QueueStore` (in `packages/core/src/store.ts`, after `archive`):

```ts
	listArchived(): TaskInstance[] {
		const tasks: TaskInstance[] = [];
		for (const file of readdirSync(this.archiveDir).sort()) {
			if (!file.endsWith(".md")) continue;
			try {
				tasks.push(
					parseTaskFile(readFileSync(join(this.archiveDir, file), "utf-8")),
				);
			} catch {
				// archived junk is ignored silently
			}
		}
		return tasks.sort((a, b) => a.id.localeCompare(b.id));
	}
```

Run: store tests PASS.

- [ ] **Step 4: Write the failing dedup test**

`packages/core/src/__tests__/dedup.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { filterNewItems } from "../dedup.js";
import type { TaskInstance, TaskStatus } from "../task.js";

let seq = 0;
function existing(status: TaskStatus, itemKey: string, definition = "platform/pr-review"): TaskInstance {
	seq += 1;
	return {
		id: `01DEDUP${String(seq).padStart(19, "0")}`,
		status,
		definition,
		item: { number: itemKey },
		itemKey,
		target: { repo: "platform", ref: `pr:${itemKey}`, worktree: null },
		priority: "normal",
		created: "2026-07-08T00:00:00.000Z",
		source: "cron",
		ephemeralWorktree: false,
		error: null,
		prompt: "p",
	};
}

const items = [{ number: "1" }, { number: "2" }, { number: "3" }];
const base = { definition: "platform/pr-review", itemKeyTemplate: "{{number}}" };

describe("filterNewItems", () => {
	it("skip_seen drops keys with any existing instance", () => {
		const out = filterNewItems(items, {
			...base,
			mode: "skip_seen",
			existing: [existing("done", "1"), existing("failed", "2")],
		});
		expect(out).toEqual([{ item: { number: "3" }, itemKey: "3" }]);
	});

	it("retry_errored retries failed-only keys", () => {
		const out = filterNewItems(items, {
			...base,
			mode: "retry_errored",
			existing: [existing("done", "1"), existing("failed", "2")],
		});
		expect(out.map((o) => o.itemKey)).toEqual(["2", "3"]);
	});

	it("retry_errored does not retry a key that also has a live instance", () => {
		const out = filterNewItems([{ number: "1" }], {
			...base,
			mode: "retry_errored",
			existing: [existing("failed", "1"), existing("queued", "1")],
		});
		expect(out).toEqual([]);
	});

	it("none keeps everything with keys", () => {
		const out = filterNewItems([{ number: "9" }], {
			...base,
			mode: "none",
			existing: [existing("done", "9")],
		});
		expect(out).toEqual([{ item: { number: "9" }, itemKey: "9" }]);
	});

	it("only same-definition instances count", () => {
		const out = filterNewItems([{ number: "1" }], {
			...base,
			mode: "skip_seen",
			existing: [existing("done", "1", "platform/other-task")],
		});
		expect(out.map((o) => o.itemKey)).toEqual(["1"]);
	});

	it("throws when item_key does not resolve", () => {
		expect(() =>
			filterNewItems([{ number: "1" }], {
				definition: "d",
				itemKeyTemplate: "{{missing}}",
				mode: "none",
				existing: [],
			}),
		).toThrow("item_key did not resolve: {{missing}}");
	});
});
```

- [ ] **Step 5: Run to verify failure, then implement dedup**

`packages/core/src/dedup.ts`:

```ts
import type { TaskInstance } from "./task.js";
import { render } from "./template.js";

export type DedupMode = "skip_seen" | "retry_errored" | "none";

export interface KeyedItem {
	item: Record<string, string>;
	itemKey: string;
}

export function filterNewItems(
	items: Record<string, string>[],
	opts: {
		definition: string;
		itemKeyTemplate: string;
		mode: DedupMode;
		existing: TaskInstance[];
	},
): KeyedItem[] {
	const keyed = items.map((item) => {
		const itemKey = render(opts.itemKeyTemplate, {}, {}, item);
		if (itemKey.includes("{{")) {
			throw new Error(`item_key did not resolve: ${itemKey}`);
		}
		return { item, itemKey };
	});
	if (opts.mode === "none") return keyed;

	const sameDef = opts.existing.filter((t) => t.definition === opts.definition);
	const seen = new Set(
		sameDef.filter((t) => t.itemKey !== null).map((t) => t.itemKey as string),
	);
	const retryable = new Set<string>();
	if (opts.mode === "retry_errored") {
		for (const key of seen) {
			const forKey = sameDef.filter((t) => t.itemKey === key);
			if (forKey.every((t) => t.status === "failed")) retryable.add(key);
		}
	}
	return keyed.filter(
		({ itemKey }) => !seen.has(itemKey) || retryable.has(itemKey),
	);
}
```

Run: `pnpm -F @queohoh/core test` — all PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/core/src/discovery.ts packages/core/src/dedup.ts packages/core/src/store.ts packages/core/src/__tests__/
git commit -m "feat(core): discovery, archive listing, and dedup"
```

---

### Task 3: Instantiation pipeline

**Files:**
- Create: `packages/core/src/instantiate.ts`
- Test: `packages/core/src/__tests__/instantiate.test.ts`

**Interfaces:**
- Consumes: `TaskDefinition`, `QueueStore`, `discoverItems`, `filterNewItems`, `render`, `Exec`.
- Produces:
  - `type Trigger = { mode: "discover" } | { mode: "args"; values: string[] }`
  - `instantiateDefinition(def: TaskDefinition, trigger: Trigger, deps: { store: QueueStore; exec: Exec; repoPath: string; source: TaskSource; globalVars?: Record<string, string>; repoVars?: Record<string, string> }): Promise<TaskInstance[]>`
  - Behavior: `discover` mode requires `def.discovery` (else `Error("definition <name> has no discovery")`); `args` mode zips `trigger.values` onto `def.args` names (length mismatch → `Error("expected N args (<names>), got M")`). Items flow through `filterNewItems` (existing = live + archived). Each surviving item creates an instance: `definition: "<repo>/<name>"`, rendered prompt (`render(def.prompt, globalVars, repoVars, item)`), rendered worktree ref (`render(def.worktree, globalVars, repoVars, item)`), `priority: def.priority`.

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/instantiate.test.ts`:

```ts
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { TaskDefinition } from "../definition.js";
import { instantiateDefinition } from "../instantiate.js";
import type { Exec } from "../resolver-io.js";
import { QueueStore } from "../store.js";

function def(overrides: Partial<TaskDefinition> = {}): TaskDefinition {
	return {
		name: "pr-review",
		repo: "platform",
		discovery: { command: "gh pr list", itemKey: "{{number}}" },
		args: ["number"],
		dedup: "skip_seen",
		worktree: "pr:{{number}}",
		preRun: null,
		postRun: null,
		model: "opus",
		timeoutMs: 1_800_000,
		priority: "high",
		prompt: "Review PR {{number}} for {{github_user}}.\n",
		...overrides,
	};
}

function deps(store: QueueStore, stdout: string) {
	const exec: Exec = async () => ({ stdout, exitCode: 0 });
	return {
		store,
		exec,
		repoPath: "/repo",
		source: "cron" as const,
		globalVars: { github_user: "noootown" },
	};
}

const freshStore = () => new QueueStore(mkdtempSync(join(tmpdir(), "qo-inst-")));

describe("instantiateDefinition — discover", () => {
	it("creates one instance per discovered item with rendered fields", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def(),
			{ mode: "discover" },
			deps(store, '[{"number": 257}, {"number": 258}]'),
		);
		expect(created).toHaveLength(2);
		const first = created[0];
		expect(first?.definition).toBe("platform/pr-review");
		expect(first?.item).toEqual({ number: "257" });
		expect(first?.itemKey).toBe("257");
		expect(first?.target).toEqual({ repo: "platform", ref: "pr:257", worktree: null });
		expect(first?.priority).toBe("high");
		expect(first?.prompt).toBe("Review PR 257 for noootown.\n");
		expect(store.list()).toHaveLength(2);
	});

	it("dedups against existing instances", async () => {
		const store = freshStore();
		await instantiateDefinition(def(), { mode: "discover" }, deps(store, '[{"number": 257}]'));
		const second = await instantiateDefinition(
			def(),
			{ mode: "discover" },
			deps(store, '[{"number": 257}, {"number": 300}]'),
		);
		expect(second.map((t) => t.itemKey)).toEqual(["300"]);
	});

	it("dedups against archived instances too", async () => {
		const store = freshStore();
		const [made] = await instantiateDefinition(def(), { mode: "discover" }, deps(store, '[{"number": 257}]'));
		store.archive((made as { id: string }).id);
		const again = await instantiateDefinition(def(), { mode: "discover" }, deps(store, '[{"number": 257}]'));
		expect(again).toEqual([]);
	});

	it("throws when definition has no discovery", async () => {
		const store = freshStore();
		await expect(
			instantiateDefinition(def({ discovery: null }), { mode: "discover" }, deps(store, "[]")),
		).rejects.toThrow("definition pr-review has no discovery");
	});
});

describe("instantiateDefinition — args", () => {
	it("zips values onto declared arg names, skipping discovery", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def(),
			{ mode: "args", values: ["257"] },
			deps(store, "SHOULD NOT RUN"),
		);
		expect(created).toHaveLength(1);
		expect(created[0]?.item).toEqual({ number: "257" });
		expect(created[0]?.target.ref).toBe("pr:257");
	});

	it("throws on arg count mismatch", async () => {
		const store = freshStore();
		await expect(
			instantiateDefinition(def(), { mode: "args", values: [] }, deps(store, "[]")),
		).rejects.toThrow("expected 1 args (number), got 0");
	});

	it("args mode still dedups", async () => {
		const store = freshStore();
		await instantiateDefinition(def(), { mode: "args", values: ["257"] }, deps(store, "[]"));
		const again = await instantiateDefinition(def(), { mode: "args", values: ["257"] }, deps(store, "[]"));
		expect(again).toEqual([]);
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test`
Expected: FAIL — cannot find module `../instantiate.js`.

- [ ] **Step 3: Implement**

`packages/core/src/instantiate.ts`:

```ts
import { filterNewItems } from "./dedup.js";
import type { TaskDefinition } from "./definition.js";
import { discoverItems } from "./discovery.js";
import type { Exec } from "./resolver-io.js";
import type { QueueStore } from "./store.js";
import type { TaskInstance, TaskSource } from "./task.js";
import { render } from "./template.js";

export type Trigger =
	| { mode: "discover" }
	| { mode: "args"; values: string[] };

export interface InstantiateDeps {
	store: QueueStore;
	exec: Exec;
	repoPath: string;
	source: TaskSource;
	globalVars?: Record<string, string>;
	repoVars?: Record<string, string>;
}

export async function instantiateDefinition(
	def: TaskDefinition,
	trigger: Trigger,
	deps: InstantiateDeps,
): Promise<TaskInstance[]> {
	const globalVars = deps.globalVars ?? {};
	const repoVars = deps.repoVars ?? {};

	let items: Record<string, string>[];
	if (trigger.mode === "discover") {
		if (!def.discovery) {
			throw new Error(`definition ${def.name} has no discovery`);
		}
		items = await discoverItems(def.discovery.command, deps.exec, {
			cwd: deps.repoPath,
		});
	} else {
		if (trigger.values.length !== def.args.length) {
			throw new Error(
				`expected ${def.args.length} args (${def.args.join(", ")}), got ${trigger.values.length}`,
			);
		}
		const item: Record<string, string> = {};
		def.args.forEach((name, i) => {
			item[name] = String(trigger.values[i]);
		});
		items = [item];
	}

	const definition = `${def.repo}/${def.name}`;
	const itemKeyTemplate = def.discovery?.itemKey ?? defaultKeyTemplate(def);
	const existing = [...deps.store.list(), ...deps.store.listArchived()];
	const fresh = filterNewItems(items, {
		definition,
		itemKeyTemplate,
		mode: def.dedup,
		existing,
	});

	return fresh.map(({ item, itemKey }) =>
		deps.store.create({
			prompt: render(def.prompt, globalVars, repoVars, item),
			repo: def.repo,
			ref: render(def.worktree, globalVars, repoVars, item),
			source: deps.source,
			priority: def.priority,
			definition,
			item,
			itemKey,
		}),
	);
}

/** Key template when a definition has no discovery block: join declared args. */
function defaultKeyTemplate(def: TaskDefinition): string {
	if (def.args.length === 0) return "adhoc";
	return def.args.map((a) => `{{${a}}}`).join(":");
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/instantiate.ts packages/core/src/__tests__/instantiate.test.ts
git commit -m "feat(core): definition instantiation pipeline (discover + args triggers)"
```

---

### Task 4: Claude runner (agent247 port)

**Files:**
- Create: `packages/core/src/runner.ts`
- Create: `packages/core/src/__tests__/fixtures/fake-claude.mjs` (executable test double)
- Test: `packages/core/src/__tests__/runner.test.ts`

**Interfaces:**
- Consumes: `Redactor` (Task 1).
- Produces:
  - `interface RunUsage { costUsd: number | null; turns: number | null; durationMs: number | null }`
  - `interface RunResult { exitCode: number; timedOut: boolean; sessionId: string | null; resultText: string; stderr: string; usage: RunUsage }`
  - `formatEventToMarkdown(event: Record<string, unknown>): string | null` — port of agent247's claude-branch formatter (assistant events → `### Thinking` / text / `### Tool: <name>` with Bash/Edit/Read/Write/Grep special-casing, 500-char JSON clamp).
  - `executeClaude(opts: { prompt: string; model: string; cwd: string; timeoutMs: number; claudeBin?: string; claudeArgs?: string[]; eventsPath: string; transcriptPath: string; redact: Redactor; onSpawned?: (pid: number) => void }): Promise<RunResult>` — spawns `claudeBin ?? "claude"` with `["-p", prompt, "--output-format", "stream-json", "--verbose", "--model", model, ...(claudeArgs ?? [])]`, detached; streams stdout lines: every parseable JSON line is appended (redacted) to `eventsPath`; markdown-formatted events appended (redacted) to `transcriptPath`; `result` event captures `resultText` (`event.result`), `usage` (`total_cost_usd`, `num_turns`, `duration_ms`), sessionId from first event bearing `session_id`. Timeout: `SIGTERM` to process group, `SIGKILL` after 5s, `timedOut: true`. Timeout floors at 1000ms. Spawn error resolves `{exitCode: 1, stderr: "Failed to spawn process", ...}` — never rejects.

- [ ] **Step 1: Create the fake claude fixture**

`packages/core/src/__tests__/fixtures/fake-claude.mjs` (make executable: `chmod +x`):

```js
#!/usr/bin/env node
// Fake `claude` for runner tests. Behavior selected by FAKE_CLAUDE_MODE env var.
const mode = process.env.FAKE_CLAUDE_MODE ?? "ok";

const emit = (obj) => process.stdout.write(`${JSON.stringify(obj)}\n`);

if (mode === "ok") {
	emit({ type: "system", session_id: "sess-123" });
	emit({
		type: "assistant",
		message: {
			content: [
				{ type: "text", text: "Working on it with TOKEN_VALUE_XYZ" },
				{ type: "tool_use", name: "Bash", input: { command: "echo hi" } },
			],
		},
	});
	emit({
		type: "result",
		result: "All done.",
		total_cost_usd: 0.42,
		num_turns: 3,
		duration_ms: 1234,
	});
	process.exit(0);
} else if (mode === "hang") {
	emit({ type: "system", session_id: "sess-hang" });
	// Never exits — runner must SIGTERM the group.
	setInterval(() => {}, 1000);
} else if (mode === "crash") {
	process.stderr.write("boom\n");
	process.exit(2);
}
```

- [ ] **Step 2: Write the failing test**

`packages/core/src/__tests__/runner.test.ts`:

```ts
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { makeRedactor } from "../redact.js";
import { executeClaude, formatEventToMarkdown } from "../runner.js";

const FAKE = join(
	dirname(fileURLToPath(import.meta.url)),
	"fixtures",
	"fake-claude.mjs",
);

function paths() {
	const dir = mkdtempSync(join(tmpdir(), "qo-run-"));
	return {
		dir,
		eventsPath: join(dir, "events.jsonl"),
		transcriptPath: join(dir, "transcript.md"),
	};
}

const passthrough = makeRedactor(new Map());

describe("executeClaude", () => {
	it("captures result, usage, session id, and writes events + transcript", async () => {
		const { dir, eventsPath, transcriptPath } = paths();
		const result = await executeClaude({
			prompt: "do the thing",
			model: "opus",
			cwd: dir,
			timeoutMs: 30_000,
			claudeBin: FAKE,
			eventsPath,
			transcriptPath,
			redact: makeRedactor(new Map([["TOKEN_VALUE_XYZ", "MY_TOKEN"]])),
		});
		expect(result.exitCode).toBe(0);
		expect(result.timedOut).toBe(false);
		expect(result.sessionId).toBe("sess-123");
		expect(result.resultText).toBe("All done.");
		expect(result.usage).toEqual({ costUsd: 0.42, turns: 3, durationMs: 1234 });

		const events = readFileSync(eventsPath, "utf-8").trim().split("\n");
		expect(events).toHaveLength(3);
		expect(events[1]).toContain("[REDACTED:MY_TOKEN]");
		expect(events[1]).not.toContain("TOKEN_VALUE_XYZ");

		const transcript = readFileSync(transcriptPath, "utf-8");
		expect(transcript).toContain("### Tool: Bash");
		expect(transcript).toContain("echo hi");
		expect(transcript).toContain("[REDACTED:MY_TOKEN]");
	});

	it("times out a hung process", async () => {
		const { eventsPath, transcriptPath, dir } = paths();
		const result = await executeClaude({
			prompt: "hang",
			model: "opus",
			cwd: dir,
			timeoutMs: 1500,
			claudeBin: FAKE,
			claudeArgs: [],
			eventsPath,
			transcriptPath,
			redact: passthrough,
		});
		expect(result.timedOut).toBe(true);
	}, 15_000);

	it("reports nonzero exit with stderr", async () => {
		const { eventsPath, transcriptPath, dir } = paths();
		const result = await executeClaude({
			prompt: "x",
			model: "opus",
			cwd: dir,
			timeoutMs: 10_000,
			claudeBin: FAKE,
			eventsPath,
			transcriptPath,
			redact: passthrough,
		});
		// FAKE_CLAUDE_MODE comes from env — set per test via claudeArgs is not possible;
		// crash mode is exercised via env in this test file's runner config below.
		expect(result.exitCode).toBeGreaterThanOrEqual(0);
	});

	it("resolves (never rejects) when the binary is missing", async () => {
		const { eventsPath, transcriptPath, dir } = paths();
		const result = await executeClaude({
			prompt: "x",
			model: "opus",
			cwd: dir,
			timeoutMs: 5_000,
			claudeBin: "/nonexistent/claude-bin",
			eventsPath,
			transcriptPath,
			redact: passthrough,
		});
		expect(result.exitCode).toBe(1);
		expect(result.stderr).toContain("Failed to spawn");
	});
});

describe("formatEventToMarkdown", () => {
	it("formats assistant text, thinking, and tool_use blocks", () => {
		const md = formatEventToMarkdown({
			type: "assistant",
			message: {
				content: [
					{ type: "thinking", thinking: "hmm" },
					{ type: "text", text: "hello" },
					{ type: "tool_use", name: "Edit", input: { file_path: "/a.ts" } },
				],
			},
		});
		expect(md).toContain("### Thinking");
		expect(md).toContain("hello");
		expect(md).toContain("### Tool: Edit");
		expect(md).toContain("File: `/a.ts`");
	});

	it("returns null for non-assistant events", () => {
		expect(formatEventToMarkdown({ type: "system" })).toBeNull();
	});
});
```

Note for the crash path: rather than per-test env plumbing, the hang test drives `FAKE_CLAUDE_MODE` via `executeClaude`'s spawned env. To keep the fixture simple, `executeClaude` must pass an `env` merged from `process.env` — the test sets mode by prompt content instead is NOT required; instead expose `opts.env?: Record<string, string>` on `executeClaude` and pass `{ FAKE_CLAUDE_MODE: "hang" }` / `"crash"` in those tests. Update the two tests accordingly:

```ts
// hang test: add
			env: { FAKE_CLAUDE_MODE: "hang" },
// crash test: replace body with
		const result = await executeClaude({
			prompt: "x",
			model: "opus",
			cwd: dir,
			timeoutMs: 10_000,
			claudeBin: FAKE,
			env: { FAKE_CLAUDE_MODE: "crash" },
			eventsPath,
			transcriptPath,
			redact: passthrough,
		});
		expect(result.exitCode).toBe(2);
		expect(result.stderr).toContain("boom");
```

- [ ] **Step 3: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test`
Expected: FAIL — cannot find module `../runner.js`.

- [ ] **Step 4: Implement**

`packages/core/src/runner.ts`:

```ts
import { type ChildProcess, spawn } from "node:child_process";
import { appendFileSync, writeFileSync } from "node:fs";
import type { Redactor } from "./redact.js";

export interface RunUsage {
	costUsd: number | null;
	turns: number | null;
	durationMs: number | null;
}

export interface RunResult {
	exitCode: number;
	timedOut: boolean;
	sessionId: string | null;
	resultText: string;
	stderr: string;
	usage: RunUsage;
}

export interface ExecuteClaudeOptions {
	prompt: string;
	model: string;
	cwd: string;
	timeoutMs: number;
	claudeBin?: string;
	claudeArgs?: string[];
	env?: Record<string, string>;
	eventsPath: string;
	transcriptPath: string;
	redact: Redactor;
	onSpawned?: (pid: number) => void;
}

export function formatEventToMarkdown(
	event: Record<string, unknown>,
): string | null {
	if ((event.type as string) !== "assistant") return null;
	const msg = event.message as Record<string, unknown> | undefined;
	const content = msg?.content as Array<Record<string, unknown>> | undefined;
	if (!content) return null;

	const parts: string[] = [];
	for (const block of content) {
		if (block.type === "thinking" && block.thinking) {
			parts.push("### Thinking");
			parts.push(String(block.thinking));
			parts.push("");
		}
		if (block.type === "text" && block.text) {
			parts.push(String(block.text));
			parts.push("");
		}
		if (block.type === "tool_use") {
			const name = block.name as string;
			const input = (block.input as Record<string, unknown>) ?? {};
			parts.push(`### Tool: ${name}`);
			const filePath = input.file_path as string | undefined;
			if (name === "Bash" && input.command) {
				parts.push("```bash");
				parts.push(String(input.command));
				parts.push("```");
			} else if (["Edit", "Read", "Write"].includes(name) && filePath) {
				parts.push(`File: \`${filePath}\``);
			} else if (name === "Grep" && input.pattern) {
				parts.push(`Pattern: \`${input.pattern}\``);
			} else {
				parts.push("```json");
				parts.push(JSON.stringify(input, null, 2).slice(0, 500));
				parts.push("```");
			}
			parts.push("");
		}
	}
	return parts.length > 0 ? parts.join("\n") : null;
}

export function executeClaude(
	opts: ExecuteClaudeOptions,
): Promise<RunResult> {
	const timeoutMs = Math.max(1000, opts.timeoutMs);
	const args = [
		"-p",
		opts.prompt,
		"--output-format",
		"stream-json",
		"--verbose",
		"--model",
		opts.model,
		...(opts.claudeArgs ?? []),
	];

	return new Promise((resolve) => {
		const child: ChildProcess = spawn(opts.claudeBin ?? "claude", args, {
			env: { ...process.env, ...opts.env },
			cwd: opts.cwd,
			stdio: ["ignore", "pipe", "pipe"],
			detached: true,
		});
		if (child.pid && opts.onSpawned) opts.onSpawned(child.pid);

		let stderr = "";
		let resultText = "";
		let timedOut = false;
		let sessionId: string | null = null;
		let usage: RunUsage = { costUsd: null, turns: null, durationMs: null };
		let lineBuffer = "";

		writeFileSync(opts.eventsPath, "");
		writeFileSync(opts.transcriptPath, "");

		const timeout = setTimeout(() => {
			timedOut = true;
			if (child.pid) {
				try {
					process.kill(-child.pid, "SIGTERM");
				} catch {
					child.kill("SIGTERM");
				}
			}
			setTimeout(() => {
				if (child.pid) {
					try {
						process.kill(-child.pid, "SIGKILL");
					} catch {}
				}
			}, 5000).unref();
		}, timeoutMs);

		const handleLine = (line: string) => {
			if (!line.trim()) return;
			let event: Record<string, unknown>;
			try {
				event = JSON.parse(line);
			} catch {
				return;
			}
			appendFileSync(opts.eventsPath, `${opts.redact(line)}\n`);

			if (!sessionId && event.session_id) {
				sessionId = event.session_id as string;
			}
			if ((event.type as string) === "result") {
				resultText = (event.result as string) ?? "";
				usage = {
					costUsd:
						typeof event.total_cost_usd === "number"
							? event.total_cost_usd
							: null,
					turns: typeof event.num_turns === "number" ? event.num_turns : null,
					durationMs:
						typeof event.duration_ms === "number" ? event.duration_ms : null,
				};
			}
			const md = formatEventToMarkdown(event);
			if (md) appendFileSync(opts.transcriptPath, `${opts.redact(md)}\n`);
		};

		child.stdout?.on("data", (chunk: Buffer) => {
			lineBuffer += chunk.toString();
			const lines = lineBuffer.split("\n");
			lineBuffer = lines.pop() ?? "";
			for (const line of lines) handleLine(line);
		});

		child.stderr?.on("data", (chunk: Buffer) => {
			stderr += chunk.toString();
		});

		child.on("close", (code) => {
			clearTimeout(timeout);
			if (lineBuffer) handleLine(lineBuffer);
			resolve({
				exitCode: code ?? 1,
				timedOut,
				sessionId,
				resultText,
				stderr,
				usage,
			});
		});

		child.on("error", () => {
			clearTimeout(timeout);
			resolve({
				exitCode: 1,
				timedOut: false,
				sessionId,
				resultText: "",
				stderr: "Failed to spawn process",
				usage,
			});
		});
	});
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test`
Expected: PASS (hang test takes ~1.5s+5s worst case; ensure suite green).

- [ ] **Step 6: Commit**

```bash
git add packages/core/src/runner.ts packages/core/src/__tests__/runner.test.ts packages/core/src/__tests__/fixtures/fake-claude.mjs
git commit -m "feat(core): claude stream-json runner with events, transcript, redaction (agent247 port)"
```

---

### Task 5: Run store (snapshot, report, worker pid)

**Files:**
- Create: `packages/core/src/run-store.ts`
- Test: `packages/core/src/__tests__/run-store.test.ts`

**Interfaces:**
- Consumes: `Redactor`, `RunResult`, `TaskInstance`, `TaskDefinition`.
- Produces: `class RunStore`:
  - `constructor(runsDir: string)`
  - `runDir(taskId: string): string` — `<runsDir>/<taskId>`, mkdir'd on demand.
  - `writeSnapshot(taskId, data: { task: TaskInstance; definition: TaskDefinition | null; resolvedWorktree: string; prompt: string; model: string }, redact: Redactor): void` — writes `data.json` (`{ task, definition, resolved_worktree, model, started_at }`) + `prompt.rendered.md`, both redacted.
  - `writeWorkerPid(taskId, pid: number): void` / `readWorkerPid(taskId): number | null` — `worker.json`.
  - `finishRun(taskId, data: { result: RunResult; outcome: "done" | "failed"; reason: string | null }, redact: Redactor): void` — merges `{ finished_at, outcome, reason, exit_code, timed_out, session_id, usage }` into `data.json`, writes `report.md` (`# Result\n\n<resultText>\n\n## Stats\n- outcome / reason / cost / turns / duration / model`), redacted.
  - `readRunMeta(taskId): Record<string, unknown> | null` — parsed `data.json` or null.
  - `eventsPath(taskId)` / `transcriptPath(taskId)` — path helpers used by the worker.

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/run-store.test.ts`:

```ts
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { makeRedactor } from "../redact.js";
import { RunStore } from "../run-store.js";
import type { TaskInstance } from "../task.js";

const task: TaskInstance = {
	id: "01RUNSTORE0000000000000000",
	status: "running",
	definition: "platform/pr-review",
	item: { number: "257" },
	itemKey: "257",
	target: { repo: "platform", ref: "pr:257", worktree: "JUS-257" },
	priority: "normal",
	created: "2026-07-08T10:00:00.000Z",
	source: "mcp",
	ephemeralWorktree: false,
	error: null,
	prompt: "Review PR 257 with secret shh-token.\n",
};

const redact = makeRedactor(new Map([["shh-token", "GH_TOKEN"]]));
const fresh = () => new RunStore(mkdtempSync(join(tmpdir(), "qo-runs-")));

describe("RunStore", () => {
	it("writeSnapshot writes redacted data.json and prompt", () => {
		const rs = fresh();
		rs.writeSnapshot(
			task.id,
			{ task, definition: null, resolvedWorktree: "JUS-257", prompt: task.prompt, model: "opus" },
			redact,
		);
		const meta = rs.readRunMeta(task.id);
		expect(meta?.resolved_worktree).toBe("JUS-257");
		expect(meta?.model).toBe("opus");
		expect(typeof meta?.started_at).toBe("string");
		const prompt = readFileSync(join(rs.runDir(task.id), "prompt.rendered.md"), "utf-8");
		expect(prompt).toContain("[REDACTED:GH_TOKEN]");
		expect(prompt).not.toContain("shh-token");
	});

	it("worker pid round-trips", () => {
		const rs = fresh();
		rs.writeWorkerPid(task.id, 4242);
		expect(rs.readWorkerPid(task.id)).toBe(4242);
		expect(rs.readWorkerPid("01NOPE")).toBeNull();
	});

	it("finishRun merges outcome into data.json and writes report.md", () => {
		const rs = fresh();
		rs.writeSnapshot(
			task.id,
			{ task, definition: null, resolvedWorktree: "JUS-257", prompt: task.prompt, model: "opus" },
			redact,
		);
		rs.finishRun(
			task.id,
			{
				result: {
					exitCode: 0,
					timedOut: false,
					sessionId: "s1",
					resultText: "Fixed everything with shh-token.",
					stderr: "",
					usage: { costUsd: 1.5, turns: 7, durationMs: 60000 },
				},
				outcome: "done",
				reason: null,
			},
			redact,
		);
		const meta = rs.readRunMeta(task.id);
		expect(meta?.outcome).toBe("done");
		expect(meta?.exit_code).toBe(0);
		expect((meta?.usage as Record<string, unknown>).costUsd).toBe(1.5);
		const report = readFileSync(join(rs.runDir(task.id), "report.md"), "utf-8");
		expect(report).toContain("[REDACTED:GH_TOKEN]");
		expect(report).toContain("$1.5");
		expect(existsSync(rs.eventsPath(task.id))).toBe(false); // runner owns these
	});

	it("readRunMeta returns null for unknown task", () => {
		expect(fresh().readRunMeta("01NOPE")).toBeNull();
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test`
Expected: FAIL — cannot find module `../run-store.js`.

- [ ] **Step 3: Implement**

`packages/core/src/run-store.ts`:

```ts
import {
	existsSync,
	mkdirSync,
	readFileSync,
	writeFileSync,
} from "node:fs";
import { join } from "node:path";
import type { TaskDefinition } from "./definition.js";
import type { Redactor } from "./redact.js";
import type { RunResult } from "./runner.js";
import type { TaskInstance } from "./task.js";

export class RunStore {
	constructor(readonly runsDir: string) {
		mkdirSync(runsDir, { recursive: true });
	}

	runDir(taskId: string): string {
		const dir = join(this.runsDir, taskId);
		mkdirSync(dir, { recursive: true });
		return dir;
	}

	eventsPath(taskId: string): string {
		return join(this.runDir(taskId), "events.jsonl");
	}

	transcriptPath(taskId: string): string {
		return join(this.runDir(taskId), "transcript.md");
	}

	writeSnapshot(
		taskId: string,
		data: {
			task: TaskInstance;
			definition: TaskDefinition | null;
			resolvedWorktree: string;
			prompt: string;
			model: string;
		},
		redact: Redactor,
	): void {
		const dir = this.runDir(taskId);
		const snapshot = {
			task: data.task,
			definition: data.definition,
			resolved_worktree: data.resolvedWorktree,
			model: data.model,
			started_at: new Date().toISOString(),
		};
		writeFileSync(
			join(dir, "data.json"),
			redact(JSON.stringify(snapshot, null, 2)),
		);
		writeFileSync(join(dir, "prompt.rendered.md"), redact(data.prompt));
	}

	writeWorkerPid(taskId: string, pid: number): void {
		writeFileSync(
			join(this.runDir(taskId), "worker.json"),
			JSON.stringify({ pid }),
		);
	}

	readWorkerPid(taskId: string): number | null {
		const path = join(this.runsDir, taskId, "worker.json");
		if (!existsSync(path)) return null;
		try {
			const parsed = JSON.parse(readFileSync(path, "utf-8"));
			return typeof parsed.pid === "number" ? parsed.pid : null;
		} catch {
			return null;
		}
	}

	finishRun(
		taskId: string,
		data: {
			result: RunResult;
			outcome: "done" | "failed";
			reason: string | null;
		},
		redact: Redactor,
	): void {
		const dir = this.runDir(taskId);
		const dataPath = join(dir, "data.json");
		let existing: Record<string, unknown> = {};
		if (existsSync(dataPath)) {
			try {
				existing = JSON.parse(readFileSync(dataPath, "utf-8"));
			} catch {}
		}
		const merged = {
			...existing,
			finished_at: new Date().toISOString(),
			outcome: data.outcome,
			reason: data.reason,
			exit_code: data.result.exitCode,
			timed_out: data.result.timedOut,
			session_id: data.result.sessionId,
			usage: data.result.usage,
		};
		writeFileSync(dataPath, redact(JSON.stringify(merged, null, 2)));

		const { usage } = data.result;
		const report = [
			"# Result",
			"",
			data.result.resultText || "(no result text)",
			"",
			"## Stats",
			`- outcome: ${data.outcome}${data.reason ? ` (${data.reason})` : ""}`,
			`- cost: ${usage.costUsd === null ? "n/a" : `$${usage.costUsd}`}`,
			`- turns: ${usage.turns ?? "n/a"}`,
			`- duration: ${usage.durationMs === null ? "n/a" : `${Math.round(usage.durationMs / 1000)}s`}`,
			"",
		].join("\n");
		writeFileSync(join(dir, "report.md"), redact(report));
	}

	readRunMeta(taskId: string): Record<string, unknown> | null {
		const path = join(this.runsDir, taskId, "data.json");
		if (!existsSync(path)) return null;
		try {
			return JSON.parse(readFileSync(path, "utf-8"));
		} catch {
			return null;
		}
	}
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/run-store.ts packages/core/src/__tests__/run-store.test.ts
git commit -m "feat(core): run store — snapshot, report, worker pid"
```

---

### Task 6: Hooks + worker (end-to-end task execution)

**Files:**
- Create: `packages/core/src/hooks.ts`, `packages/core/src/worker.ts`
- Test: `packages/core/src/__tests__/hooks.test.ts`, `packages/core/src/__tests__/worker.test.ts`

**Interfaces:**
- Consumes: `Exec`, `QueueStore`, `RunStore`, `executeClaude` (injected as a function type), `TaskDefinition`, `Redactor`, `WorktreeInfo`.
- Produces:
  - `execHook(cmd: string, exec: Exec, opts: { cwd: string }): Promise<void>` — runs via `/bin/bash -lc`, throws `Error("hook failed (exit N): <cmd>")` on nonzero exit.
  - `type ClaudeExecutor = typeof executeClaude` (same signature)
  - `interface WorkerDeps { store: QueueStore; runStore: RunStore; exec: Exec; executeClaude: ClaudeExecutor; redact: Redactor; loadDef: (definition: string) => TaskDefinition | null; worktreePath: (repo: string, worktree: string) => Promise<string | null>; defaults: { model: string; timeoutMs: number } }`
  - `runTask(taskId: string, deps: WorkerDeps): Promise<TaskInstance>` — the end-to-end worker:
    1. `store.get(taskId)` (missing → throw), `store.update(id, {status: "running"})`.
    2. Resolve worktree path via `deps.worktreePath` (null → fail "worktree path not found: <lane>").
    3. Load definition when `task.definition` is set (loadDef returns null → fail "definition not found") for model/timeout/hooks; adhoc uses `deps.defaults`.
    4. `runStore.writeSnapshot(...)` + `writeWorkerPid(id, process.pid)`.
    5. `pre_run` hook if present (failure → `failed` with error `pre_run failed: <msg>`, Claude never runs, post_run DOES still run).
    6. `executeClaude` with task.prompt, model, worktree cwd, events/transcript paths.
    7. `post_run` hook if present — ALWAYS attempted (even after pre_run/claude failure); its own failure only logs into reason suffix, never changes outcome from done.
    8. Completion contract: exitCode 0 AND !timedOut AND `git status --porcelain` (via exec, cwd=worktree) empty → `done`; else `failed` with reason (`"exit code N"` / `"timed out"` / `"tree left dirty"`).
    9. `runStore.finishRun(...)`, `store.update(id, {status, error})`, return updated task.

- [ ] **Step 1: Write the failing hooks test**

`packages/core/src/__tests__/hooks.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { execHook } from "../hooks.js";
import type { Exec } from "../resolver-io.js";

describe("execHook", () => {
	it("runs the command through bash -lc in cwd", async () => {
		let seen: unknown;
		const exec: Exec = async (command, args, opts) => {
			seen = { command, args, cwd: opts.cwd };
			return { stdout: "", exitCode: 0 };
		};
		await execHook("mise run setup", exec, { cwd: "/wt" });
		expect(seen).toEqual({
			command: "/bin/bash",
			args: ["-lc", "mise run setup"],
			cwd: "/wt",
		});
	});

	it("throws on nonzero exit", async () => {
		const exec: Exec = async () => ({ stdout: "", exitCode: 3 });
		await expect(execHook("boom", exec, { cwd: "/wt" })).rejects.toThrow(
			"hook failed (exit 3): boom",
		);
	});
});
```

- [ ] **Step 2: Implement hooks after seeing it fail**

`packages/core/src/hooks.ts`:

```ts
import type { Exec } from "./resolver-io.js";

export async function execHook(
	cmd: string,
	exec: Exec,
	opts: { cwd: string },
): Promise<void> {
	const { exitCode } = await exec("/bin/bash", ["-lc", cmd], { cwd: opts.cwd });
	if (exitCode !== 0) {
		throw new Error(`hook failed (exit ${exitCode}): ${cmd}`);
	}
}
```

Run: hooks tests PASS.

- [ ] **Step 3: Write the failing worker test**

`packages/core/src/__tests__/worker.test.ts`:

```ts
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { TaskDefinition } from "../definition.js";
import { makeRedactor } from "../redact.js";
import type { Exec } from "../resolver-io.js";
import { RunStore } from "../run-store.js";
import type { RunResult } from "../runner.js";
import { QueueStore } from "../store.js";
import type { WorkerDeps } from "../worker.js";
import { runTask } from "../worker.js";

const okResult: RunResult = {
	exitCode: 0,
	timedOut: false,
	sessionId: "s",
	resultText: "did it",
	stderr: "",
	usage: { costUsd: 0.1, turns: 1, durationMs: 100 },
};

function makeDeps(overrides: Partial<WorkerDeps> = {}) {
	const base = mkdtempSync(join(tmpdir(), "qo-worker-"));
	const store = new QueueStore(join(base, "state"));
	const runStore = new RunStore(join(base, "runs"));
	const hookCalls: string[] = [];
	const gitClean: Exec = async (_c, args) => {
		const joined = args.join(" ");
		if (joined.includes("status")) return { stdout: "", exitCode: 0 };
		hookCalls.push(joined.replace("-lc ", ""));
		return { stdout: "", exitCode: 0 };
	};
	const deps: WorkerDeps = {
		store,
		runStore,
		exec: gitClean,
		executeClaude: async () => okResult,
		redact: makeRedactor(new Map()),
		loadDef: () => null,
		worktreePath: async () => "/wt/path",
		defaults: { model: "sonnet", timeoutMs: 60_000 },
		...overrides,
	};
	return { deps, store, runStore, hookCalls };
}

const enqueue = (store: QueueStore, definition?: string) =>
	store.create({
		prompt: "do it\n",
		repo: "platform",
		ref: "temp",
		source: "tui",
		definition,
		item: definition ? { number: "1" } : undefined,
		itemKey: definition ? "1" : undefined,
	});

function withWorktree(store: QueueStore, id: string) {
	return store.update(id, {
		target: { repo: "platform", ref: "temp", worktree: "tmp-x" },
	});
}

describe("runTask", () => {
	it("happy path: adhoc task ends done with report + snapshot", async () => {
		const { deps, store, runStore } = makeDeps();
		const t = enqueue(store);
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		expect(result.error).toBeNull();
		const meta = runStore.readRunMeta(t.id);
		expect(meta?.outcome).toBe("done");
		expect(meta?.model).toBe("sonnet");
		expect(runStore.readWorkerPid(t.id)).toBe(process.pid);
	});

	it("nonzero exit → failed with exit reason", async () => {
		const { deps, store } = makeDeps({
			executeClaude: async () => ({ ...okResult, exitCode: 3 }),
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("failed");
		expect(result.error).toBe("exit code 3");
	});

	it("timeout → failed with timed out reason", async () => {
		const { deps, store } = makeDeps({
			executeClaude: async () => ({ ...okResult, timedOut: true, exitCode: 1 }),
		});
		const t = enqueue(store);
		withWorktree(store, t.id);
		expect((await runTask(t.id, deps)).error).toBe("timed out");
	});

	it("dirty tree → failed with tree left dirty", async () => {
		const dirtyGit: Exec = async (_c, args) =>
			args.join(" ").includes("status")
				? { stdout: " M src/x.ts\n", exitCode: 0 }
				: { stdout: "", exitCode: 0 };
		const { deps, store } = makeDeps({ exec: dirtyGit });
		const t = enqueue(store);
		withWorktree(store, t.id);
		expect((await runTask(t.id, deps)).error).toBe("tree left dirty");
	});

	it("definition task uses def model/timeout and runs hooks around claude", async () => {
		const def: TaskDefinition = {
			name: "pr-review",
			repo: "platform",
			discovery: null,
			args: ["number"],
			dedup: "none",
			worktree: "temp",
			preRun: "mise run setup",
			postRun: "echo done",
			model: "opus",
			timeoutMs: 120_000,
			priority: "normal",
			prompt: "review {{number}}",
		};
		let claudeModel = "";
		const { deps, store, hookCalls, runStore } = makeDeps({
			loadDef: () => def,
			executeClaude: async (opts) => {
				claudeModel = opts.model;
				return okResult;
			},
		});
		const t = enqueue(store, "platform/pr-review");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("done");
		expect(claudeModel).toBe("opus");
		expect(hookCalls).toEqual(["mise run setup", "echo done"]);
		expect(runStore.readRunMeta(t.id)?.model).toBe("opus");
	});

	it("pre_run failure → failed, claude never runs, post_run still runs", async () => {
		const def: TaskDefinition = {
			name: "d",
			repo: "platform",
			discovery: null,
			args: [],
			dedup: "none",
			worktree: "temp",
			preRun: "bad-setup",
			postRun: "cleanup",
			model: "opus",
			timeoutMs: 60_000,
			priority: "normal",
			prompt: "p",
		};
		const calls: string[] = [];
		const exec: Exec = async (_c, args) => {
			const cmd = args[1] ?? "";
			calls.push(cmd);
			if (cmd === "bad-setup") return { stdout: "", exitCode: 1 };
			return { stdout: "", exitCode: 0 };
		};
		let claudeRan = false;
		const { deps, store } = makeDeps({
			exec,
			loadDef: () => def,
			executeClaude: async () => {
				claudeRan = true;
				return okResult;
			},
		});
		const t = enqueue(store, "platform/d");
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("failed");
		expect(result.error).toContain("pre_run failed");
		expect(claudeRan).toBe(false);
		expect(calls).toContain("cleanup");
	});

	it("unresolved worktree path → failed", async () => {
		const { deps, store } = makeDeps({ worktreePath: async () => null });
		const t = enqueue(store);
		withWorktree(store, t.id);
		const result = await runTask(t.id, deps);
		expect(result.status).toBe("failed");
		expect(result.error).toContain("worktree path not found");
	});
});
```

- [ ] **Step 4: Run to verify failure, then implement worker**

`packages/core/src/worker.ts`:

```ts
import type { TaskDefinition } from "./definition.js";
import { execHook } from "./hooks.js";
import type { Redactor } from "./redact.js";
import type { Exec } from "./resolver-io.js";
import type { RunStore } from "./run-store.js";
import type { executeClaude, RunResult } from "./runner.js";
import type { QueueStore } from "./store.js";
import type { TaskInstance } from "./task.js";
import { laneKey } from "./task.js";

export type ClaudeExecutor = typeof executeClaude;

export interface WorkerDeps {
	store: QueueStore;
	runStore: RunStore;
	exec: Exec;
	executeClaude: ClaudeExecutor;
	redact: Redactor;
	loadDef: (definition: string) => TaskDefinition | null;
	worktreePath: (repo: string, worktree: string) => Promise<string | null>;
	defaults: { model: string; timeoutMs: number };
}

const EMPTY_RESULT: RunResult = {
	exitCode: 1,
	timedOut: false,
	sessionId: null,
	resultText: "",
	stderr: "",
	usage: { costUsd: null, turns: null, durationMs: null },
};

export async function runTask(
	taskId: string,
	deps: WorkerDeps,
): Promise<TaskInstance> {
	const task = deps.store.get(taskId);
	if (!task) throw new Error(`task not found: ${taskId}`);
	deps.store.update(taskId, { status: "running", error: null });

	const fail = (reason: string, result: RunResult = EMPTY_RESULT) => {
		deps.runStore.finishRun(
			taskId,
			{ result, outcome: "failed", reason },
			deps.redact,
		);
		return deps.store.update(taskId, { status: "failed", error: reason });
	};

	const worktree = task.target.worktree;
	if (worktree === null) {
		return fail("worktree path not found: unresolved task");
	}
	const cwd = await deps.worktreePath(task.target.repo, worktree);
	if (cwd === null) {
		return fail(`worktree path not found: ${laneKey(task)}`);
	}

	let def: TaskDefinition | null = null;
	if (task.definition !== null) {
		def = deps.loadDef(task.definition);
		if (def === null) return fail(`definition not found: ${task.definition}`);
	}
	const model = def?.model ?? deps.defaults.model;
	const timeoutMs = def?.timeoutMs ?? deps.defaults.timeoutMs;

	deps.runStore.writeSnapshot(
		taskId,
		{ task, definition: def, resolvedWorktree: worktree, prompt: task.prompt, model },
		deps.redact,
	);
	deps.runStore.writeWorkerPid(taskId, process.pid);

	let outcome: "done" | "failed" = "done";
	let reason: string | null = null;
	let result: RunResult = EMPTY_RESULT;

	// pre_run
	let preRunOk = true;
	if (def?.preRun) {
		try {
			await execHook(def.preRun, deps.exec, { cwd });
		} catch (err) {
			preRunOk = false;
			outcome = "failed";
			reason = `pre_run failed: ${err instanceof Error ? err.message : String(err)}`;
		}
	}

	// claude
	if (preRunOk) {
		result = await deps.executeClaude({
			prompt: task.prompt,
			model,
			cwd,
			timeoutMs,
			eventsPath: deps.runStore.eventsPath(taskId),
			transcriptPath: deps.runStore.transcriptPath(taskId),
			redact: deps.redact,
		});
		if (result.timedOut) {
			outcome = "failed";
			reason = "timed out";
		} else if (result.exitCode !== 0) {
			outcome = "failed";
			reason = `exit code ${result.exitCode}`;
		} else {
			const status = await deps.exec("git", ["status", "--porcelain"], { cwd });
			if (status.exitCode !== 0 || status.stdout.trim() !== "") {
				outcome = "failed";
				reason = "tree left dirty";
			}
		}
	}

	// post_run — always attempted; its failure never flips a done outcome
	if (def?.postRun) {
		try {
			await execHook(def.postRun, deps.exec, { cwd });
		} catch (err) {
			const msg = `post_run failed: ${err instanceof Error ? err.message : String(err)}`;
			reason = reason ? `${reason}; ${msg}` : null;
		}
	}

	deps.runStore.finishRun(taskId, { result, outcome, reason }, deps.redact);
	return deps.store.update(taskId, {
		status: outcome,
		error: outcome === "failed" ? reason : null,
	});
}
```

Run: `pnpm -F @queohoh/core test` — all PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/hooks.ts packages/core/src/worker.ts packages/core/src/__tests__/hooks.test.ts packages/core/src/__tests__/worker.test.ts
git commit -m "feat(core): hooks and end-to-end task worker with completion contract"
```

---

### Task 7: Session registry + LiveState builder

**Files:**
- Create: `packages/core/src/sessions.ts`
- Test: `packages/core/src/__tests__/sessions.test.ts`

**Interfaces:**
- Consumes: `LiveState` (Plan A scheduler), `TaskInstance`, `laneKey`.
- Produces: `class SessionRegistry`:
  - `constructor(filePath: string, opts?: { interactiveTtlMs?: number; isPidAlive?: (pid: number) => boolean })` — default TTL 300_000; default liveness `process.kill(pid, 0)` try/catch.
  - `registerWorker(taskId: string, lane: string, pid: number): void`
  - `unregisterWorker(taskId: string): void`
  - `upsertInteractive(cwd: string, pid: number | null): void` — keyed by cwd, refreshes `heartbeatAt`.
  - `removeInteractive(cwd: string): void`
  - `sweep(now?: number): void` — drops interactive entries older than TTL and worker entries with dead pids; persists.
  - `list(): SessionEntry[]` where `interface SessionEntry { kind: "worker" | "interactive"; key: string; lane: string | null; cwd: string | null; pid: number | null; startedAt: string; heartbeatAt: string }`
  - `buildLiveState(sessions: SessionEntry[], tasks: TaskInstance[], laneOfCwd: (cwd: string) => string | null): LiveState` (standalone export) — `runningLanes` from tasks with status `running` (lane non-null), `interactiveLanes` from interactive sessions via `laneOfCwd`, `runningCount` = running tasks count.
  - File format: JSON `{ sessions: [...] }`, corrupt/missing file → empty registry (never throws).

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/sessions.test.ts`:

```ts
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { buildLiveState, SessionRegistry } from "../sessions.js";
import type { TaskInstance } from "../task.js";

const file = () => join(mkdtempSync(join(tmpdir(), "qo-sess-")), "sessions.json");

function runningTask(worktree: string): TaskInstance {
	return {
		id: `01SESS${worktree.padEnd(20, "0")}`,
		status: "running",
		definition: null,
		item: null,
		itemKey: null,
		target: { repo: "platform", ref: "temp", worktree },
		priority: "normal",
		created: "2026-07-08T00:00:00.000Z",
		source: "tui",
		ephemeralWorktree: false,
		error: null,
		prompt: "p",
	};
}

describe("SessionRegistry", () => {
	it("registers and persists workers, reloads from disk", () => {
		const path = file();
		const reg = new SessionRegistry(path);
		reg.registerWorker("t1", "platform:JUS-1", 111);
		const reloaded = new SessionRegistry(path);
		expect(reloaded.list()).toHaveLength(1);
		expect(reloaded.list()[0]?.lane).toBe("platform:JUS-1");
	});

	it("unregisters workers", () => {
		const reg = new SessionRegistry(file());
		reg.registerWorker("t1", "l", 1);
		reg.unregisterWorker("t1");
		expect(reg.list()).toEqual([]);
	});

	it("upserts interactive sessions keyed by cwd", () => {
		const reg = new SessionRegistry(file());
		reg.upsertInteractive("/wt/a", 5);
		reg.upsertInteractive("/wt/a", 5);
		expect(reg.list().filter((s) => s.kind === "interactive")).toHaveLength(1);
	});

	it("sweep drops stale interactive and dead workers", () => {
		const reg = new SessionRegistry(file(), {
			interactiveTtlMs: 1000,
			isPidAlive: (pid) => pid === 111,
		});
		reg.registerWorker("alive", "l1", 111);
		reg.registerWorker("dead", "l2", 222);
		reg.upsertInteractive("/wt/a", null);
		reg.sweep(Date.now() + 5000);
		const kinds = reg.list().map((s) => [s.kind, s.key]);
		expect(kinds).toEqual([["worker", "alive"]]);
	});

	it("tolerates corrupt file", () => {
		const path = file();
		writeFileSync(path, "{nope");
		expect(new SessionRegistry(path).list()).toEqual([]);
	});
});

describe("buildLiveState", () => {
	it("derives lanes from running tasks and interactive sessions", () => {
		const reg = new SessionRegistry(file());
		reg.upsertInteractive("/wt/main", null);
		reg.upsertInteractive("/wt/unknown", null);
		const live = buildLiveState(
			reg.list(),
			[runningTask("JUS-1")],
			(cwd) => (cwd === "/wt/main" ? "platform:main" : null),
		);
		expect(live.runningLanes).toEqual(new Set(["platform:JUS-1"]));
		expect(live.interactiveLanes).toEqual(new Set(["platform:main"]));
		expect(live.runningCount).toBe(1);
	});
});
```

- [ ] **Step 2: Run to verify failure, then implement**

`packages/core/src/sessions.ts`:

```ts
import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import type { LiveState } from "./scheduler.js";
import type { TaskInstance } from "./task.js";
import { laneKey } from "./task.js";

export interface SessionEntry {
	kind: "worker" | "interactive";
	key: string;
	lane: string | null;
	cwd: string | null;
	pid: number | null;
	startedAt: string;
	heartbeatAt: string;
}

function defaultIsPidAlive(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch {
		return false;
	}
}

export class SessionRegistry {
	private sessions: SessionEntry[] = [];
	private readonly ttlMs: number;
	private readonly isPidAlive: (pid: number) => boolean;

	constructor(
		readonly filePath: string,
		opts?: {
			interactiveTtlMs?: number;
			isPidAlive?: (pid: number) => boolean;
		},
	) {
		this.ttlMs = opts?.interactiveTtlMs ?? 300_000;
		this.isPidAlive = opts?.isPidAlive ?? defaultIsPidAlive;
		if (existsSync(filePath)) {
			try {
				const parsed = JSON.parse(readFileSync(filePath, "utf-8"));
				if (Array.isArray(parsed.sessions)) this.sessions = parsed.sessions;
			} catch {
				this.sessions = [];
			}
		}
	}

	private persist(): void {
		const tmp = `${this.filePath}.tmp`;
		writeFileSync(tmp, JSON.stringify({ sessions: this.sessions }, null, 2));
		renameSync(tmp, this.filePath);
	}

	registerWorker(taskId: string, lane: string, pid: number): void {
		const now = new Date().toISOString();
		this.sessions = this.sessions.filter(
			(s) => !(s.kind === "worker" && s.key === taskId),
		);
		this.sessions.push({
			kind: "worker",
			key: taskId,
			lane,
			cwd: null,
			pid,
			startedAt: now,
			heartbeatAt: now,
		});
		this.persist();
	}

	unregisterWorker(taskId: string): void {
		this.sessions = this.sessions.filter(
			(s) => !(s.kind === "worker" && s.key === taskId),
		);
		this.persist();
	}

	upsertInteractive(cwd: string, pid: number | null): void {
		const now = new Date().toISOString();
		const existing = this.sessions.find(
			(s) => s.kind === "interactive" && s.key === cwd,
		);
		if (existing) {
			existing.heartbeatAt = now;
			existing.pid = pid;
		} else {
			this.sessions.push({
				kind: "interactive",
				key: cwd,
				lane: null,
				cwd,
				pid,
				startedAt: now,
				heartbeatAt: now,
			});
		}
		this.persist();
	}

	removeInteractive(cwd: string): void {
		this.sessions = this.sessions.filter(
			(s) => !(s.kind === "interactive" && s.key === cwd),
		);
		this.persist();
	}

	sweep(now: number = Date.now()): void {
		this.sessions = this.sessions.filter((s) => {
			if (s.kind === "interactive") {
				return now - Date.parse(s.heartbeatAt) < this.ttlMs;
			}
			return s.pid !== null && this.isPidAlive(s.pid);
		});
		this.persist();
	}

	list(): SessionEntry[] {
		return [...this.sessions];
	}
}

export function buildLiveState(
	sessions: SessionEntry[],
	tasks: TaskInstance[],
	laneOfCwd: (cwd: string) => string | null,
): LiveState {
	const running = tasks.filter((t) => t.status === "running");
	const runningLanes = new Set<string>();
	for (const t of running) {
		const lane = laneKey(t);
		if (lane) runningLanes.add(lane);
	}
	const interactiveLanes = new Set<string>();
	for (const s of sessions) {
		if (s.kind === "interactive" && s.cwd) {
			const lane = laneOfCwd(s.cwd);
			if (lane) interactiveLanes.add(lane);
		}
	}
	return { runningLanes, interactiveLanes, runningCount: running.length };
}
```

Run: `pnpm -F @queohoh/core test` — PASS.

- [ ] **Step 3: Commit**

```bash
git add packages/core/src/sessions.ts packages/core/src/__tests__/sessions.test.ts
git commit -m "feat(core): session registry and LiveState builder"
```

---

### Task 8: Core barrel update + daemon package scaffold

**Files:**
- Modify: `packages/core/src/index.ts` (export Tasks 1–7 surface)
- Create: `packages/daemon/package.json`, `packages/daemon/tsconfig.json`, `packages/daemon/vitest.config.ts`, `packages/daemon/src/paths.ts`
- Test: `packages/daemon/src/__tests__/paths.test.ts`

**Interfaces:**
- Consumes: everything prior.
- Produces:
  - Core barrel additionally exports: `buildSecretMap`/`redact`/`makeRedactor`/`Redactor`; `discoverItems`; `filterNewItems`/`DedupMode`/`KeyedItem`; `instantiateDefinition`/`Trigger`/`InstantiateDeps`; `executeClaude`/`formatEventToMarkdown`/`RunResult`/`RunUsage`/`ExecuteClaudeOptions`; `RunStore`; `execHook`; `runTask`/`WorkerDeps`/`ClaudeExecutor`; `SessionRegistry`/`SessionEntry`/`buildLiveState`.
  - `@queohoh/daemon` package (deps: `@queohoh/core` workspace link, `commander`, `js-yaml`; dev: vitest, @types/node, typescript).
  - `packages/daemon/src/paths.ts`: `statePath(): string` (env `QUEOHOH_STATE_DIR` else `~/.local/state/queohoh`), `configPath(): string` (env `QUEOHOH_CONFIG` else `~/.config/queohoh/config.yaml`), `socketPath(state: string)`, `pidPath(state: string)`, `sessionsPath(state: string)`, `runsPath(state: string)` — pure string builders.

- [ ] **Step 1: Update core barrel and smoke test, verify suite still green**

Append to `packages/core/src/index.ts`:

```ts
export { buildSecretMap, makeRedactor, redact } from "./redact.js";
export type { Redactor } from "./redact.js";
export { discoverItems } from "./discovery.js";
export { filterNewItems } from "./dedup.js";
export type { DedupMode, KeyedItem } from "./dedup.js";
export { instantiateDefinition } from "./instantiate.js";
export type { InstantiateDeps, Trigger } from "./instantiate.js";
export { executeClaude, formatEventToMarkdown } from "./runner.js";
export type {
	ExecuteClaudeOptions,
	RunResult,
	RunUsage,
} from "./runner.js";
export { RunStore } from "./run-store.js";
export { execHook } from "./hooks.js";
export { runTask } from "./worker.js";
export type { ClaudeExecutor, WorkerDeps } from "./worker.js";
export { buildLiveState, SessionRegistry } from "./sessions.js";
export type { SessionEntry } from "./sessions.js";
```

Run: `pnpm -F @queohoh/core test && pnpm -F @queohoh/core typecheck` — green.

- [ ] **Step 2: Scaffold the daemon package**

`packages/daemon/package.json`:

```json
{
	"name": "@queohoh/daemon",
	"version": "0.1.0",
	"type": "module",
	"main": "./src/index.ts",
	"bin": { "queohoh": "./dist/cli.js" },
	"scripts": {
		"test": "vitest run",
		"typecheck": "tsc --noEmit",
		"build": "tsc"
	},
	"dependencies": {
		"@queohoh/core": "workspace:*",
		"commander": "^14.0.3",
		"js-yaml": "^4.1.1"
	},
	"devDependencies": {
		"@types/js-yaml": "^4.0.9",
		"@types/node": "^25.5.0",
		"typescript": "^6.0.2",
		"vitest": "^4.1.0"
	}
}
```

`packages/daemon/tsconfig.json`:

```json
{
	"extends": "../../tsconfig.base.json",
	"compilerOptions": { "rootDir": "src", "outDir": "dist" },
	"include": ["src"]
}
```

`packages/daemon/vitest.config.ts`:

```ts
import { defineConfig } from "vitest/config";

export default defineConfig({
	test: { include: ["src/**/*.test.ts"] },
});
```

- [ ] **Step 3: Write the failing paths test**

`packages/daemon/src/__tests__/paths.test.ts`:

```ts
import { homedir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
	configPath,
	pidPath,
	runsPath,
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
		expect(runsPath("/s")).toBe("/s/runs");
	});
});
```

- [ ] **Step 4: Implement paths, install, verify**

`packages/daemon/src/paths.ts`:

```ts
import { homedir } from "node:os";
import { join } from "node:path";

export function statePath(): string {
	return process.env.QUEOHOH_STATE_DIR ?? join(homedir(), ".local/state/queohoh");
}

export function configPath(): string {
	return process.env.QUEOHOH_CONFIG ?? join(homedir(), ".config/queohoh/config.yaml");
}

export const socketPath = (state: string) => join(state, "daemon/daemon.sock");
export const pidPath = (state: string) => join(state, "daemon/daemon.pid");
export const sessionsPath = (state: string) => join(state, "daemon/sessions.json");
export const runsPath = (state: string) => join(state, "runs");
```

Run: `pnpm install && pnpm -F @queohoh/daemon test && pnpm -F @queohoh/daemon typecheck`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(daemon): package scaffold and path helpers; export execution surface from core"
```

---

### Task 9: Daemon engine (tick loop, orphan sweep, auto-archive)

**Files:**
- Create: `packages/daemon/src/engine.ts`
- Test: `packages/daemon/src/__tests__/engine.test.ts`

**Interfaces:**
- Consumes: core (`QueueStore`, `RunStore`, `schedule`, `buildLiveState`, `SessionRegistry`, `resolveTarget`, `createResolverIO`, `runTask`, `loadDefinition`, `loadGlobalConfig`, `loadRepoConfig`, `render`, `laneKey`, `formatRef`).
- Produces: `class Engine`:
  - `constructor(deps: EngineDeps)` where `interface EngineDeps { store: QueueStore; runStore: RunStore; registry: SessionRegistry; config: GlobalConfig; resolverIO: ResolverIO; exec: Exec; executeClaude: ClaudeExecutor; redact: Redactor; onChange?: () => void }`.
  - `async tick(): Promise<void>` — one scheduling pass, safe to call repeatedly (re-entrancy guarded by an internal `ticking` flag; overlapping calls coalesce). Pass order:
    1. `registry.sweep()`.
    2. **Orphan sweep**: tasks `running` with no in-memory worker → `failed`, error `"orphaned by daemon restart"`.
    3. **Auto-archive**: `done` tasks older than `config.archiveAfterDays` (by `created`) → `store.archive`.
    4. Build `LiveState` (in-memory running workers count as runningLanes too).
    5. `schedule(...)` with `maxConcurrent`.
    6. For each `resolve` decision: `resolveTarget` (repo path from config projects; unknown repo → `needs-input` with reason) — resolved → `update {target.worktree, ephemeralWorktree}`; needs-input → `update {status: "needs-input", error: reason}`; **thrown spawn errors → `failed` with message** (Global Constraint).
    7. For each `start` decision: launch `runTask` async — track in in-memory `running: Map<taskId, Promise>`, `registry.registerWorker` before, `unregisterWorker` + `onChange` in finally.
  - `runningTaskIds(): string[]`
  - `laneOfCwd(cwd: string): string | null` — maps a cwd to `"<repo>:<worktree>"` by prefix-matching against cached `listWorktrees` of each configured project (cache refreshed each tick).
  - Engine never throws from `tick()` — per-task errors land on the task; unexpected errors are caught and logged to `console.error`.

- [ ] **Step 1: Write the failing test**

`packages/daemon/src/__tests__/engine.test.ts`:

```ts
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
	makeRedactor,
	QueueStore,
	RunStore,
	SessionRegistry,
} from "@queohoh/core";
import type { Exec, GlobalConfig, ResolverIO, RunResult } from "@queohoh/core";
import { describe, expect, it } from "vitest";
import { Engine } from "../engine.js";

const okResult: RunResult = {
	exitCode: 0,
	timedOut: false,
	sessionId: null,
	resultText: "ok",
	stderr: "",
	usage: { costUsd: 0, turns: 1, durationMs: 10 },
};

function setup(overrides: {
	resolverIO?: Partial<ResolverIO>;
	config?: Partial<GlobalConfig>;
	claudeResult?: RunResult;
} = {}) {
	const base = mkdtempSync(join(tmpdir(), "qo-engine-"));
	const repoPath = join(base, "repo");
	mkdirSync(repoPath, { recursive: true });
	const store = new QueueStore(join(base, "state"));
	const runStore = new RunStore(join(base, "runs"));
	const registry = new SessionRegistry(join(base, "sessions.json"));
	const config: GlobalConfig = {
		projects: [{ name: "platform", path: repoPath }],
		maxConcurrentTasks: 3,
		archiveAfterDays: 7,
		vars: {},
		...overrides.config,
	};
	const resolverIO: ResolverIO = {
		listWorktrees: async () => [
			{ name: "JUS-1", path: join(base, "wt-jus1"), branch: "JUS-1" },
		],
		prBranch: async () => null,
		spawnWorktree: async (_r, name) => ({
			name,
			path: join(base, `wt-${name}`),
			branch: name,
		}),
		...overrides.resolverIO,
	};
	const exec: Exec = async () => ({ stdout: "", exitCode: 0 });
	const engine = new Engine({
		store,
		runStore,
		registry,
		config,
		resolverIO,
		exec,
		executeClaude: async () => overrides.claudeResult ?? okResult,
		redact: makeRedactor(new Map()),
	});
	return { engine, store, base };
}

describe("Engine.tick", () => {
	it("resolves an unresolved task then runs it to done across ticks", async () => {
		const { engine, store } = setup();
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		await engine.tick(); // resolve pass
		expect(store.list()[0]?.target.worktree).toBe("JUS-1");
		await engine.tick(); // start pass
		await engine.drain();
		expect(store.list()[0]?.status).toBe("done");
	});

	it("routes unknown repo to needs-input", async () => {
		const { engine, store } = setup();
		store.create({ prompt: "p", repo: "ghost", ref: "temp", source: "tui" });
		await engine.tick();
		const t = store.list()[0];
		expect(t?.status).toBe("needs-input");
		expect(t?.error).toContain("unknown repo");
	});

	it("maps resolver needs-input outcome onto the task", async () => {
		const { engine, store } = setup();
		store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:missing",
			source: "tui",
		});
		await engine.tick();
		expect(store.list()[0]?.status).toBe("needs-input");
	});

	it("maps thrown spawn errors to failed", async () => {
		const { engine, store } = setup({
			resolverIO: {
				spawnWorktree: async () => {
					throw new Error("wt exploded");
				},
			},
		});
		store.create({ prompt: "p", repo: "platform", ref: "temp", source: "tui" });
		await engine.tick();
		const t = store.list()[0];
		expect(t?.status).toBe("failed");
		expect(t?.error).toContain("wt exploded");
	});

	it("marks running tasks with no live worker as orphaned", async () => {
		const { engine, store } = setup();
		const t = store.create({ prompt: "p", repo: "platform", ref: "temp", source: "tui" });
		store.update(t.id, {
			status: "running",
			target: { repo: "platform", ref: "temp", worktree: "x" },
		});
		await engine.tick();
		expect(store.get(t.id)?.status).toBe("failed");
		expect(store.get(t.id)?.error).toBe("orphaned by daemon restart");
	});

	it("archives old done tasks", async () => {
		const { engine, store } = setup();
		const t = store.create({ prompt: "p", repo: "platform", ref: "temp", source: "tui" });
		store.update(t.id, {
			status: "done",
			created: "2020-01-01T00:00:00.000Z",
		});
		await engine.tick();
		expect(store.list()).toEqual([]);
		expect(store.listArchived().map((a) => a.id)).toEqual([t.id]);
	});
});

describe("Engine.laneOfCwd", () => {
	it("prefix-matches worktree paths after a tick", async () => {
		const { engine, base } = setup();
		await engine.tick();
		expect(engine.laneOfCwd(join(base, "wt-jus1", "src"))).toBe(
			"platform:JUS-1",
		);
		expect(engine.laneOfCwd("/elsewhere")).toBeNull();
	});
});
```

- [ ] **Step 2: Run to verify failure, then implement**

`packages/daemon/src/engine.ts`:

```ts
import type {
	ClaudeExecutor,
	Exec,
	GlobalConfig,
	QueueStore,
	Redactor,
	ResolverIO,
	RunStore,
	SessionRegistry,
	TaskInstance,
	WorktreeInfo,
} from "@queohoh/core";
import {
	buildLiveState,
	laneKey,
	loadDefinition,
	loadRepoConfig,
	resolveTarget,
	runTask,
	schedule,
} from "@queohoh/core";

export interface EngineDeps {
	store: QueueStore;
	runStore: RunStore;
	registry: SessionRegistry;
	config: GlobalConfig;
	resolverIO: ResolverIO;
	exec: Exec;
	executeClaude: ClaudeExecutor;
	redact: Redactor;
	onChange?: () => void;
}

export class Engine {
	private running = new Map<string, Promise<void>>();
	private ticking = false;
	private worktreeCache = new Map<string, WorktreeInfo[]>(); // repo name -> worktrees

	constructor(private readonly deps: EngineDeps) {}

	runningTaskIds(): string[] {
		return [...this.running.keys()];
	}

	/** Await all in-flight workers (test helper / shutdown). */
	async drain(): Promise<void> {
		await Promise.all([...this.running.values()]);
	}

	laneOfCwd(cwd: string): string | null {
		for (const [repo, worktrees] of this.worktreeCache) {
			for (const wt of worktrees) {
				if (cwd === wt.path || cwd.startsWith(`${wt.path}/`)) {
					return `${repo}:${wt.name}`;
				}
			}
		}
		return null;
	}

	private repoPath(repo: string): string | null {
		return this.deps.config.projects.find((p) => p.name === repo)?.path ?? null;
	}

	async tick(): Promise<void> {
		if (this.ticking) return;
		this.ticking = true;
		try {
			await this.pass();
		} catch (err) {
			console.error("engine tick error:", err);
		} finally {
			this.ticking = false;
		}
	}

	private async pass(): Promise<void> {
		const { deps } = this;
		deps.registry.sweep();
		await this.refreshWorktreeCache();

		// Orphan sweep: running on disk but not in this process.
		for (const t of deps.store.list()) {
			if (t.status === "running" && !this.running.has(t.id)) {
				deps.store.update(t.id, {
					status: "failed",
					error: "orphaned by daemon restart",
				});
			}
		}

		// Auto-archive old done tasks.
		const cutoff = Date.now() - deps.config.archiveAfterDays * 86_400_000;
		for (const t of deps.store.list()) {
			if (t.status === "done" && Date.parse(t.created) < cutoff) {
				deps.store.archive(t.id);
			}
		}

		const tasks = deps.store.list();
		const live = buildLiveState(deps.registry.list(), tasks, (cwd) =>
			this.laneOfCwd(cwd),
		);
		const decision = schedule(tasks, live, {
			maxConcurrent: deps.config.maxConcurrentTasks,
		});

		for (const task of decision.resolve) {
			await this.resolveTask(task);
		}
		for (const task of decision.start) {
			this.startWorker(task);
		}
	}

	private async refreshWorktreeCache(): Promise<void> {
		for (const project of this.deps.config.projects) {
			try {
				this.worktreeCache.set(
					project.name,
					await this.deps.resolverIO.listWorktrees(project.path),
				);
			} catch {
				this.worktreeCache.set(project.name, []);
			}
		}
	}

	private async resolveTask(task: TaskInstance): Promise<void> {
		const { deps } = this;
		const repoPath = this.repoPath(task.target.repo);
		if (repoPath === null) {
			deps.store.update(task.id, {
				status: "needs-input",
				error: `unknown repo: ${task.target.repo}`,
			});
			deps.onChange?.();
			return;
		}
		try {
			const resolution = await resolveTarget(
				task.target.ref,
				{ repoPath },
				deps.resolverIO,
			);
			if (resolution.outcome === "resolved") {
				deps.store.update(task.id, {
					target: { ...task.target, worktree: resolution.worktree },
					ephemeralWorktree: resolution.ephemeral,
				});
				this.worktreeCache.delete(task.target.repo); // stale after spawn
			} else {
				deps.store.update(task.id, {
					status: "needs-input",
					error: resolution.reason,
				});
			}
		} catch (err) {
			deps.store.update(task.id, {
				status: "failed",
				error: err instanceof Error ? err.message : String(err),
			});
		}
		deps.onChange?.();
	}

	private startWorker(task: TaskInstance): void {
		const { deps } = this;
		const lane = laneKey(task) ?? task.id;
		deps.registry.registerWorker(task.id, lane, process.pid);
		const promise = runTask(task.id, {
			store: deps.store,
			runStore: deps.runStore,
			exec: deps.exec,
			executeClaude: deps.executeClaude,
			redact: deps.redact,
			loadDef: (definition) => {
				const [repo, ...nameParts] = definition.split("/");
				const name = nameParts.join("/");
				const repoPath = this.repoPath(repo ?? "");
				if (!repoPath) return null;
				try {
					return loadDefinition(repoPath, repo as string, name);
				} catch {
					return null;
				}
			},
			worktreePath: async (repo, worktree) => {
				const repoPath = this.repoPath(repo);
				if (!repoPath) return null;
				const list = await deps.resolverIO.listWorktrees(repoPath);
				return list.find((w) => w.name === worktree)?.path ?? null;
			},
			defaults: { model: "sonnet", timeoutMs: 1_800_000 },
		})
			.catch((err) => {
				try {
					deps.store.update(task.id, {
						status: "failed",
						error: err instanceof Error ? err.message : String(err),
					});
				} catch {}
			})
			.then(() => {
				this.running.delete(task.id);
				deps.registry.unregisterWorker(task.id);
				deps.onChange?.();
			});
		this.running.set(task.id, promise);
		deps.onChange?.();
	}
}
```

Note: `loadRepoConfig` import intentionality — if unused after implementation, drop it from the import list (biome will flag it).

Run: `pnpm -F @queohoh/daemon test` — PASS.

- [ ] **Step 3: Commit**

```bash
git add packages/daemon/src/engine.ts packages/daemon/src/__tests__/engine.test.ts
git commit -m "feat(daemon): engine tick loop with resolve/start dispatch, orphan sweep, auto-archive"
```

---

### Task 10: Socket API server + client

**Files:**
- Create: `packages/daemon/src/api.ts`, `packages/daemon/src/client.ts`
- Test: `packages/daemon/src/__tests__/api.test.ts`

**Interfaces:**
- Consumes: `Engine`, `QueueStore`, `RunStore`, `SessionRegistry`, `instantiateDefinition`, `listDefinitions`, `loadRepoConfig`, core types.
- Produces:
  - Protocol (Global Constraints): ndjson request `{id, method, params?}` → `{id, result}` / `{id, error}`; `subscribe` flags the connection for `{event: "state", data: StateSnapshot}` pushes.
  - `interface StateSnapshot { tasks: TaskInstance[]; archivedRecent: TaskInstance[]; sessions: SessionEntry[]; running: string[] }`
  - `class ApiServer`:
    - `constructor(deps: { engine: Engine; store: QueueStore; runStore: RunStore; registry: SessionRegistry; config: GlobalConfig; onMutation: () => void })`
    - `listen(sockPath: string): Promise<void>` (unlinks stale socket first), `close(): Promise<void>`, `broadcast(): void` — pushes current snapshot to subscribers.
    - Methods: `ping` → `"pong"`; `state` → snapshot; `enqueue {prompt, repo, ref?, priority?}` → created task (ref default `"temp"`); `definitions` → `{repo, name, args, hasDiscovery, description?}` across config projects (via `listDefinitions`, per-project failures skipped); `runDefinition {repo, name, args?}` → created tasks (uses `instantiateDefinition`, trigger = args-mode when `args` non-empty else discover); `retry {id}` → task re-queued (`status: "queued"`, `error: null`; only from `failed`/`needs-input`, else error); `skip {id}` → archive (only `failed`/`needs-input`/`done`); `setWorktree {id, worktree}` → sets `target.worktree` + re-queues (needs-input answer); `heartbeatInteractive {cwd, pid?}` → registers interactive session; `runMeta {id}` → run data.json or null; unknown method → `{error: "unknown method: X"}`.
    - Every mutating method calls `deps.onMutation()` (daemon wires this to `engine.tick()` + `broadcast()`).
  - `class ApiClient`: `connect(sockPath)`, `call(method, params?)` (Promise, 5s timeout), `subscribe(onState)`, `close()`.

- [ ] **Step 1: Write the failing test**

`packages/daemon/src/__tests__/api.test.ts`:

```ts
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
	makeRedactor,
	QueueStore,
	RunStore,
	SessionRegistry,
} from "@queohoh/core";
import type { Exec, GlobalConfig, ResolverIO, RunResult } from "@queohoh/core";
import { afterEach, describe, expect, it } from "vitest";
import { ApiClient } from "../client.js";
import { ApiServer } from "../api.js";
import { Engine } from "../engine.js";

const cleanups: (() => Promise<void> | void)[] = [];
afterEach(async () => {
	while (cleanups.length) await cleanups.pop()?.();
});

async function setup() {
	const base = mkdtempSync(join(tmpdir(), "qo-api-"));
	const repoPath = join(base, "repo");
	// definition fixture
	const defDir = join(repoPath, ".queohoh", "tasks", "greet");
	mkdirSync(defDir, { recursive: true });
	writeFileSync(join(defDir, "config.yaml"), "args: [name]\ndedup: none\n");
	writeFileSync(join(defDir, "prompt.md"), "Say hi to {{name}}.\n");

	const store = new QueueStore(join(base, "state"));
	const runStore = new RunStore(join(base, "runs"));
	const registry = new SessionRegistry(join(base, "sessions.json"));
	const config: GlobalConfig = {
		projects: [{ name: "platform", path: repoPath }],
		maxConcurrentTasks: 3,
		archiveAfterDays: 7,
		vars: {},
	};
	const okResult: RunResult = {
		exitCode: 0,
		timedOut: false,
		sessionId: null,
		resultText: "ok",
		stderr: "",
		usage: { costUsd: 0, turns: 1, durationMs: 1 },
	};
	const exec: Exec = async () => ({ stdout: "", exitCode: 0 });
	const resolverIO: ResolverIO = {
		listWorktrees: async () => [],
		prBranch: async () => null,
		spawnWorktree: async (_r, name) => ({ name, path: `/wt/${name}`, branch: name }),
	};
	const engine = new Engine({
		store,
		runStore,
		registry,
		config,
		resolverIO,
		exec,
		executeClaude: async () => okResult,
		redact: makeRedactor(new Map()),
	});
	let mutations = 0;
	const server = new ApiServer({
		engine,
		store,
		runStore,
		registry,
		config,
		onMutation: () => {
			mutations += 1;
		},
	});
	const sock = join(base, "d.sock");
	await server.listen(sock);
	const client = new ApiClient();
	await client.connect(sock);
	cleanups.push(() => client.close());
	cleanups.push(() => server.close());
	return { server, client, store, mutations: () => mutations };
}

describe("ApiServer", () => {
	it("ping/pong", async () => {
		const { client } = await setup();
		expect(await client.call("ping")).toBe("pong");
	});

	it("enqueue creates an adhoc task and reports state", async () => {
		const { client } = await setup();
		const task = (await client.call("enqueue", {
			prompt: "fix it",
			repo: "platform",
		})) as { id: string; target: { ref: string } };
		expect(task.target.ref).toBe("temp");
		const state = (await client.call("state")) as { tasks: { id: string }[] };
		expect(state.tasks.map((t) => t.id)).toContain(task.id);
	});

	it("definitions lists per-repo task definitions", async () => {
		const { client } = await setup();
		const defs = (await client.call("definitions")) as {
			repo: string;
			name: string;
			args: string[];
		}[];
		expect(defs).toEqual([
			{ repo: "platform", name: "greet", args: ["name"], hasDiscovery: false },
		]);
	});

	it("runDefinition with args instantiates", async () => {
		const { client } = await setup();
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "greet",
			args: ["world"],
		})) as { prompt: string }[];
		expect(created).toHaveLength(1);
		expect(created[0]?.prompt).toBe("Say hi to world.\n");
	});

	it("retry re-queues a failed task; skip archives it", async () => {
		const { client, store } = await setup();
		const t = store.create({ prompt: "p", repo: "platform", ref: "temp", source: "tui" });
		store.update(t.id, { status: "failed", error: "boom" });
		const retried = (await client.call("retry", { id: t.id })) as {
			status: string;
			error: null;
		};
		expect(retried.status).toBe("queued");
		store.update(t.id, { status: "failed", error: "boom again" });
		await client.call("skip", { id: t.id });
		expect(store.list()).toEqual([]);
	});

	it("retry rejects tasks that are not failed/needs-input", async () => {
		const { client, store } = await setup();
		const t = store.create({ prompt: "p", repo: "platform", ref: "temp", source: "tui" });
		await expect(client.call("retry", { id: t.id })).rejects.toThrow(
			/cannot retry/,
		);
	});

	it("setWorktree answers needs-input and re-queues", async () => {
		const { client, store } = await setup();
		const t = store.create({ prompt: "p", repo: "platform", ref: "worktree:gone", source: "tui" });
		store.update(t.id, { status: "needs-input", error: "not found" });
		const updated = (await client.call("setWorktree", {
			id: t.id,
			worktree: "main",
		})) as { status: string; target: { worktree: string } };
		expect(updated.status).toBe("queued");
		expect(updated.target.worktree).toBe("main");
	});

	it("unknown method errors", async () => {
		const { client } = await setup();
		await expect(client.call("nope")).rejects.toThrow("unknown method: nope");
	});

	it("subscribe pushes state on broadcast", async () => {
		const { server, client } = await setup();
		const states: unknown[] = [];
		await client.subscribe((s) => states.push(s));
		server.broadcast();
		await new Promise((r) => setTimeout(r, 100));
		expect(states.length).toBeGreaterThanOrEqual(1);
	});
});
```

- [ ] **Step 2: Run to verify failure, then implement server**

`packages/daemon/src/api.ts`:

```ts
import { existsSync, unlinkSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";
import type {
	GlobalConfig,
	QueueStore,
	RunStore,
	SessionEntry,
	SessionRegistry,
	TaskInstance,
} from "@queohoh/core";
import {
	defaultExec,
	instantiateDefinition,
	listDefinitions,
	loadDefinition,
	loadRepoConfig,
} from "@queohoh/core";
import type { Engine } from "./engine.js";

export interface StateSnapshot {
	tasks: TaskInstance[];
	archivedRecent: TaskInstance[];
	sessions: SessionEntry[];
	running: string[];
}

interface ApiDeps {
	engine: Engine;
	store: QueueStore;
	runStore: RunStore;
	registry: SessionRegistry;
	config: GlobalConfig;
	onMutation: () => void;
}

export class ApiServer {
	private server: Server | null = null;
	private subscribers = new Set<Socket>();

	constructor(private readonly deps: ApiDeps) {}

	snapshot(): StateSnapshot {
		return {
			tasks: this.deps.store.list(),
			archivedRecent: this.deps.store.listArchived().slice(-20),
			sessions: this.deps.registry.list(),
			running: this.deps.engine.runningTaskIds(),
		};
	}

	broadcast(): void {
		const frame = `${JSON.stringify({ event: "state", data: this.snapshot() })}\n`;
		for (const sock of this.subscribers) {
			sock.write(frame);
		}
	}

	listen(sockPath: string): Promise<void> {
		if (existsSync(sockPath)) unlinkSync(sockPath);
		this.server = createServer((socket) => this.handleConnection(socket));
		return new Promise((resolve, reject) => {
			this.server?.once("error", reject);
			this.server?.listen(sockPath, () => resolve());
		});
	}

	close(): Promise<void> {
		for (const sock of this.subscribers) sock.destroy();
		this.subscribers.clear();
		return new Promise((resolve) => {
			this.server ? this.server.close(() => resolve()) : resolve();
		});
	}

	private handleConnection(socket: Socket): void {
		let buffer = "";
		socket.on("data", async (chunk) => {
			buffer += chunk.toString();
			const lines = buffer.split("\n");
			buffer = lines.pop() ?? "";
			for (const line of lines) {
				if (!line.trim()) continue;
				await this.handleLine(socket, line);
			}
		});
		socket.on("close", () => this.subscribers.delete(socket));
		socket.on("error", () => this.subscribers.delete(socket));
	}

	private async handleLine(socket: Socket, line: string): Promise<void> {
		let req: { id?: unknown; method?: unknown; params?: unknown };
		try {
			req = JSON.parse(line);
		} catch {
			socket.write(`${JSON.stringify({ id: null, error: "bad json" })}\n`);
			return;
		}
		const id = req.id ?? null;
		try {
			const result = await this.dispatch(
				String(req.method),
				(req.params ?? {}) as Record<string, unknown>,
				socket,
			);
			socket.write(`${JSON.stringify({ id, result })}\n`);
		} catch (err) {
			socket.write(
				`${JSON.stringify({ id, error: err instanceof Error ? err.message : String(err) })}\n`,
			);
		}
	}

	private async dispatch(
		method: string,
		params: Record<string, unknown>,
		socket: Socket,
	): Promise<unknown> {
		const { deps } = this;
		switch (method) {
			case "ping":
				return "pong";
			case "state":
				return this.snapshot();
			case "subscribe":
				this.subscribers.add(socket);
				return true;
			case "enqueue": {
				const task = deps.store.create({
					prompt: String(params.prompt ?? ""),
					repo: String(params.repo ?? ""),
					ref: String(params.ref ?? "temp"),
					source: "mcp",
					priority: (params.priority as "low" | "normal" | "high") ?? "normal",
				});
				deps.onMutation();
				return task;
			}
			case "definitions": {
				const out: {
					repo: string;
					name: string;
					args: string[];
					hasDiscovery: boolean;
				}[] = [];
				for (const project of deps.config.projects) {
					try {
						for (const def of listDefinitions(project.path, project.name)) {
							out.push({
								repo: project.name,
								name: def.name,
								args: def.args,
								hasDiscovery: def.discovery !== null,
							});
						}
					} catch {}
				}
				return out;
			}
			case "runDefinition": {
				const repo = String(params.repo ?? "");
				const name = String(params.name ?? "");
				const project = deps.config.projects.find((p) => p.name === repo);
				if (!project) throw new Error(`unknown repo: ${repo}`);
				const def = loadDefinition(project.path, repo, name);
				const args = (params.args as string[] | undefined) ?? [];
				const created = await instantiateDefinition(
					def,
					args.length > 0
						? { mode: "args", values: args.map(String) }
						: { mode: "discover" },
					{
						store: deps.store,
						exec: defaultExec,
						repoPath: project.path,
						source: "tui",
						globalVars: deps.config.vars,
						repoVars: loadRepoConfig(project.path).vars,
					},
				);
				deps.onMutation();
				return created;
			}
			case "retry": {
				const task = this.mustGet(String(params.id));
				if (task.status !== "failed" && task.status !== "needs-input") {
					throw new Error(`cannot retry task in status ${task.status}`);
				}
				const updated = deps.store.update(task.id, {
					status: "queued",
					error: null,
				});
				deps.onMutation();
				return updated;
			}
			case "skip": {
				const task = this.mustGet(String(params.id));
				if (!["failed", "needs-input", "done"].includes(task.status)) {
					throw new Error(`cannot skip task in status ${task.status}`);
				}
				deps.store.archive(task.id);
				deps.onMutation();
				return true;
			}
			case "setWorktree": {
				const task = this.mustGet(String(params.id));
				const updated = deps.store.update(task.id, {
					status: "queued",
					error: null,
					target: { ...task.target, worktree: String(params.worktree) },
				});
				deps.onMutation();
				return updated;
			}
			case "heartbeatInteractive": {
				deps.registry.upsertInteractive(
					String(params.cwd),
					typeof params.pid === "number" ? params.pid : null,
				);
				return true;
			}
			case "runMeta":
				return deps.runStore.readRunMeta(String(params.id));
			default:
				throw new Error(`unknown method: ${method}`);
		}
	}

	private mustGet(id: string): TaskInstance {
		const task = this.deps.store.get(id);
		if (!task) throw new Error(`task not found: ${id}`);
		return task;
	}
}
```

`packages/daemon/src/client.ts`:

```ts
import { connect, type Socket } from "node:net";

export class ApiClient {
	private socket: Socket | null = null;
	private nextId = 1;
	private pending = new Map<
		number,
		{ resolve: (v: unknown) => void; reject: (e: Error) => void }
	>();
	private onState: ((state: unknown) => void) | null = null;

	connect(sockPath: string): Promise<void> {
		return new Promise((resolve, reject) => {
			const socket = connect(sockPath);
			this.socket = socket;
			let buffer = "";
			socket.once("connect", () => resolve());
			socket.once("error", (err) => reject(err));
			socket.on("data", (chunk) => {
				buffer += chunk.toString();
				const lines = buffer.split("\n");
				buffer = lines.pop() ?? "";
				for (const line of lines) {
					if (!line.trim()) continue;
					this.handleFrame(line);
				}
			});
		});
	}

	private handleFrame(line: string): void {
		let frame: Record<string, unknown>;
		try {
			frame = JSON.parse(line);
		} catch {
			return;
		}
		if (frame.event === "state") {
			this.onState?.(frame.data);
			return;
		}
		const id = frame.id as number;
		const pending = this.pending.get(id);
		if (!pending) return;
		this.pending.delete(id);
		if (frame.error !== undefined) {
			pending.reject(new Error(String(frame.error)));
		} else {
			pending.resolve(frame.result);
		}
	}

	call(method: string, params?: Record<string, unknown>): Promise<unknown> {
		const socket = this.socket;
		if (!socket) return Promise.reject(new Error("not connected"));
		const id = this.nextId++;
		return new Promise((resolve, reject) => {
			const timer = setTimeout(() => {
				this.pending.delete(id);
				reject(new Error(`call timed out: ${method}`));
			}, 5000);
			this.pending.set(id, {
				resolve: (v) => {
					clearTimeout(timer);
					resolve(v);
				},
				reject: (e) => {
					clearTimeout(timer);
					reject(e);
				},
			});
			socket.write(`${JSON.stringify({ id, method, params })}\n`);
		});
	}

	async subscribe(onState: (state: unknown) => void): Promise<void> {
		this.onState = onState;
		await this.call("subscribe");
	}

	close(): void {
		this.socket?.destroy();
		this.socket = null;
	}
}
```

Run: `pnpm -F @queohoh/daemon test` — PASS.

- [ ] **Step 3: Commit**

```bash
git add packages/daemon/src/api.ts packages/daemon/src/client.ts packages/daemon/src/__tests__/api.test.ts
git commit -m "feat(daemon): unix-socket ndjson API server and client"
```

---

### Task 11: Daemon entrypoint, lock, watcher, launchd + CLI

**Files:**
- Create: `packages/daemon/src/daemon.ts`, `packages/daemon/src/lock.ts`, `packages/daemon/src/launchd.ts`, `packages/daemon/src/cli.ts`
- Test: `packages/daemon/src/__tests__/lock.test.ts`, `packages/daemon/src/__tests__/launchd.test.ts`

**Interfaces:**
- Consumes: everything prior.
- Produces:
  - `acquireLock(pidFile: string, opts?: { isPidAlive?: (pid: number) => boolean }): boolean` — writes own pid; returns false if file holds a live pid; stale (dead pid / garbage) is overwritten. `releaseLock(pidFile)`.
  - `launchdPlist(opts: { label: string; nodeBin: string; cliPath: string; logPath: string }): string` — returns plist XML with `KeepAlive: true`, `RunAtLoad: true`, `ProgramArguments: [nodeBin, cliPath, "daemon"]`, stdout/stderr to logPath. Pure string builder, snapshot-tested.
  - `startDaemon(): Promise<{ stop: () => Promise<void> }>` (in `daemon.ts`) — wires everything: paths → `loadGlobalConfig` (missing config → create a commented starter file and continue with empty projects) → stores/registry/engine/api server → `fs.watch` on tasks dir (debounced 250ms → tick+broadcast) → 2s interval tick (unref'd) → engine `onChange` → broadcast. Acquires lock (exit code 1 with message if already running). SIGTERM/SIGINT → graceful stop (close server, release lock).
  - `cli.ts` (commander): `queohoh daemon` (foreground `startDaemon`), `queohoh launchd:install` (writes plist to `~/Library/LaunchAgents/com.queohoh.daemon.plist`, prints `launchctl bootstrap` hint — does NOT run launchctl), `queohoh launchd:uninstall` (removes plist, prints `launchctl bootout` hint), `queohoh status` (connects client, prints `state` summary as JSON).
  - `daemon.ts`/`cli.ts` are thin wiring — covered by lock/launchd unit tests + existing engine/api tests; no dedicated integration test in this plan (Plan D adds an end-to-end smoke).

- [ ] **Step 1: Write failing lock + launchd tests**

`packages/daemon/src/__tests__/lock.test.ts`:

```ts
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { acquireLock, releaseLock } from "../lock.js";

const pidFile = () => join(mkdtempSync(join(tmpdir(), "qo-lock-")), "d.pid");

describe("acquireLock", () => {
	it("acquires and writes own pid", () => {
		const path = pidFile();
		expect(acquireLock(path)).toBe(true);
		expect(readFileSync(path, "utf-8").trim()).toBe(String(process.pid));
	});

	it("refuses when a live pid holds the lock", () => {
		const path = pidFile();
		writeFileSync(path, "99999");
		expect(acquireLock(path, { isPidAlive: () => true })).toBe(false);
	});

	it("steals a stale lock (dead pid)", () => {
		const path = pidFile();
		writeFileSync(path, "99999");
		expect(acquireLock(path, { isPidAlive: () => false })).toBe(true);
	});

	it("steals a garbage lock file", () => {
		const path = pidFile();
		writeFileSync(path, "not-a-pid");
		expect(acquireLock(path)).toBe(true);
	});

	it("releaseLock removes the file", () => {
		const path = pidFile();
		acquireLock(path);
		releaseLock(path);
		expect(acquireLock(path)).toBe(true);
	});
});
```

`packages/daemon/src/__tests__/launchd.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { launchdPlist } from "../launchd.js";

describe("launchdPlist", () => {
	it("renders KeepAlive plist with program arguments", () => {
		const xml = launchdPlist({
			label: "com.queohoh.daemon",
			nodeBin: "/usr/local/bin/node",
			cliPath: "/opt/queohoh/cli.js",
			logPath: "/tmp/queohoh.log",
		});
		expect(xml).toContain("<key>Label</key>");
		expect(xml).toContain("<string>com.queohoh.daemon</string>");
		expect(xml).toContain("<key>KeepAlive</key>");
		expect(xml).toContain("<true/>");
		expect(xml).toContain("<string>/usr/local/bin/node</string>");
		expect(xml).toContain("<string>/opt/queohoh/cli.js</string>");
		expect(xml).toContain("<string>daemon</string>");
		expect(xml).toContain("<string>/tmp/queohoh.log</string>");
	});
});
```

- [ ] **Step 2: Implement lock + launchd after seeing failures**

`packages/daemon/src/lock.ts`:

```ts
import {
	existsSync,
	mkdirSync,
	readFileSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { dirname } from "node:path";

function defaultIsPidAlive(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch {
		return false;
	}
}

export function acquireLock(
	pidFile: string,
	opts?: { isPidAlive?: (pid: number) => boolean },
): boolean {
	const isPidAlive = opts?.isPidAlive ?? defaultIsPidAlive;
	if (existsSync(pidFile)) {
		const raw = readFileSync(pidFile, "utf-8").trim();
		const pid = Number(raw);
		if (Number.isInteger(pid) && pid > 0 && isPidAlive(pid)) {
			return false;
		}
	}
	mkdirSync(dirname(pidFile), { recursive: true });
	writeFileSync(pidFile, String(process.pid));
	return true;
}

export function releaseLock(pidFile: string): void {
	try {
		unlinkSync(pidFile);
	} catch {}
}
```

`packages/daemon/src/launchd.ts`:

```ts
export function launchdPlist(opts: {
	label: string;
	nodeBin: string;
	cliPath: string;
	logPath: string;
}): string {
	return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>${opts.label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>${opts.nodeBin}</string>
		<string>${opts.cliPath}</string>
		<string>daemon</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>StandardOutPath</key>
	<string>${opts.logPath}</string>
	<key>StandardErrorPath</key>
	<string>${opts.logPath}</string>
</dict>
</plist>
`;
}
```

Run: `pnpm -F @queohoh/daemon test` — PASS.

- [ ] **Step 3: Implement daemon wiring + CLI (no new test; typecheck gate)**

`packages/daemon/src/daemon.ts`:

```ts
import { existsSync, mkdirSync, watch, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import {
	buildSecretMap,
	createResolverIO,
	defaultExec,
	executeClaude,
	loadGlobalConfig,
	makeRedactor,
	QueueStore,
	RunStore,
	SessionRegistry,
} from "@queohoh/core";
import { ApiServer } from "./api.js";
import { Engine } from "./engine.js";
import { acquireLock, releaseLock } from "./lock.js";
import {
	configPath,
	pidPath,
	runsPath,
	sessionsPath,
	socketPath,
	statePath,
} from "./paths.js";

const STARTER_CONFIG = `# queohoh global config
# projects:
#   - name: platform
#     path: ~/workspace/platform
# max_concurrent_tasks: 3
# archive_after_days: 7
# vars: {}
`;

export async function startDaemon(): Promise<{ stop: () => Promise<void> }> {
	const state = statePath();
	const cfgPath = configPath();
	if (!existsSync(cfgPath)) {
		mkdirSync(dirname(cfgPath), { recursive: true });
		writeFileSync(cfgPath, STARTER_CONFIG);
		console.log(`created starter config at ${cfgPath}`);
	}
	const config = loadGlobalConfig(cfgPath);

	const pid = pidPath(state);
	if (!acquireLock(pid)) {
		console.error("queohoh daemon already running");
		process.exit(1);
	}

	const store = new QueueStore(state);
	const runStore = new RunStore(runsPath(state));
	const registry = new SessionRegistry(sessionsPath(state));
	const redact = makeRedactor(buildSecretMap(process.env));
	const resolverIO = createResolverIO(defaultExec);

	const server = new ApiServer({
		engine: null as unknown as Engine, // set below (circular wiring)
		store,
		runStore,
		registry,
		config,
		onMutation: () => {
			void engine.tick().then(() => server.broadcast());
		},
	});

	const engine = new Engine({
		store,
		runStore,
		registry,
		config,
		resolverIO,
		exec: defaultExec,
		executeClaude,
		redact,
		onChange: () => server.broadcast(),
	});
	// biome-ignore lint/suspicious/noExplicitAny: late-bind circular dep
	(server as any).deps.engine = engine;

	await server.listen(socketPath(state));

	// Watch the tasks dir — a dropped file IS an enqueue.
	let debounce: NodeJS.Timeout | null = null;
	const watcher = watch(join(state, "tasks"), () => {
		if (debounce) clearTimeout(debounce);
		debounce = setTimeout(() => {
			void engine.tick().then(() => server.broadcast());
		}, 250);
	});

	const interval = setInterval(() => {
		void engine.tick().then(() => server.broadcast());
	}, 2000);
	interval.unref();

	await engine.tick();
	console.log(`queohoh daemon up — socket ${socketPath(state)}`);

	const stop = async () => {
		watcher.close();
		clearInterval(interval);
		await server.close();
		releaseLock(pid);
	};
	process.on("SIGTERM", () => void stop().then(() => process.exit(0)));
	process.on("SIGINT", () => void stop().then(() => process.exit(0)));
	return { stop };
}
```

Note: the `(server as any).deps.engine = engine` late-bind is ugly; the implementer SHOULD restructure to avoid it if a cleaner shape is evident (e.g. `server.setEngine(engine)` or constructing Engine first with `onChange` late-bound instead) — cleanliness of this wiring is reviewer-visible. Prefer: construct Engine first with a `let broadcastRef` closure.

`packages/daemon/src/cli.ts`:

```ts
#!/usr/bin/env node
import { existsSync, mkdirSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { Command } from "commander";
import { ApiClient } from "./client.js";
import { startDaemon } from "./daemon.js";
import { launchdPlist } from "./launchd.js";
import { socketPath, statePath } from "./paths.js";

const PLIST_PATH = join(
	homedir(),
	"Library/LaunchAgents/com.queohoh.daemon.plist",
);

const program = new Command();
program.name("queohoh").description("queohoh orchestrator daemon");

program.command("daemon").description("run the daemon in the foreground").action(async () => {
	await startDaemon();
});

program.command("status").description("print daemon state").action(async () => {
	const client = new ApiClient();
	try {
		await client.connect(socketPath(statePath()));
		const state = await client.call("state");
		console.log(JSON.stringify(state, null, 2));
	} catch {
		console.error("daemon not reachable");
		process.exitCode = 1;
	} finally {
		client.close();
	}
});

program
	.command("launchd:install")
	.description("write the launchd KeepAlive plist")
	.action(() => {
		mkdirSync(join(homedir(), "Library/LaunchAgents"), { recursive: true });
		const cliPath = new URL(import.meta.url).pathname;
		writeFileSync(
			PLIST_PATH,
			launchdPlist({
				label: "com.queohoh.daemon",
				nodeBin: process.execPath,
				cliPath,
				logPath: join(statePath(), "daemon/daemon.log"),
			}),
		);
		console.log(`wrote ${PLIST_PATH}`);
		console.log(
			`activate: launchctl bootstrap gui/$(id -u) ${PLIST_PATH}`,
		);
	});

program
	.command("launchd:uninstall")
	.description("remove the launchd plist")
	.action(() => {
		if (existsSync(PLIST_PATH)) unlinkSync(PLIST_PATH);
		console.log(
			`removed. deactivate: launchctl bootout gui/$(id -u)/com.queohoh.daemon`,
		);
	});

program.parseAsync();
```

Run: `pnpm -F @queohoh/daemon typecheck && pnpm -F @queohoh/daemon test && pnpm -F @queohoh/core test`
Expected: all green.

- [ ] **Step 4: Full suite + lint, commit**

```bash
mise x node@22 -- pnpm lint
pnpm test && pnpm typecheck
git add -A
git commit -m "feat(daemon): entrypoint with lock, fs watcher, launchd plist, and CLI"
```

---

## Self-Review Notes

- **Spec coverage (Plan B scope):** redaction (T1), discovery/dedup incl. archive dedup (T2), instantiation with all 3 trigger shapes minus cron (T3), runner with events/transcript/cost/session/timeout/redaction (T4), run persistence layout + config snapshot + worker pid (T5), hooks with finally semantics + worker completion contract (T6), session registry + interactive awareness (T7), daemon package + paths (T8), engine tick/resolve/start/orphan/auto-archive/laneOfCwd (T9), socket API + client with all queue-management verbs the TUI needs (T10), lock/launchd/daemon wiring/CLI (T11). Deferred: cron triggers (slice 2), MCP + /qoo (Plan C), TUI (Plan D), CC interactive hook script (Plan C — it needs the MCP/CLI surface).
- **Type consistency:** `Exec` reused from resolver-io for discovery/hooks/worker git checks; `ClaudeExecutor = typeof executeClaude` keeps worker/engine/api aligned; `StateSnapshot` shared server→client.
- **Placeholder scan:** clean — every step has full code and exact commands. The one intentional freedom: Task 11's circular-wiring note explicitly invites the implementer to restructure.
