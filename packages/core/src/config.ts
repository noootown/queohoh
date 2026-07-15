import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import yaml from "js-yaml";
import { z } from "zod";
import {
	definitionExists,
	loadDefinition,
	type TaskDefinition,
} from "./definition.js";

const GlobalConfigSchema = z
	.object({
		workspace: z.string().default("~/.config/queohoh"),
		projects: z
			.array(z.object({ name: z.string().min(1), path: z.string().min(1) }))
			.default([]),
		// Per-project concurrency cap (each registered project may run up to this
		// many tasks at once; the cap is independent per project, not a shared total).
		max_concurrent_tasks: z.number().int().positive().default(5),
		archive_after_days: z.number().int().positive().default(7),
		vars: z.record(z.string(), z.string()).default({}),
		models: z.record(z.string(), z.unknown()).default({}),
		// A line of shell typed into the tmux window that `goto` opens (worktree-
		// goto and queue-goto). The `{cmd}` placeholder is substituted downstream:
		// the `claude --resume <session>` command for queue-goto, empty for
		// worktree-goto. Absent → the TUI keeps its built-in `tmux new-window`
		// behavior. NOTE: a template without `{cmd}` means queue-goto will not
		// resume Claude (nothing to substitute the resume command into).
		goto_command: z.string().optional(),
	})
	.superRefine((config, ctx) => {
		const seen = new Set<string>();
		for (const project of config.projects) {
			if (seen.has(project.name)) {
				ctx.addIssue({
					code: z.ZodIssueCode.custom,
					message: `duplicate project name: ${project.name}`,
					path: ["projects"],
				});
			}
			seen.add(project.name);
		}
	});

export interface GlobalConfig {
	workspace: string;
	projects: { name: string; path: string }[];
	/** Per-project concurrency cap — see `max_concurrent_tasks` above. */
	maxConcurrentTasks: number;
	archiveAfterDays: number;
	vars: Record<string, string>;
	models: Record<string, string>;
	/** Workspace-level override for the command `goto` runs — see the schema. */
	gotoCommand?: string;
}

function expandTilde(path: string): string {
	return path.startsWith("~/") ? join(homedir(), path.slice(2)) : path;
}

export function loadGlobalConfig(path: string): GlobalConfig {
	if (!existsSync(path)) throw new Error(`config not found: ${path}`);
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	const config = GlobalConfigSchema.parse(raw);
	// Tolerant models parse: a malformed value (non-string or nested map) is
	// skipped with a warning rather than crashing config loading — mirrors
	// loadProjectModels and keeps daemon boot resilient to a bad models block.
	const models: Record<string, string> = {};
	for (const [alias, id] of Object.entries(config.models)) {
		if (typeof id === "string" && id.length > 0) {
			models[alias] = id;
		} else {
			console.warn(`config.yaml models.${alias}: not a string, skipping`);
		}
	}
	return {
		workspace: expandTilde(config.workspace),
		projects: config.projects.map((p) => ({
			name: p.name,
			path: expandTilde(p.path),
		})),
		maxConcurrentTasks: config.max_concurrent_tasks,
		archiveAfterDays: config.archive_after_days,
		vars: config.vars,
		models,
		gotoCommand: config.goto_command,
	};
}

export function projectWorkspaceDir(
	config: GlobalConfig,
	projectName: string,
): string {
	return join(config.workspace, projectName);
}

/**
 * Conventional directory for cross-project (global) task definitions:
 * `<workspace>/global`. Its `tasks/<name>/` folders share the project format and
 * appear under every project (a project-local name of the same name shadows it).
 */
export function globalWorkspaceDir(config: GlobalConfig): string {
	return join(config.workspace, "global");
}

/**
 * Load a definition for `repo` by name, checking the project's own tasks dir
 * first and falling back to the global tasks dir. `repo` stays the target
 * project on the returned definition (so worktree/vars resolve against it),
 * regardless of which directory supplied the config. When the name is absent
 * from both, the project-dir load is attempted so its ENOENT error surfaces.
 */
export function resolveDefinition(
	config: GlobalConfig,
	repo: string,
	name: string,
): TaskDefinition {
	const projectDir = projectWorkspaceDir(config, repo);
	if (definitionExists(projectDir, name)) {
		return loadDefinition(projectDir, repo, name);
	}
	const globalDir = globalWorkspaceDir(config);
	if (definitionExists(globalDir, name)) {
		return loadDefinition(globalDir, repo, name);
	}
	return loadDefinition(projectDir, repo, name);
}

