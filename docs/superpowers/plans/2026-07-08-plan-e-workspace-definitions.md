# queohoh Plan E — Workspace-Based Definitions + agent247 pr-review Port

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move task definitions out of work repos into the user's workspace (`<workspace>/<project>/tasks/<name>/`, agent247-style), then port the real agent247 `pr-review` task (prompt + discover.sh verbatim, config translated) so it can be kicked off through the queue and picked up by headless `claude -p`.

**Architecture:** Global config gains a `workspace:` root (default `~/.config/queohoh`). A project's queohoh home is `<workspace>/<project-name>/` holding `tasks/` and optional `vars.yaml`. Definitions never live in work repos; `loadRepoConfig` (which read `<repo>/.queohoh/config.yaml`) is replaced by `loadProjectVars`. Discovery and hooks that reference relative paths (e.g. `bash tasks/pr-review/discover.sh`) run with cwd = the project workspace dir — exactly agent247's semantics. Queue and runs stay central (deferred split).

**Spec:** amended 2026-07-08 in `docs/superpowers/specs/2026-07-08-queohoh-slice1-design.md` (Project model section).

## Global Constraints

- Node >= 22, TS strict ESM. Lint via `mise x node@22 -- pnpm lint`; `pnpm lint:ci` must exit 0 with 0 warnings. No Co-Authored-By trailers.
- All zod schemas on user files `.strict()`.
- `workspace` path tilde-expands like project paths.
- Definition folder layout: `<workspace>/<project>/tasks/<name>/{config.yaml,prompt.md,…scripts}`.
- Per-project vars: `<workspace>/<project>/vars.yaml` — flat map, values stringified, `{}` when absent. Var precedence unchanged: global < project < item.
- Discovery commands run with cwd = `<workspace>/<project>` (NOT the repo). Repo-touching discovery uses absolute paths from vars (agent247 pattern).
- `pre_run`/`post_run` still run inside the resolved WORKTREE (unchanged — the resolver owns worktree setup).
- Hermetic tests: tui vitest aliases already map workspace deps to src; keep every suite green without prebuilt dist.

---

### Task 1: Core + daemon rewiring to workspace definitions

**Files:**
- Modify: `packages/core/src/config.ts` (add `workspace`, replace `loadRepoConfig` with `loadProjectVars`)
- Modify: `packages/core/src/definition.ts` (project-dir based paths)
- Modify: `packages/core/src/instantiate.ts` (rename `repoPath` → `cwd`)
- Modify: `packages/core/src/index.ts` (barrel: swap `loadRepoConfig` → `loadProjectVars`, export `projectWorkspaceDir`)
- Modify: `packages/daemon/src/api.ts`, `packages/daemon/src/engine.ts`, `packages/daemon/src/daemon.ts` (starter config comment)
- Test: update `packages/core/src/__tests__/{config,definition,instantiate}.test.ts`, `packages/daemon/src/__tests__/{api,engine}.test.ts`, `packages/tui/src/__tests__/helpers.ts` (+ any GlobalConfig literals)

**Interfaces:**
- Consumes: existing core/daemon surface.
- Produces:
  - `GlobalConfig` gains `workspace: string` (yaml key `workspace`, default `"~/.config/queohoh"`, tilde-expanded).
  - `projectWorkspaceDir(config: GlobalConfig, projectName: string): string` — `join(config.workspace, projectName)`.
  - `loadProjectVars(projectDir: string): Record<string, string>` — reads `<projectDir>/vars.yaml`, `{}` if absent, values `String()`-coerced, `.strict()` not applicable (flat record; reject non-scalar values with a clear error).
  - `loadDefinition(projectDir: string, repoName: string, taskName: string)` / `listDefinitions(projectDir: string, repoName: string)` — read `<projectDir>/tasks/<name>/…` (was `<repoPath>/.queohoh/tasks/`). Same `TaskDefinition` shape.
  - `InstantiateDeps.repoPath` renamed to `cwd` (discovery working directory).
  - `loadRepoConfig` + `RepoConfig` DELETED (barrel too).
  - api.ts: `definitions` and `runDefinition` resolve via `projectWorkspaceDir(config, project.name)`; `runDefinition` passes `cwd: projectWorkspaceDir(...)`, `repoVars: loadProjectVars(projectWorkspaceDir(...))`.
  - engine.ts `loadDef`: `loadDefinition(projectWorkspaceDir(this.deps.config, repo), repo, name)` (repo must still be a configured project name; unknown → null).

