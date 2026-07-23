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
	/**
	 * How the TUI renders the arg. `worktree` → a type-or-pick combobox that
	 * resolves to a target ref; `branch` → a dropdown of the repo's branches;
	 * `text` → a multiline textarea. Absent → a single-line input (or a dropdown
	 * when `options` is given). A `type` is mutually exclusive with `options`.
	 */
	type?: "worktree" | "branch" | "text";
	default?: string;
	options?: string[];
	description?: string;
}

const ArgSpecSchema = z
	.object({
		name: z.string().min(1),
		type: z.enum(["worktree", "branch", "text"]).optional(),
		default: z.string().optional(),
		options: z.array(z.string().min(1)).min(1).optional(),
		description: z.string().optional(),
	})
	.strict();

const ArgEntrySchema = z.union([z.string().min(1), ArgSpecSchema]);

const DefinitionConfigSchema = z
	.object({
		description: z.string().min(1).optional(),
		discovery: z
			.object({ command: z.string().min(1), item_key: z.string().min(1) })
			.strict()
			.optional(),
		cron: z.string().min(1).optional(),
		args: z.array(ArgEntrySchema).default([]),
		dedup: z.enum(["skip_seen", "retry_errored", "none"]).default("skip_seen"),
		// A ref template (`temp`, `repo`, `pr:{{n}}`, `ticket:{{id}}`,
		// `worktree:{{name}}`) or the literal `auto`, which derives the ref from
		// the task's arg values at instantiate time (see resolveRef).
		worktree: z.string().default("temp"),
		// Optional scheduler-lane override. When set, every instance of this
		// definition shares one lane (`repo:<lane>`) instead of the default
		// per-worktree lane — serializing runs across different worktrees.
		// Motivating case: the autotest task always spawns a stack on
		// testing1's ports, so two instances must never run concurrently even
		// though each lives in its own PR worktree.
		lane: z.string().min(1).optional(),
		pre_run: z.string().optional(),
		post_run: z.string().optional(),
		// Done-condition command. The framework runs it after the worker claims
		// success; a non-zero exit or timeout lands the task `verify-failed`.
		// Interpolated with the same `{{var}}` context as the prompt and the
		// pre/post_run hooks. A clean-tree requirement is expressed here too
		// (e.g. `[ -z "$(git status --porcelain)" ]`) — there is no universal
		// dirty-tree check.
		verify: z.string().optional(),
		// A `provider/label` model ref, or an ordered fallback list of them (see
		// `resolveModelChain`). Optional: a definition with no `model:` resolves
		// against `default_models` like any other model-less task — so this is
		// left unset (→ null) rather than defaulted to a single alias.
		model: z
			.union([z.string().min(1), z.array(z.string().min(1)).min(1)])
			.optional(),
		timeout: z.string().default("30m"),
		priority: PrioritySchema.default("normal"),
		// After a successful run (`done`): `stay` on the live queue (default) or
		// `archive` immediately (soft dismiss — still a track record). Failures
		// always stay live until human archive or purge. Legacy `archive_on_done:
		// true` is accepted in loadDefinition as `archive`.
		on_done: z.enum(["stay", "archive"]).optional(),
		archive_on_done: z.boolean().optional(), // legacy → on_done: archive
		// Hard-delete terminal instances after N days (live or archived).
		// Overrides workspace `purge_after_days`. Clock = finished_at ?? created.
		purge_after_days: z.number().int().positive().optional(),
		// Legacy aliases (ignored once on_done / purge_after_days exist).
		task_retention_days: z.number().int().positive().optional(),
	})
	.strict();

export type OnDone = "stay" | "archive";

export interface TaskDefinition {
	name: string;
	repo: string;
	description: string | null;
	discovery: { command: string; itemKey: string } | null;
	cron: string | null;
	args: ArgSpec[];
	dedup: "skip_seen" | "retry_errored" | "none";
	worktree: string;
	/** Scheduler-lane override; null = default per-worktree lane. See the
	 * schema comment — serializes all instances of this definition. */
	lane: string | null;
	preRun: string | null;
	postRun: string | null;
	verify: string | null;
	/** Requested model(s): a single `provider/label` ref, an ordered fallback
	 * list, or null (no `model:` → resolves against `default_models`). See
	 * `resolveModelChain`. */
	model: string | string[] | null;
	timeoutMs: number;
	priority: Priority;
	/** `stay` (default) or `archive` on successful `done`. */
	onDone: OnDone;
	/** Per-def hard-delete after N days; null = workspace `purge_after_days`. */
	purgeAfterDays: number | null;
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
		if (spec.type && spec.options) {
			throw new Error(
				`arg ${spec.name}: type "${spec.type}" cannot combine with options`,
			);
		}
	}
	return specs;
}

function tasksDir(projectDir: string): string {
	return join(projectDir, "tasks");
}

/** `repo/name` keys already warned about in this process, so a definition that
 * fails to parse is logged once rather than on every enumeration (the cron
 * scheduler lists definitions every tick). */
const warnedBadDefs = new Set<string>();

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
		description: config.description ?? null,
		discovery: config.discovery
			? {
					command: config.discovery.command,
					itemKey: config.discovery.item_key,
				}
			: null,
		cron: config.cron ?? null,
		args: normalizeArgs(config.args),
		dedup: config.dedup,
		worktree: config.worktree,
		lane: config.lane ?? null,
		preRun: config.pre_run ?? null,
		postRun: config.post_run ?? null,
		verify: config.verify ?? null,
		model: config.model ?? null,
		timeoutMs: parseDuration(config.timeout),
		priority: config.priority,
		onDone:
			config.on_done ??
			(config.archive_on_done === true ? "archive" : "stay"),
		// Prefer purge_after_days; legacy task_retention_days maps to purge age
		// (old "leave live N days" is gone — age only hard-deletes now).
		purgeAfterDays:
			config.purge_after_days ?? config.task_retention_days ?? null,
		prompt,
	};
}

export function listDefinitions(
	projectDir: string,
	repoName: string,
): TaskDefinition[] {
	const dir = tasksDir(projectDir);
	if (!existsSync(dir)) return [];
	const defs: TaskDefinition[] = [];
	for (const entry of readdirSync(dir, { withFileTypes: true })) {
		if (!entry.isDirectory()) continue;
		try {
			defs.push(loadDefinition(projectDir, repoName, entry.name));
		} catch (err) {
			// A single malformed definition must not hide the rest — and for the
			// cron scheduler, which enumerates every project's defs each tick, an
			// all-or-nothing throw would silently disable ALL scheduling. Skip the
			// bad one with a once-per-process warning; the error still surfaces
			// loudly if someone resolves or runs that def by name.
			const key = `${repoName}/${entry.name}`;
			if (!warnedBadDefs.has(key)) {
				warnedBadDefs.add(key);
				console.warn(
					`skipping unparseable definition ${key}: ${err instanceof Error ? err.message : String(err)}`,
				);
			}
		}
	}
	return defs.sort((a, b) => a.name.localeCompare(b.name));
}
