import { parseDuration } from "@queohoh/core";

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
		repo?: string;
		cwd?: string;
		ref?: string;
		priority?: "low" | "normal" | "high";
		resume_session_id?: string;
		model?: string;
		timeout?: string;
		verify?: string;
	},
): Promise<ToolResult> {
	return withPort(caller, (port) => {
		// Parsed here (not silently ignored on failure) so a malformed duration
		// (e.g. "30 minutes") surfaces as a clear MCP error rather than falling
		// back to the daemon default.
		const timeoutMs =
			args.timeout !== undefined ? parseDuration(args.timeout) : undefined;
		return port.call("enqueue", {
			prompt: args.prompt,
			repo: args.repo,
			cwd: args.cwd,
			ref: args.ref,
			priority: args.priority,
			resume_session_id: args.resume_session_id,
			model: args.model,
			timeout_ms: timeoutMs,
			verify: args.verify,
		});
	});
}

export function mcpEnqueueChain(
	caller: McpCaller,
	args: {
		steps: {
			definition?: string;
			args?: string[];
			prompt?: string;
			verify?: string;
		}[];
		repo?: string;
		cwd?: string;
		ref?: string;
		worktree?: string;
		priority?: "low" | "normal" | "high";
		resume_session_id?: string;
		model?: string;
		timeout?: string;
	},
): Promise<ToolResult> {
	return withPort(caller, (port) => {
		// Parsed here (not silently ignored on failure) so a malformed duration
		// surfaces as a clear MCP error; the one ceiling applies to every step.
		const timeoutMs =
			args.timeout !== undefined ? parseDuration(args.timeout) : undefined;
		return port.call("enqueue_chain", {
			steps: args.steps,
			repo: args.repo,
			cwd: args.cwd,
			ref: args.ref,
			worktree: args.worktree,
			priority: args.priority,
			resume_session_id: args.resume_session_id,
			model: args.model,
			timeout_ms: timeoutMs,
			source: "mcp",
		});
	});
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
	args: {
		repo: string;
		name: string;
		args?: string[];
		cwd?: string;
		worktree?: string;
		ref?: string;
		resume_session_id?: string;
	},
): Promise<ToolResult> {
	return withPort(caller, (port) =>
		port.call("runDefinition", {
			repo: args.repo,
			name: args.name,
			args: args.args ?? [],
			source: "mcp",
			cwd: args.cwd,
			worktree: args.worktree,
			ref: args.ref,
			resume_session_id: args.resume_session_id,
		}),
	);
}