- [ ] **Step 1: config.ts — failing tests first**

Replace the `loadRepoConfig` describe-block in `packages/core/src/__tests__/config.test.ts` with:

```ts
describe("workspace", () => {
	it("defaults workspace to ~/.config/queohoh and expands tilde", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-"));
		const path = join(dir, "config.yaml");
		writeFileSync(path, "projects: []\n");
		expect(loadGlobalConfig(path).workspace).toBe(
			join(homedir(), ".config/queohoh"),
		);
		writeFileSync(path, "workspace: ~/workspace/queohoh\nprojects: []\n");
		expect(loadGlobalConfig(path).workspace).toBe(
			join(homedir(), "workspace/queohoh"),
		);
	});

	it("projectWorkspaceDir joins workspace and project name", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-"));
		const path = join(dir, "config.yaml");
		writeFileSync(path, "workspace: /ws\nprojects: []\n");
		const config = loadGlobalConfig(path);
		expect(projectWorkspaceDir(config, "platform")).toBe("/ws/platform");
	});
});

describe("loadProjectVars", () => {
	it("reads and stringifies vars.yaml", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pv-"));
		writeFileSync(join(dir, "vars.yaml"), "repo: justicebid/platform\nport: 3000\n");
		expect(loadProjectVars(dir)).toEqual({
			repo: "justicebid/platform",
			port: "3000",
		});
	});

	it("returns {} when absent and rejects non-scalar values", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pv-"));
		expect(loadProjectVars(dir)).toEqual({});
		writeFileSync(join(dir, "vars.yaml"), "nested:\n  a: 1\n");
		expect(() => loadProjectVars(dir)).toThrow(/non-scalar var: nested/);
	});
});
```

Implement in `packages/core/src/config.ts`: add to `GlobalConfigSchema` `workspace: z.string().default("~/.config/queohoh")`; in `loadGlobalConfig` return `workspace: expandTilde(config.workspace)`. Add:

```ts
export function projectWorkspaceDir(
	config: GlobalConfig,
	projectName: string,
): string {
	return join(config.workspace, projectName);
}

export function loadProjectVars(projectDir: string): Record<string, string> {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return {};
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
		throw new Error(`vars.yaml is not a mapping: ${path}`);
	}
	const vars: Record<string, string> = {};
	for (const [key, value] of Object.entries(raw)) {
		if (value !== null && typeof value === "object") {
			throw new Error(`non-scalar var: ${key}`);
		}
		vars[key] = String(value);
	}
	return vars;
}
```

Delete `loadRepoConfig`, `RepoConfigSchema`, `RepoConfig`. Run config tests green.

- [ ] **Step 2: definition.ts — path change (tests first)**

In `packages/core/src/__tests__/definition.test.ts` change the fixture helper: definitions are written to `join(projectDir, "tasks", name)` where `projectDir = mkdtempSync(...)` directly (drop the `.queohoh` segment). Rename helper param/vars from `repo` to `projectDir` for clarity. Update `loadDefinition`/`listDefinitions` call sites in the test accordingly.

Implement: in `packages/core/src/definition.ts` change

```ts
function tasksDir(projectDir: string): string {
	return join(projectDir, "tasks");
}
```

and rename the first param of `loadDefinition`/`listDefinitions` from `repoPath` to `projectDir` (behavior otherwise identical). Run green.

- [ ] **Step 3: instantiate.ts rename (tests updated in lockstep)**

Rename `InstantiateDeps.repoPath` → `cwd` (it is only used as `discoverItems(..., { cwd: deps.cwd })`). Update `instantiate.test.ts` deps literal. Run green.

