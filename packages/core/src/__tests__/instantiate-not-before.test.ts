// packages/core/src/__tests__/instantiate-not-before.test.ts
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import type { TaskDefinition } from "../definition.js";
import { instantiateDefinition } from "../instantiate.js";
import { QueueStore } from "../store.js";

function minimalDef(over: Partial<TaskDefinition> = {}): TaskDefinition {
	return {
		name: "demo",
		repo: "platform",
		description: null,
		discovery: null,
		cron: null,
		args: [{ name: "target" }],
		dedup: "none",
		worktree: "repo",
		lane: null,
		preRun: null,
		postRun: null,
		verify: null,
		model: null,
		timeoutMs: 600_000,
		priority: "normal",
		onDone: "stay",
		purgeAfterDays: null,
		prompt: "run {{target}}",
		...over,
	};
}

describe("instantiateDefinition notBefore", () => {
	const dirs: string[] = [];
	afterEach(() => {
		for (const d of dirs) rmSync(d, { recursive: true, force: true });
	});

	it("stamps notBefore on created tasks when deps.notBefore is set", async () => {
		const stateDir = mkdtempSync(join(tmpdir(), "qoo-nb-"));
		dirs.push(stateDir);
		const store = new QueueStore(stateDir);
		const until = "2099-06-01T12:00:00.000Z";
		const created = await instantiateDefinition(
			minimalDef(),
			{ mode: "args", values: ["main"] },
			{
				store,
				exec: async () => ({ stdout: "", exitCode: 0 }),
				cwd: stateDir,
				source: "mcp",
				notBefore: until,
			},
		);
		expect(created).toHaveLength(1);
		const first = created[0];
		expect(first?.notBefore).toBe(until);
		expect(first && store.get(first.id)?.notBefore).toBe(until);
	});

	it("leaves notBefore null when deps.notBefore omitted", async () => {
		const stateDir = mkdtempSync(join(tmpdir(), "qoo-nb-"));
		dirs.push(stateDir);
		const store = new QueueStore(stateDir);
		const created = await instantiateDefinition(
			minimalDef(),
			{ mode: "args", values: ["x"] },
			{
				store,
				exec: async () => ({ stdout: "", exitCode: 0 }),
				cwd: stateDir,
				source: "mcp",
			},
		);
		expect(created[0]?.notBefore).toBeNull();
	});
});
