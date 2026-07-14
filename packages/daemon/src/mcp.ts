import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";
import { ApiClient } from "./client.js";
import type { McpCaller, ToolResult } from "./mcp-tools.js";
import {
	mcpEnqueueChain,
	mcpEnqueueTask,
	mcpListTaskDefinitions,
	mcpListTasks,
	mcpRunTaskDefinition,
} from "./mcp-tools.js";
import { socketPath, statePath } from "./paths.js";

// The SDK's tool handler return type carries an index signature that our
// leaner ToolResult lacks. Reconstruct a fresh object literal (exempt from the
// missing-index-signature rule) so the SDK accepts it without weakening our
// mcp-tools types.
async function toCallResult(
	pending: Promise<ToolResult>,
): Promise<CallToolResult> {
	const result = await pending;
	return { content: result.content, isError: result.isError };
}

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
		"Enqueue an ad-hoc task into the queohoh queue. The task runs end-to-end in a worktree and commits its work. Pass cwd (absolute path inside the target worktree) to target the current worktree, and resume_session_id to make the run RESUME that Claude session instead of starting fresh — resumed runs keep the full conversation context. Without resume_session_id workers never see this conversation: transcribe any images, error text, or rich context into the prompt verbatim. Returns the created task as JSON.",
		{
			prompt: z.string().describe("Task prompt (directive if resuming)"),
			repo: z
				.string()
				.optional()
				.describe("Registered project name; omit when cwd is given"),
			cwd: z
				.string()
				.optional()
				.describe(
					"Absolute path inside the target worktree; the daemon resolves repo + worktree from it",
				),
			ref: z
				.string()
				.optional()
				.describe(
					"Target ref: pr:<N> | ticket:<ID> | worktree:<name> | temp (default: temp; ignored when cwd is given)",
				),
			priority: z.enum(["low", "normal", "high"]).optional(),
			resume_session_id: z
				.string()
				.optional()
				.describe(
					"Claude session id to resume; the run continues that session's context",
				),
			model: z
				.string()
				.optional()
				.describe(
					"Model for the run (e.g. claude-fable-5); defaults to the daemon default",
				),
			timeout: z
				.string()
				.optional()
				.describe(
					"Duration like '30m' or '2h' that sets the run's hard wall-clock ceiling; defaults to the daemon default (3h). Inactivity (a wedged worker) is reaped separately by an idle timer and is not configurable here.",
				),
			verify: z
				.string()
				.optional()
				.describe(
					"Done-condition shell command run (in the worktree) AFTER the task claims success. Exit 0 → done; non-zero or timeout → the task lands 'verify-failed'. The framework owns this check — the worker cannot fake it. E.g. \"gh pr view --json labels -q '.labels[].name' | grep -qx ready-for-review\".",
				),
		},
		async (args) => toCallResult(mcpEnqueueTask(caller, args)),
	);

	server.tool(
		"enqueue_chain",
		"Enqueue an ORDERED CHAIN of tasks that run one after another in a single shared worktree — use for a multi-step request like 'do A, then B'. Each step is either a task definition ({definition, args?}) or an ad-hoc prompt ({prompt}). The chain resolves its worktree ONCE (the first step's ref); later steps land on the same worktree and never spawn their own. A step runs only if the previous one SUCCEEDED — if a step fails, needs input, or is stopped, the rest are marked 'skipped'. resume_session_id (if given) applies to the first step only; later steps are always fresh. Shares repo/cwd/ref/worktree/priority/model with enqueue_task. Returns the created tasks (head-first) as JSON.",
		{
			steps: z
				.array(
					z.object({
						definition: z
							.string()
							.optional()
							.describe(
								"Definition name in the repo (mutually exclusive with prompt)",
							),
						args: z
							.array(z.string())
							.optional()
							.describe("Positional args for the definition"),
						prompt: z
							.string()
							.optional()
							.describe("Ad-hoc prompt (mutually exclusive with definition)"),
						verify: z
							.string()
							.optional()
							.describe(
								"Per-step done-condition shell command; non-zero/timeout lands the step 'verify-failed' and skips the rest of the chain (a definition step's own verify still wins)",
							),
					}),
				)
				.min(1)
				.describe(
					"Ordered steps; each needs exactly one of definition | prompt",
				),
			repo: z
				.string()
				.optional()
				.describe("Registered project name; omit when cwd is given"),
			cwd: z
				.string()
				.optional()
				.describe(
					"Absolute path inside the target worktree; resolves repo + worktree",
				),
			ref: z
				.string()
				.optional()
				.describe(
					"Target ref for the whole chain: pr:<N> | ticket:<ID> | worktree:<name> | temp (default: temp)",
				),
			worktree: z
				.string()
				.optional()
				.describe(
					"Existing worktree name to pin the chain to (shorthand for ref worktree:<name>)",
				),
			priority: z.enum(["low", "normal", "high"]).optional(),
			resume_session_id: z
				.string()
				.optional()
				.describe("Claude session id to resume for the FIRST step only"),
			model: z
				.string()
				.optional()
				.describe(
					"Model for prompt steps (definition steps use their own model)",
				),
			timeout: z
				.string()
				.optional()
				.describe(
					"Duration like '30m' or '2h' that sets the hard wall-clock ceiling applied to EVERY step in the chain; defaults to the daemon default (3h). Inactivity (a wedged worker) is reaped separately by an idle timer and is not configurable here.",
				),
		},
		async (args) => toCallResult(mcpEnqueueChain(caller, args)),
	);

	server.tool(
		"list_tasks",
		"List the current queohoh queue: all live tasks plus which task ids are actively running. Returns JSON.",
		{},
		async () => toCallResult(mcpListTasks(caller)),
	);

	server.tool(
		"list_task_definitions",
		"List all task definitions across registered repos (name, args, whether it has discovery). Use this to find the right definition before run_task_definition. Returns JSON.",
		{},
		async () => toCallResult(mcpListTaskDefinitions(caller)),
	);

	server.tool(
		"run_task_definition",
		"Trigger a task definition. With args, instantiates directly (e.g. pr-review 257). Without args, runs the definition's discovery command which may take a while — if the call times out, the tasks may still be created: verify with list_tasks instead of retrying. Target precedence is cwd > worktree > ref > the definition's own worktree: setting; pass ref to pin the target (e.g. ref 'temp') and override a 'worktree: auto' definition that would otherwise target a PR/ticket URL found in the args. Returns created tasks as JSON.",
		{
			repo: z.string().describe("Registered project name"),
			name: z.string().describe("Definition name (e.g. 'pr-review')"),
			args: z
				.array(z.string())
				.optional()
				.describe("Positional args matching the definition's declared args"),
			cwd: z
				.string()
				.optional()
				.describe(
					"Absolute path inside the target worktree; overrides the definition's worktree and beats worktree/ref",
				),
			worktree: z
				.string()
				.optional()
				.describe(
					"Existing worktree name to run in (shorthand for ref worktree:<name>); beats ref, beaten by cwd. Ignored for a 'worktree: repo' def.",
				),
			ref: z
				.string()
				.optional()
				.describe(
					"Target ref: pr:<N> | ticket:<ID> | worktree:<name> | temp. Overrides the definition's worktree: setting (e.g. pin 'temp' over a worktree: auto def). Ignored for a location-critical 'worktree: repo' def; beaten by worktree and cwd.",
				),
			resume_session_id: z
				.string()
				.optional()
				.describe("Claude session id to resume for the created task(s)"),
		},
		async (args) => toCallResult(mcpRunTaskDefinition(caller, args)),
	);

	return server;
}

export async function runMcpStdio(): Promise<void> {
	const server = createMcpServer(defaultCaller());
	await server.connect(new StdioServerTransport());
}
