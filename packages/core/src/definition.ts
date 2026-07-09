import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import yaml from "js-yaml";
import { z } from "zod";
import { parseDuration } from "./duration.js";
import type { Priority } from "./task.js";
import { PrioritySchema } from "./task.js";

const DefinitionConfigSchema = z
	.object({
		discovery: z
			.object({ command: z.string().min(1), item_key: z.string().min(1) })
			.strict()
			.optional(),
		args: z.array(z.string()).default([]),
		dedup: z.enum(["skip_seen", "retry_errored", "none"]).default("skip_seen"),
		worktree: z.string().default("temp"),
		pre_run: z.string().optional(),
		post_run: z.string().optional(),
		model: z.string().default("sonnet"),
		timeout: z.string().default("30m"),
		priority: PrioritySchema.default("normal"),
	})
	.strict();

export interface TaskDefinition {
	name: string;
	repo: string;
	discovery: { command: string; itemKey: string } | null;
	args: string[];
	dedup: "skip_seen" | "retry_errored" | "none";
	worktree: string;
	preRun: string | null;
	postRun: string | null;
	model: string;
	timeoutMs: number;
	priority: Priority;
	prompt: string;
}

function tasksDir(projectDir: string): string {
	return join(projectDir, "tasks");
}

export function loadDefinition(
	projectDir: string,
	repoName: string,
	taskName: string,
): TaskDefinition {
	const dir = join(tasksDir(projectDir), taskName);
	const raw = yaml.load(readFileSync(join(dir, "config.yaml"), "utf-8")) ?? {};
	const config = DefinitionConfigSchema.parse(raw);
	const prompt = readFileSync(join(dir, "prompt.md"), "utf-8");
	return {
		name: taskName,
		repo: repoName,
		discovery: config.discovery
			? {
					command: config.discovery.command,
					itemKey: config.discovery.item_key,
				}
			: null,
		args: config.args,
		dedup: config.dedup,
		worktree: config.worktree,
		preRun: config.pre_run ?? null,
		postRun: config.post_run ?? null,
		model: config.model,
		timeoutMs: parseDuration(config.timeout),
		priority: config.priority,
		prompt,
	};
}

export function listDefinitions(
	projectDir: string,
	repoName: string,
): TaskDefinition[] {
	const dir = tasksDir(projectDir);
	if (!existsSync(dir)) return [];
	return readdirSync(dir, { withFileTypes: true })
		.filter((entry) => entry.isDirectory())
		.map((entry) => loadDefinition(projectDir, repoName, entry.name))
		.sort((a, b) => a.name.localeCompare(b.name));
}
