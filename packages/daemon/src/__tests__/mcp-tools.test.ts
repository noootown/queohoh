import { describe, expect, it } from "vitest";
import type { DaemonPort, McpCaller } from "../mcp-tools.js";
import {
	mcpEnqueueTask,
	mcpListTaskDefinitions,
	mcpListTasks,
	mcpRunTaskDefinition,
} from "../mcp-tools.js";

function fakeCaller(
	handler: (method: string, params?: Record<string, unknown>) => unknown,
) {
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
		const { caller, calls, closedCount } = fakeCaller(() => ({
			id: "01X",
			status: "queued",
		}));
		const result = await mcpEnqueueTask(caller, {
			prompt: "fix it",
			repo: "platform",
		});
		expect(calls).toEqual([
			{
				method: "enqueue",
				params: {
					prompt: "fix it",
					repo: "platform",
					ref: undefined,
					priority: undefined,
				},
			},
		]);
		expect(result.isError).toBeUndefined();
		expect(JSON.parse(result.content[0]?.text ?? "")).toEqual({
			id: "01X",
			status: "queued",
		});
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
			{
				repo: "platform",
				name: "pr-review",
				args: ["number"],
				hasDiscovery: true,
			},
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
				params: {
					repo: "platform",
					name: "pr-review",
					args: ["257"],
					source: "mcp",
				},
			},
		]);
		expect(JSON.parse(result.content[0]?.text ?? "")).toEqual([{ id: "01B" }]);
	});
});
