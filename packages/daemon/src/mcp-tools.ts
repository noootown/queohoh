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
			source: "mcp",
		}),
	);
}
