# queohoh Plan C — MCP Server, Heartbeat Hook & /qoo Skill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Claude Code sessions first-class access to the daemon: an MCP server exposing enqueue/list/run tools over the socket API, a `queohoh heartbeat` command + hook snippet for interactive-session awareness, and the `/qoo` skill that fuzzy-matches natural language onto task definitions.

**Architecture:** The MCP server is a thin stdio bridge inside `packages/daemon`: MCP tool call → `ApiClient` → unix socket → daemon. No new state, no new logic beyond translation. The tool layer is factored as a pure `McpToolDeps`-injected module so it tests without a real daemon or MCP transport. The heartbeat is one CLI subcommand. The skill is a markdown file plus install docs.

**Tech Stack:** `@modelcontextprotocol/sdk` (agent247's pattern), zod v4, existing `ApiClient`.

**Spec:** `docs/superpowers/specs/2026-07-08-queohoh-slice1-design.md` · **Builds on:** Plan B (`ApiClient`, socket methods: enqueue/state/definitions/runDefinition/heartbeatInteractive/runMeta).

## Global Constraints

- Node >= 22, TS strict ESM. Lint via `mise x node@22 -- pnpm lint`. No Co-Authored-By trailers.
- MCP tool results are `{ content: [{ type: "text", text: JSON.stringify(result, null, 2) }] }`; errors are returned as `{ content: [{type:"text", text: "error: <msg>"}], isError: true }` — MCP tools never throw.
- Tool names and shapes (spec contract): `enqueue_task(prompt, repo, ref?, priority?)`, `list_tasks()`, `list_task_definitions()`, `run_task_definition(repo, name, args?)`.
- The MCP process connects lazily per tool call (connect → call → close) so a daemon restart never wedges a long-lived session. Timeout errors surface as tool errors with the reminder that `runDefinition` with discovery is fire-and-verify-by-state.
- Attachment convention (spec): the CALLING session transcribes images/rich context into the task prompt before enqueueing — enforced by tool description text, not code.
- All interactive-session writes go through the daemon API (`heartbeatInteractive`) — never a second SessionRegistry (B7 invariant).

---

### Task 1: MCP tool layer (pure, injected)

**Files:**
- Create: `packages/daemon/src/mcp-tools.ts`
- Test: `packages/daemon/src/__tests__/mcp-tools.test.ts`

**Interfaces:**
- Consumes: nothing concrete — defines its own port.
- Produces:
  - `interface DaemonPort { call(method: string, params?: Record<string, unknown>): Promise<unknown> }` (structural subset of `ApiClient`)
  - `type McpCaller = () => Promise<{ port: DaemonPort; close: () => void }>` — lazy per-call connection factory.
  - `interface ToolResult { content: { type: "text"; text: string }[]; isError?: boolean }`
  - `mcpEnqueueTask(caller: McpCaller, args: { prompt: string; repo: string; ref?: string; priority?: "low" | "normal" | "high" }): Promise<ToolResult>`
  - `mcpListTasks(caller: McpCaller): Promise<ToolResult>` — returns the `state` snapshot's tasks + running ids.
  - `mcpListTaskDefinitions(caller: McpCaller): Promise<ToolResult>`
  - `mcpRunTaskDefinition(caller: McpCaller, args: { repo: string; name: string; args?: string[] }): Promise<ToolResult>`
  - All four: on any throw (connect failure, call timeout, server error) return `{content:[{type:"text",text:"error: <msg>"}], isError: true}`; always `close()` in finally.

- [ ] **Step 1: Write the failing test**

`packages/daemon/src/__tests__/mcp-tools.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { DaemonPort, McpCaller } from "../mcp-tools.js";
import {
	mcpEnqueueTask,
	mcpListTaskDefinitions,
	mcpListTasks,
	mcpRunTaskDefinition,
} from "../mcp-tools.js";

function fakeCaller(handler: (method: string, params?: Record<string, unknown>) => unknown) {
	const calls: { method: string; params?: Record<string, unknown> }[] = [];
	let closed = 0;
	const caller: McpCaller = async () => ({
		port: {
			call: async (method, params) => {
				calls.push({ method, params });
				return handler(method, params);
			},
		} satisfies DaemonPort,
		close: () => {
			closed += 1;
		},
	});
	return { caller, calls, closedCount: () => closed };
}

describe("mcpEnqueueTask", () => {
	it("calls enqueue and returns the task as text JSON", async () => {
		const { caller, calls, closedCount } = fakeCaller(() => ({ id: "01X", status: "queued" }));
		const result = await mcpEnqueueTask(caller, {
			prompt: "fix it",
			repo: "platform",
		});
		expect(calls).toEqual([
			{ method: "enqueue", params: { prompt: "fix it", repo: "platform", ref: undefined, priority: undefined } },
		]);
		expect(result.isError).toBeUndefined();
		expect(JSON.parse(result.content[0]?.text ?? "")).toEqual({ id: "01X", status: "queued" });
		expect(closedCount()).toBe(1);
	});

	it("maps failures to isError result and still closes", async () => {
		const { caller, closedCount } = fakeCaller(() => {
			throw new Error("daemon not reachable");
		});
		const result = await mcpEnqueueTask(caller, { prompt: "x", repo: "r" });
		expect(result.isError).toBe(true);
		expect(result.content[0]?.text).toContain("error: daemon not reachable");
		expect(closedCount()).toBe(1);
	});
});

describe("mcpListTasks", () => {
	it("returns tasks and running from state", async () => {
		const { caller } = fakeCaller(() => ({
			tasks: [{ id: "01A" }],
			archivedRecent: [],
			sessions: [],
			running: ["01A"],
		}));
		const result = await mcpListTasks(caller);
		const parsed = JSON.parse(result.content[0]?.text ?? "");
		expect(parsed.tasks).toEqual([{ id: "01A" }]);
		expect(parsed.running).toEqual(["01A"]);
	});
});

describe("mcpListTaskDefinitions", () => {
	it("passes through the definitions list", async () => {
		const { caller, calls } = fakeCaller(() => [
			{ repo: "platform", name: "pr-review", args: ["number"], hasDiscovery: true },
		]);
		const result = await mcpListTaskDefinitions(caller);
		expect(calls[0]?.method).toBe("definitions");
		expect(JSON.parse(result.content[0]?.text ?? "")).toHaveLength(1);
	});
});

describe("mcpRunTaskDefinition", () => {
	it("passes repo/name/args through", async () => {
		const { caller, calls } = fakeCaller(() => [{ id: "01B" }]);
		const result = await mcpRunTaskDefinition(caller, {
			repo: "platform",
			name: "pr-review",
			args: ["257"],
		});
		expect(calls).toEqual([
			{
				method: "runDefinition",
				params: { repo: "platform", name: "pr-review", args: ["257"] },
			},
		]);
		expect(JSON.parse(result.content[0]?.text ?? "")).toEqual([{ id: "01B" }]);
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/daemon test`
Expected: FAIL — cannot find module `../mcp-tools.js`.

- [ ] **Step 3: Implement**

`packages/daemon/src/mcp-tools.ts`:

```ts
export interface DaemonPort {
	call(method: string, params?: Record<string, unknown>): Promise<unknown>;
}

export type McpCaller = () => Promise<{
	port: DaemonPort;
	close: () => void;
}>;

export interface ToolResult {
	content: { type: "text"; text: string }[];
	isError?: boolean;
}

function ok(result: unknown): ToolResult {
	return { content: [{ type: "text", text: JSON.stringify(result, null, 2) }] };
}

function fail(err: unknown): ToolResult {
	const msg = err instanceof Error ? err.message : String(err);
	return { content: [{ type: "text", text: `error: ${msg}` }], isError: true };
}

async function withPort(
	caller: McpCaller,
	fn: (port: DaemonPort) => Promise<unknown>,
): Promise<ToolResult> {
	let close: (() => void) | null = null;
	try {
		const conn = await caller();
		close = conn.close;
		return ok(await fn(conn.port));
	} catch (err) {
		return fail(err);
	} finally {
		close?.();
	}
}

export function mcpEnqueueTask(
	caller: McpCaller,
	args: {
		prompt: string;
		repo: string;
		ref?: string;
		priority?: "low" | "normal" | "high";
	},
): Promise<ToolResult> {
	return withPort(caller, (port) =>
		port.call("enqueue", {
			prompt: args.prompt,
			repo: args.repo,
			ref: args.ref,
			priority: args.priority,
		}),
	);
}

export function mcpListTasks(caller: McpCaller): Promise<ToolResult> {
	return withPort(caller, async (port) => {
		const state = (await port.call("state")) as {
			tasks: unknown[];
			running: string[];
		};
		return { tasks: state.tasks, running: state.running };
	});
}

export function mcpListTaskDefinitions(caller: McpCaller): Promise<ToolResult> {
	return withPort(caller, (port) => port.call("definitions"));
}

export function mcpRunTaskDefinition(
	caller: McpCaller,
	args: { repo: string; name: string; args?: string[] },
): Promise<ToolResult> {
	return withPort(caller, (port) =>
		port.call("runDefinition", {
			repo: args.repo,
			name: args.name,
			args: args.args ?? [],
		}),
	);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/daemon test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/mcp-tools.ts packages/daemon/src/__tests__/mcp-tools.test.ts
git commit -m "feat(daemon): injected MCP tool layer"
```

---

### Task 2: MCP stdio server + CLI subcommands (mcp, heartbeat)

**Files:**
- Create: `packages/daemon/src/mcp.ts`
- Modify: `packages/daemon/src/cli.ts` (add `mcp` and `heartbeat` subcommands)
- Modify: `packages/daemon/package.json` (add `@modelcontextprotocol/sdk`)
- Test: typecheck gate + existing suite (stdio server wiring is thin; tool logic already tested in Task 1)

**Interfaces:**
- Consumes: Task 1 tool layer, `ApiClient`, `paths`.
- Produces:
  - `createMcpServer(caller: McpCaller): McpServer` — registers the four tools with zod schemas and descriptions. Descriptions must state: results are JSON; `run_task_definition` without args triggers discovery and may take a while — enqueue is fire-and-verify via `list_tasks`; the CALLER must transcribe any images/rich context into `prompt` text because workers never see the conversation.
  - `defaultCaller(): McpCaller` — `ApiClient` connect to `socketPath(statePath())`, `close()` = client.close.
  - CLI `queohoh mcp` — stdio transport, runs until stdin closes (register in Claude Code as an MCP server: `claude mcp add queohoh -- queohoh mcp`).
  - CLI `queohoh heartbeat [--cwd <dir>]` — one-shot: connects, calls `heartbeatInteractive {cwd: cwd ?? process.cwd(), pid: ppid}`, exits 0; daemon unreachable → exit 0 silently (heartbeats are best-effort, must never break a shell hook).

- [ ] **Step 1: Add dependency**

In `packages/daemon/package.json` dependencies add `"@modelcontextprotocol/sdk": "^1.29.0"`, then `pnpm install`.

- [ ] **Step 2: Implement mcp.ts**

`packages/daemon/src/mcp.ts`:

```ts
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { ApiClient } from "./client.js";
import type { McpCaller } from "./mcp-tools.js";
import {
	mcpEnqueueTask,
	mcpListTaskDefinitions,
	mcpListTasks,
	mcpRunTaskDefinition,
} from "./mcp-tools.js";
import { socketPath, statePath } from "./paths.js";

export function defaultCaller(): McpCaller {
	return async () => {
		const client = new ApiClient();
		await client.connect(socketPath(statePath()));
		return { port: client, close: () => client.close() };
	};
}

export function createMcpServer(caller: McpCaller): McpServer {
	const server = new McpServer({ name: "queohoh", version: "0.1.0" });

	server.tool(
		"enqueue_task",
		"Enqueue an ad-hoc task into the queohoh queue. The task runs end-to-end in a worktree and commits its work. IMPORTANT: workers never see this conversation — transcribe any images, error text, or rich context into the prompt verbatim before enqueueing. Returns the created task as JSON.",
		{
			prompt: z.string().describe("Full self-contained task prompt"),
			repo: z.string().describe("Registered project name (e.g. 'platform')"),
			ref: z
				.string()
				.optional()
				.describe(
					"Target ref: pr:<N> | ticket:<ID> | worktree:<name> | temp (default: temp)",
				),
			priority: z.enum(["low", "normal", "high"]).optional(),
		},
		async (args) => mcpEnqueueTask(caller, args),
	);

	server.tool(
		"list_tasks",
		"List the current queohoh queue: all live tasks plus which task ids are actively running. Returns JSON.",
		{},
		async () => mcpListTasks(caller),
	);

	server.tool(
		"list_task_definitions",
		"List all task definitions across registered repos (name, args, whether it has discovery). Use this to find the right definition before run_task_definition. Returns JSON.",
		{},
		async () => mcpListTaskDefinitions(caller),
	);

	server.tool(
		"run_task_definition",
		"Trigger a task definition. With args, instantiates directly (e.g. pr-review 257). Without args, runs the definition's discovery command which may take a while — if the call times out, the tasks may still be created: verify with list_tasks instead of retrying. Returns created tasks as JSON.",
		{
			repo: z.string().describe("Registered project name"),
			name: z.string().describe("Definition name (e.g. 'pr-review')"),
			args: z
				.array(z.string())
				.optional()
				.describe("Positional args matching the definition's declared args"),
		},
		async (args) => mcpRunTaskDefinition(caller, args),
	);

	return server;
}

export async function runMcpStdio(): Promise<void> {
	const server = createMcpServer(defaultCaller());
	await server.connect(new StdioServerTransport());
}
```

- [ ] **Step 3: Wire CLI subcommands**

In `packages/daemon/src/cli.ts` add (imports: `runMcpStdio` from `./mcp.js`):

```ts
program
	.command("mcp")
	.description("run the MCP stdio server (register in Claude Code)")
	.action(async () => {
		await runMcpStdio();
	});

program
	.command("heartbeat")
	.description("register an interactive session heartbeat (best-effort)")
	.option("--cwd <dir>", "session working directory", process.cwd())
	.action(async (opts: { cwd: string }) => {
		const client = new ApiClient();
		try {
			await client.connect(socketPath(statePath()));
			await client.call("heartbeatInteractive", {
				cwd: opts.cwd,
				pid: process.ppid,
			});
		} catch {
			// best-effort: never break a shell hook
		} finally {
			client.close();
		}
	});
```

- [ ] **Step 4: Verify**

Run: `pnpm -F @queohoh/daemon test && pnpm -F @queohoh/daemon typecheck && pnpm -F @queohoh/core test`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(daemon): MCP stdio server and heartbeat CLI"
```

---

### Task 3: /qoo skill + install docs

**Files:**
- Create: `skills/qoo/SKILL.md`
- Create: `docs/setup.md`
- Test: none (documentation task); verify skill frontmatter is valid YAML by inspection.

**Interfaces:**
- Consumes: the MCP tools (by name).
- Produces: the user-facing skill + setup instructions.

- [ ] **Step 1: Write the skill**

`skills/qoo/SKILL.md`:

```markdown
---
name: "qoo"
description: Queue work onto the queohoh orchestrator. Describe what you want ("review Kevin's auth PR", "fix the flaky date test in platform") and this skill finds the matching task definition and queues it — or enqueues an ad-hoc task if nothing matches. Requires the queohoh MCP server.
user-invocable: true
argument-hint: "<what you want done> | status"
---

# /qoo — queue it and forget it

Turn the user's natural-language request into a queued queohoh task. The
daemon runs it end-to-end in the right worktree; the user monitors via the
queohoh TUI. Your job is ONLY to route and enqueue — never do the work
yourself.

**Input:** `$ARGUMENTS` — free text describing the work, or exactly `status`.

## Routing

- `status` (single token) → call `list_tasks`, render a compact table
  (id-suffix, status, lane, first ~60 chars of prompt), done.
- Anything else → the Enqueue procedure.

## Enqueue procedure

1. Call `list_task_definitions`.
2. Match the request against definitions by name and argument shape.
   Examples: "review PR 257 in platform" → `platform/pr-review` with
   args ["257"]; "run pr-review" (no args) → discovery mode.
   - Match on meaning, not string equality. If exactly one definition
     plausibly fits, use it.
   - If 2+ fit, ask the user to pick (one short question).
   - If none fit, fall back to `enqueue_task` (ad-hoc).
3. Extract the target:
   - Definition match → `run_task_definition(repo, name, args?)`.
   - Ad-hoc → `enqueue_task(prompt, repo, ref?, priority?)`. Derive `ref`
     when obvious: "PR 257" → `pr:257`, "JUS-1423" → `ticket:JUS-1423`,
     a named worktree → `worktree:<name>`; otherwise omit (defaults to a
     temp worktree). Derive `repo` from the definition list's repo names
     or the current directory; ask if genuinely ambiguous.
4. **Ad-hoc prompt quality:** the worker is a fresh agent that sees ONLY
   the prompt text. Transcribe into it: verbatim error messages, file
   paths, stack traces, and a faithful description of any pasted images.
   The prompt must stand alone.
5. Report back in one line: what was queued, where it will run, and that
   the TUI shows progress. If `run_task_definition` timed out, call
   `list_tasks` to check whether the tasks were created before telling
   the user anything failed.

## Rules

- Never implement the work inline — even if it looks quick. The point of
  the queue is that the user stays in flow.
- Never invent definition names — only use names returned by
  `list_task_definitions`.
- One clarifying question max; prefer sensible defaults.
```

- [ ] **Step 2: Write setup docs**

`docs/setup.md`:

```markdown
# queohoh setup

## 1. Build & install

```bash
pnpm install && pnpm -r build
# expose the CLI (pick one):
pnpm -F @queohoh/daemon link --global   # or add packages/daemon/dist/cli.js to PATH
```

## 2. Configure

`~/.config/queohoh/config.yaml` (created with comments on first daemon start):

```yaml
projects:
  - name: platform
    path: ~/workspace/platform
max_concurrent_tasks: 3
archive_after_days: 7
vars:
  github_user: you
```

Per-repo task definitions live in `<repo>/.queohoh/tasks/<name>/`
(`config.yaml` + `prompt.md`) — committed, shared by all worktrees.

## 3. Run the daemon

```bash
queohoh daemon              # foreground (first run writes a starter config)
queohoh launchd:install     # keep-alive via launchd (prints the bootstrap command)
queohoh status              # check it's up
```

## 4. Claude Code integration

```bash
# MCP server (enqueue_task / list_tasks / list_task_definitions / run_task_definition):
claude mcp add queohoh -- queohoh mcp

# /qoo skill:
ln -s "$(pwd)/skills/qoo" ~/.claude/skills/qoo
```

Interactive-session awareness (the scheduler won't run tasks in a worktree
you're actively using) — add to `~/.claude/settings.json` hooks:

```json
{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "queohoh heartbeat" }] }
    ],
    "UserPromptSubmit": [
      { "hooks": [{ "type": "command", "command": "queohoh heartbeat" }] }
    ]
  }
}
```

Heartbeats expire after 5 minutes; they're best-effort and never block.

## 5. Enqueue from anywhere

- In any Claude Code session: `/qoo review PR 257 in platform`
- Drop a well-formed task file into `~/.local/state/queohoh/tasks/` — that IS an enqueue.
```

- [ ] **Step 3: Verify & commit**

Run: `pnpm test && pnpm typecheck` (unchanged, green).

```bash
git add skills/ docs/setup.md
git commit -m "docs: /qoo skill and setup guide"
```

---

## Self-Review Notes

- **Spec coverage (Plan C scope):** MCP tools exactly as spec'd (`enqueue_task`, `list_tasks`, `list_task_definitions`, `run_task_definition`) (T1+T2); attachment-transcription convention encoded in tool description + skill rules (T2+T3); /qoo skill with deterministic-daemon/fuzzy-session split (T3); interactive heartbeat via daemon API preserving the single-writer invariant (T2); fire-and-verify guidance for discovery timeouts baked into tool description and skill (Plan B deferred finding addressed at the UX layer).
- **Type consistency:** `DaemonPort` is a structural subset of `ApiClient` (`call`), so `defaultCaller` needs no adapter.
- **Placeholder scan:** clean.