export function loadProjectVars(projectDir: string): Record<string, string> {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return {};
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
		throw new Error(`vars.yaml is not a mapping: ${path}`);
	}
	const vars: Record<string, string> = {};
	for (const [key, value] of Object.entries(raw)) {
		if (key === "models") continue; // reserved: read by loadProjectModels
		if (key === "github_id") continue; // reserved: read by loadProjectGithubId
		if (key === "default_model") continue; // reserved: read by loadProjectDefaultModel
		if (key === "protected_worktrees") continue; // reserved: read by loadProjectProtectedWorktrees
		if (key === "default_branch") continue; // reserved: read by loadProjectDefaultBranch
		if (value !== null && typeof value === "object") {
			throw new Error(`non-scalar var: ${key}`);
		}
		vars[key] = String(value);
	}
	return vars;
}

/** The project's `models:` alias overrides from vars.yaml. Tolerant: absent
 * file, absent key, or a non-map value all yield {} (a bad block must never
 * take down config loading — it only disables the override). Non-string
 * values are skipped. */
export function loadProjectModels(projectDir: string): Record<string, string> {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return {};
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return {};
	const block = (raw as Record<string, unknown>).models;
	if (block === null || typeof block !== "object" || Array.isArray(block))
		return {};
	const out: Record<string, string> = {};
	for (const [alias, id] of Object.entries(block)) {
		if (typeof id === "string" && id.length > 0) out[alias] = id;
	}
	return out;
}

/** The project's optional `github_id` from vars.yaml — the author identity used
 * by the TUI to sort the operator's own worktrees first. Tolerant like
 * loadProjectModels: absent file, absent key, a non-string, or an empty string
 * all yield undefined and it never throws, so a bad value only disables the
 * "mine-first" sort rather than wedging config loading. */
export function loadProjectGithubId(projectDir: string): string | undefined {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return undefined;
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw))
		return undefined;
	const value = (raw as Record<string, unknown>).github_id;
	return typeof value === "string" && value.length > 0 ? value : undefined;
}

/** The project's optional `default_model` from vars.yaml — the model an ad-hoc /
 * enqueue run uses when neither the task nor a definition sets one, and the value
 * the TUI launcher preselects in its model dropdown. An alias (e.g. `opus`) or a
 * full model id; resolved through the alias table downstream. Tolerant like
 * loadProjectGithubId: absent file, absent key, a non-string, or an empty string
 * all yield undefined (callers fall back to the built-in `opus` default), so a
 * bad value never wedges config loading. */
export function loadProjectDefaultModel(
	projectDir: string,
): string | undefined {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return undefined;
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw))
		return undefined;
	const value = (raw as Record<string, unknown>).default_model;
	return typeof value === "string" && value.length > 0 ? value : undefined;
}

/** The project's optional `protected_worktrees` from vars.yaml — worktree names
 * that queohoh must never delete (on top of the always-protected main checkout).
 * Tolerant like loadProjectModels/loadProjectGithubId: absent file, absent key,
 * or a non-list value all yield [], and within a list any non-string or empty
 * entry is skipped. It never throws, so a malformed value only disables the
 * extra protections (the main checkout stays protected via path-equality) rather
 * than wedging config loading or snapshot generation. */
/** The project's optional `default_branch` from vars.yaml — the branch the
 * worktree "merged back" marker compares against. Falls back to `main` when the
 * file/key is absent or malformed (tolerant like the other loaders: a bad value
 * only mis-targets the marker, never wedges config loading). */
export function loadProjectDefaultBranch(projectDir: string): string {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return "main";
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw))
		return "main";
	const value = (raw as Record<string, unknown>).default_branch;
	return typeof value === "string" && value.length > 0 ? value : "main";
}

export function loadProjectProtectedWorktrees(projectDir: string): string[] {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return [];
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return [];
	const value = (raw as Record<string, unknown>).protected_worktrees;
	if (!Array.isArray(value)) return [];
	return value.filter(
		(v): v is string => typeof v === "string" && v.length > 0,
	);
}