- [ ] **Step 4: barrel + daemon rewiring**

`packages/core/src/index.ts`: remove `loadRepoConfig`/`RepoConfig` exports; add `loadProjectVars`, `projectWorkspaceDir`.

`packages/daemon/src/api.ts`:
- `definitions` case: `listDefinitions(projectWorkspaceDir(deps.config, project.name), project.name)`.
- `runDefinition` case: `const projectDir = projectWorkspaceDir(deps.config, repo);` → `loadDefinition(projectDir, repo, name)` → `instantiateDefinition(def, trigger, { store, exec: defaultExec, cwd: projectDir, source, globalVars: deps.config.vars, repoVars: loadProjectVars(projectDir) })`. (Keep the `unknown repo` guard on `deps.config.projects` — the project must still be registered so the resolver knows the repo path.)

`packages/daemon/src/engine.ts` `loadDef`:

```ts
loadDef: (definition) => {
	const [repo, ...nameParts] = definition.split("/");
	const name = nameParts.join("/");
	if (!repo || !this.repoPath(repo)) return null;
	try {
		return loadDefinition(
			projectWorkspaceDir(this.deps.config, repo),
			repo,
			name,
		);
	} catch {
		return null;
	}
},
```

`packages/daemon/src/daemon.ts` STARTER_CONFIG comment: add `# workspace: ~/workspace/queohoh` line.

