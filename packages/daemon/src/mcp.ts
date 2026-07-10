import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";
import { ApiClient } from "./client.js";
import type { McpCaller, ToolResult } from "./mcp-tools.js";
import {
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
		},
		async (args) => toCallResult(mcpEnqueueTask(caller, args)),
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
		"Trigger a task definition. With args, instantiates directly (e.g. pr-review 257). Without args, runs the definition's discovery command which may take a while — if the call times out, the tasks may still be created: verify with list_tasks instead of retrying. Returns created tasks as JSON.",
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
					"Absolute path inside the target worktree; overrides the definition's worktree",
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
