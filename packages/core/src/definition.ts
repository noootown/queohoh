import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import yaml from "js-yaml";
import { z } from "zod";
import { parseDuration } from "./duration.js";
import type { Priority } from "./task.js";
import { PrioritySchema } from "./task.js";

/**
 * A declared trigger argument. The shorthand string form (`- pr`) normalizes to
 * `{ name: "pr" }`. `default` absent means the arg is required; `options`
 * constrains the accepted values (and `default`, if given, must be one of them).
 */
export interface ArgSpec {
	name: string;
	default?: string;
	options?: string[];
	description?: string;
}

const ArgSpecSchema = z
	.object({
		name: z.string().min(1),
		default: z.string().optional(),
		options: z.array(z.string().min(1)).min(1).optional(),
		description: z.string().optional(),
	})
	.strict();

const ArgEntrySchema = z.union([z.string().min(1), ArgSpecSchema]);

const DefinitionConfigSchema = z
	.object({
		discovery: z
			.object({ command: z.string().min(1), item_key: z.string().min(1) })
			.strict()
			.optional(),
		args: z.array(ArgEntrySchema).default([]),
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
	args: ArgSpec[];
	dedup: "skip_seen" | "retry_errored" | "none";
	worktree: string;
	preRun: string | null;
	postRun: string | null;
	model: string;
	timeoutMs: number;
	priority: Priority;
	prompt: string;
}

/**
 * Normalize the raw `args` entries (strings or objects) to `ArgSpec[]` and
 * validate: names must be unique, and a `default` must be a member of `options`
 * when both are present. Throws on violation so `loadDefinition` rejects.
 */
function normalizeArgs(
	raw: z.infer<typeof DefinitionConfigSchema>["args"],
): ArgSpec[] {
	const specs: ArgSpec[] = raw.map((entry) =>
		typeof entry === "string" ? { name: entry } : entry,
	);
	const seen = new Set<string>();
	for (const spec of specs) {
		if (seen.has(spec.name)) {
			throw new Error(`duplicate arg name: ${spec.name}`);
		}
		seen.add(spec.name);
		if (
			spec.default !== undefined &&
			spec.options &&
			!spec.options.includes(spec.default)
		) {
			throw new Error(
				`arg ${spec.name}: default "${spec.default}" not in options (${spec.options.join(", ")})`,
			);
		}
	}
	return specs;
}

function tasksDir(projectDir: string): string {
	return join(projectDir, "tasks");
}

/** Whether `<projectDir>/tasks/<taskName>/config.yaml` exists on disk. */
export function definitionExists(
	projectDir: string,
	taskName: string,
): boolean {
	return existsSync(join(tasksDir(projectDir), taskName, "config.yaml"));
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
		args: normalizeArgs(config.args),
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