Update test fixtures: `packages/daemon/src/__tests__/api.test.ts` — the `greet` definition fixture moves from `<repoPath>/.queohoh/tasks/greet` to `<workspace>/platform/tasks/greet` where the test config gains `workspace: join(base, "ws")`; `engine.test.ts` + `packages/tui/src/__tests__/helpers.ts` — add `workspace: join(base, "ws")` to their GlobalConfig literals (any dir works; engine tests don't load definitions from disk except via loadDef-miss paths). Search the repo for remaining `GlobalConfig` literals and `.queohoh` references in tests and fix all.

- [ ] **Step 5: full gate + commit**

Run: `pnpm test && pnpm typecheck && mise x node@22 -- pnpm lint && pnpm lint:ci`
Expected: all green, 0 lint warnings.

```bash
git add -A
git commit -m "feat: workspace-based task definitions (<workspace>/<project>/tasks), retire per-repo .queohoh"
```

Also update `docs/setup.md`: replace the "Per-repo task definitions live in `<repo>/.queohoh/tasks/...`" sentence with the workspace layout (`workspace: ~/workspace/queohoh` in config; definitions at `<workspace>/<project>/tasks/<name>/`; optional `<workspace>/<project>/vars.yaml`), then amend the commit or make a second docs commit.

---

### Task 2: Port the real agent247 pr-review task + integration test

**Files:**
- Create (OUTSIDE the repo, in the user's workspace): `~/workspace/queohoh/platform/tasks/pr-review/{config.yaml,prompt.md,discover.sh}` and `~/workspace/queohoh/platform/vars.yaml`
- Create (in repo): `packages/daemon/src/__tests__/pr-review-shape.test.ts` — integration test with an in-tmp replica of the ported task proving the full instantiation path.

**Interfaces:**
- Consumes: Task 1 surface.
- Produces: a kickable `platform/pr-review` definition + a regression test locking the shape.

- [ ] **Step 1: Port the task files (workspace)**

Source of truth: `/Users/noootown/workspace/agent247/tasks/pr-review/` and `/Users/noootown/workspace/agent247/vars.yaml`.

1. `mkdir -p ~/workspace/queohoh/platform/tasks/pr-review`
2. **prompt.md — copy VERBATIM** (`cp` the file; do not edit): the `{{number}}`/`{{title}}`/`{{url}}`/`{{total_changes}}`/`{{github_username}}`… placeholders all resolve from discovery items + vars.
3. **discover.sh — copy VERBATIM** + keep executable bit.
4. **config.yaml — translate** to queohoh schema (strict — agent247-only fields must not be carried over):

```yaml
# ported from agent247 tasks/pr-review — cron/cleanup fields return in slice 2
discovery:
  command: bash tasks/pr-review/discover.sh {{github_username}} {{platform_repo}} {{platform_repo_path}}
  item_key: "{{url}}"
dedup: skip_seen
worktree: "pr:{{number}}"
model: opus
timeout: 30m
priority: normal
```

   Dropped consciously: `schedule`/`cron_enabled` (slice 2), `cleanup` (later slice), `alternatives`/`url_template` (later), `parallel` (inherent — each PR is its own lane), `pre_run` setup-worktree.sh + `cwd` (replaced by the resolver: `pr:{{number}}` finds the worktree by branch or spawns via `wt` using the JUS-ticket convention), `requires_network` (dropped in queohoh).

5. `~/workspace/queohoh/platform/vars.yaml` — from agent247 `vars.yaml`, project-scoped keys:

```yaml
platform_repo: justicebid/platform
platform_repo_path: /Users/noootown/Downloads/projects/platform
bot_name: Ian's Bot
bot_signature: Automated review by Ian's Bot
```

   (`github_username`, `linear_*`, `knowledge_base_path` already live in the global config vars — verify `github_username: ianchiu-jb` is present there; item vars override globals if discovery ever emits a colliding key.)

- [ ] **Step 2: In-repo integration test (failing first)**

`packages/daemon/src/__tests__/pr-review-shape.test.ts` — replicates the ported shape in tmp and drives the REAL api path (`definitions` → `runDefinition` discover mode with a stubbed `gh`-free discover.sh):

```ts
import { chmodSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
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
import { ApiServer } from "../api.js";
import { ApiClient } from "../client.js";
import { Engine } from "../engine.js";

const cleanups: (() => Promise<void> | void)[] = [];
afterEach(async () => {
	while (cleanups.length) await cleanups.pop()?.();
});

const DISCOVER_SH = `#!/bin/bash
# stub of agent247 discover.sh — emits one PR shaped like the real script's output
echo '[{"number": 1423, "title": "Fix auth", "url": "https://github.com/justicebid/platform/pull/1423", "additions": 10, "deletions": 2, "headRefName": "JUS-1423-fix-auth", "baseRefName": "main", "author_login": "kevin", "total_changes": 12, "worktree_path": "/x"}]'
`;

const CONFIG_YAML = `discovery:
  command: bash tasks/pr-review/discover.sh {{github_username}} {{platform_repo}} {{platform_repo_path}}
  item_key: "{{url}}"
dedup: skip_seen
worktree: "pr:{{number}}"
model: opus
timeout: 30m
priority: normal
`;

const PROMPT_MD = `You are reviewing PR #{{number}} on {{platform_repo}} as {{github_username}}.
Title: {{title}} ({{total_changes}} changes)
`;

async function setup() {
	const base = mkdtempSync(join(tmpdir(), "qo-prshape-"));
	const repoPath = join(base, "repo");
	mkdirSync(repoPath, { recursive: true });
	const taskDir = join(base, "ws", "platform", "tasks", "pr-review");
	mkdirSync(taskDir, { recursive: true });
	writeFileSync(join(taskDir, "config.yaml"), CONFIG_YAML);
	writeFileSync(join(taskDir, "prompt.md"), PROMPT_MD);
	writeFileSync(join(taskDir, "discover.sh"), DISCOVER_SH);
	chmodSync(join(taskDir, "discover.sh"), 0o755);
	writeFileSync(
		join(base, "ws", "platform", "vars.yaml"),
		"platform_repo: justicebid/platform\nplatform_repo_path: /repo/path\n",
	);

	const store = new QueueStore(join(base, "state"));
	const runStore = new RunStore(join(base, "runs"));
	const registry = new SessionRegistry(join(base, "sessions.json"));
	const config: GlobalConfig = {
		projects: [{ name: "platform", path: repoPath }],
		workspace: join(base, "ws"),
		maxConcurrentTasks: 3,
		archiveAfterDays: 7,
		vars: { github_username: "ianchiu-jb" },
	};
	const okResult: RunResult = {
		exitCode: 0, timedOut: false, sessionId: null, resultText: "ok",
		stderr: "", usage: { costUsd: 0, turns: 1, durationMs: 1 },
	};
	const exec: Exec = async () => ({ stdout: "", exitCode: 0 });
	const resolverIO: ResolverIO = {
		listWorktrees: async () => [],
		prBranch: async () => null,
		spawnWorktree: async (_r, name) => ({ name, path: `/wt/${name}`, branch: name }),
	};
	const engine = new Engine({
		store, runStore, registry, config, resolverIO, exec,
		executeClaude: async () => okResult, redact: makeRedactor(new Map()),
	});
	const server = new ApiServer({
		engine, store, runStore, registry, config, onMutation: () => {},
	});
	const sock = join(base, "d.sock");
	await server.listen(sock);
	const client = new ApiClient();
	await client.connect(sock);
	cleanups.push(() => client.close());
	cleanups.push(() => server.close());
	return { client, store };
}

describe("agent247 pr-review port shape", () => {
	it("lists the definition from the workspace", async () => {
		const { client } = await setup();
		const defs = (await client.call("definitions")) as { repo: string; name: string; hasDiscovery: boolean }[];
		expect(defs).toEqual([
			{ repo: "platform", name: "pr-review", args: [], hasDiscovery: true },
		]);
	});

	it("runDefinition discovers via discover.sh (cwd = project workspace dir) and instantiates with rendered prompt, ref, key", async () => {
		const { client, store } = await setup();
		const created = (await client.call("runDefinition", {
			repo: "platform",
			name: "pr-review",
		})) as { prompt: string }[];
		expect(created).toHaveLength(1);
		const task = store.list()[0];
		expect(task?.definition).toBe("platform/pr-review");
		expect(task?.itemKey).toBe("https://github.com/justicebid/platform/pull/1423");
		expect(task?.target.ref).toBe("pr:1423");
		expect(task?.prompt).toContain("reviewing PR #1423 on justicebid/platform as ianchiu-jb");
		expect(task?.prompt).toContain("Fix auth (12 changes)");
		expect(task?.item?.headRefName).toBe("JUS-1423-fix-auth");
	});

	it("re-running dedups on url", async () => {
		const { client } = await setup();
		await client.call("runDefinition", { repo: "platform", name: "pr-review" });
		const second = (await client.call("runDefinition", {
			repo: "platform",
			name: "pr-review",
		})) as unknown[];
		expect(second).toEqual([]);
	});
});
```

Note the second test asserts the CRITICAL cwd semantics: `bash tasks/pr-review/discover.sh` only resolves if discovery runs in `<workspace>/platform/` — if cwd were wrong, discovery exits nonzero and the call errors.

- [ ] **Step 3: Run to green, port the real files, gate, commit**

Run: `pnpm -F @queohoh/daemon test` → green. Then execute Step 1's real-workspace port (cp the real files). Sanity: `bash -n ~/workspace/queohoh/platform/tasks/pr-review/discover.sh`.

Full gate: `pnpm test && pnpm typecheck && pnpm lint:ci`

```bash
git add packages/daemon/src/__tests__/pr-review-shape.test.ts
git commit -m "feat(daemon): pr-review port shape integration test"
```

(The workspace files live outside this repo — the user's workspace is its own git repo; leave committing there to the user/coordinator.)

---

## Self-Review Notes

- **Spec coverage:** workspace model (T1), definition relocation with agent247 cwd semantics (T1+T2 test), per-project vars (T1), pr-review port with conscious field-drop list (T2), dedup + prompt render + ref template verified end-to-end (T2). Deferred: per-project state/runs split, cron trigger for the ported task (slice 2), cleanup/alternatives.
- **Type consistency:** `InstantiateDeps.cwd` rename propagated to its only caller (api.ts) and test; `GlobalConfig.workspace` added to every literal (tests enumerate).
- **Placeholder scan:** clean.
