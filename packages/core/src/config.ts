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

// A single entry in the `providers:` config block. `models` maps a tier name
// (fable/opus/sonnet/haiku, or any custom key) to a provider-specific model id.
const ProviderConfigSchema = z.object({
	name: z.string().min(1),
	enabled: z.boolean().default(true),
	bin: z.string().optional(),
	system_prompt: z.string().optional(),
	args: z.array(z.string()).optional(),
	models: z.record(z.string(), z.string()).default({}),
});

/** One agent CLI's config: enablement, spawn overrides, and its per-tier model
 * table (tier name → provider-specific model id). Fallback order across
 * providers is the array order this appears in. */
export interface ProviderConfig {
	name: string;
	enabled: boolean;
	bin?: string;
	systemPrompt?: string;
	args?: string[];
	models: Record<string, string>;
}

/** Built-in provider table (Section 7 of the design spec): claude and grok
 * enabled, codex disabled (no subscription on this machine yet). Fallback
 * order is claude, grok, codex. This is the base every `effectiveProviders`
 * call layers onto — absent `providers:` in config.yaml yields exactly this. */
export const DEFAULT_PROVIDERS: ProviderConfig[] = [
	{
		name: "claude",
		enabled: true,
		models: {
			fable: "claude-fable-5",
			opus: "claude-opus-4-8",
			sonnet: "claude-sonnet-5",
			haiku: "claude-haiku-4-5",
		},
	},
	{
		name: "grok",
		enabled: true,
		models: {
			fable: "grok-4.5",
			opus: "grok-4.5",
			sonnet: "grok-composer-2.5-fast",
			haiku: "grok-composer-2.5-fast",
		},
	},
	{
		name: "codex",
		enabled: false,
		models: {
			fable: "gpt-5.6-sol",
			opus: "gpt-5.6-terra",
			sonnet: "gpt-5.6-luna",
			haiku: "gpt-5.6-luna",
		},
	},
];

/**
 * Layer `providers:` config over the built-in defaults, then layer project
 * tier overrides on top. Three inputs, additive (never subtractive):
 *
 * 1. `DEFAULT_PROVIDERS` — always the base; every known provider name starts
 *    here even if `global` doesn't mention it (built-in ⊕ global, not a
 *    replacement).
 * 2. `global` (config.yaml `providers:`, already validated) — per-name merge:
 *    global wins on `enabled`/`bin`/`systemPrompt`/`args`; the tier table is
 *    default tiers ⊕ global tiers (global entries override, unlisted tiers
 *    keep their default). A name global introduces that isn't in the
 *    defaults becomes a new provider entry. Ordering follows `global`'s array
 *    order when `global` is given, with any default-only names appended
 *    after (still present, still fallback-eligible); otherwise the default
 *    order.
 * 3. `projectTierOverrides` (vars.yaml `providers:`, provider → tier → id) —
 *    merges into the matching provider's tier table only. Never touches
 *    `enabled`: a project cannot flip a machine-level enablement decision.
 */
export function effectiveProviders(
	global: ProviderConfig[] | undefined,
	projectTierOverrides: Record<string, Record<string, string>>,
): ProviderConfig[] {
	const defaultsByName = new Map(DEFAULT_PROVIDERS.map((p) => [p.name, p]));
	const merged = new Map<string, ProviderConfig>();
	for (const p of DEFAULT_PROVIDERS) {
		merged.set(p.name, { ...p, models: { ...p.models } });
	}

	const order: string[] =
		global && global.length > 0
			? global.map((g) => g.name)
			: DEFAULT_PROVIDERS.map((p) => p.name);

	if (global) {
		for (const g of global) {
			const base = defaultsByName.get(g.name);
			merged.set(g.name, {
				name: g.name,
				enabled: g.enabled,
				bin: g.bin ?? base?.bin,
				systemPrompt: g.systemPrompt ?? base?.systemPrompt,
				args: g.args ?? base?.args,
				models: { ...(base?.models ?? {}), ...g.models },
			});
		}
		// Default-only names the global block didn't mention stay present
		// (additive layering), appended after the global-declared order.
		for (const p of DEFAULT_PROVIDERS) {
			if (!order.includes(p.name)) order.push(p.name);
		}
	}

	for (const [name, tiers] of Object.entries(projectTierOverrides)) {
		const existing = merged.get(name);
		if (existing) {
			existing.models = { ...existing.models, ...tiers };
		}
		// A project override for a provider name that doesn't exist in the
		// merged set has nothing to attach to; silently ignored (tolerant).
	}

	return order
		.map((name) => merged.get(name))
		.filter((p): p is ProviderConfig => p !== undefined);
}

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
		// Declares which agent CLIs (claude/grok/codex/...) are enabled and their
		// per-tier model tables, in fallback order. Absent ⇒ DEFAULT_PROVIDERS.
		// Left as `unknown` here (validated separately in loadGlobalConfig via
		// ProviderConfigSchema.safeParse) so a malformed block warns and falls
		// back rather than failing the whole-config `.parse()` and wedging boot —
		// mirrors the `models:` tolerance.
		providers: z.unknown().optional(),
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
	/** Effective provider table (built-in ⊕ config.yaml `providers:`), fallback
	 * order. Project tier overrides (vars.yaml) are layered in separately where
	 * the per-run model table is built, mirroring `effectiveModelTable`. */
	providers: ProviderConfig[];
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
	// Tolerant providers parse: validated separately from the main schema (see
	// the `providers: z.unknown()` field above) so a malformed block warns and
	// falls back to DEFAULT_PROVIDERS instead of throwing out of `.parse()`
	// and taking the whole config load down with it.
	let globalProviders: ProviderConfig[] | undefined;
	if (config.providers !== undefined) {
		const parsed = z.array(ProviderConfigSchema).safeParse(config.providers);
		if (parsed.success) {
			globalProviders = parsed.data.map((p) => ({
				name: p.name,
				enabled: p.enabled,
				bin: p.bin,
				systemPrompt: p.system_prompt,
				args: p.args,
				models: p.models,
			}));
		} else {
			console.warn(
				"config.yaml providers: malformed block, falling back to built-in defaults",
			);
		}
	}
	const providers = effectiveProviders(globalProviders, {});
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
		providers,
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
		if (key === "task_retention_days") continue; // reserved: read by loadProjectTaskRetentionDays
		if (key === "providers") continue; // reserved: read by loadProjectProviderModels
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

/** The project's `providers:` tier overrides from vars.yaml — provider name →
 * tier → model id, layered on top of the effective (built-in ⊕ global)
 * provider table by `effectiveProviders`. `enabled` is not read here: a
 * project cannot flip a machine-level enablement decision. Accepts either
 * shape:
 *   providers: { grok: { models: { opus: grok-proj } } }        # map
 *   providers: [{ name: grok, models: { opus: grok-proj } }]    # list
 * Tolerant like loadProjectModels: absent file, absent key, or a malformed
 * shape all yield {}; individual malformed entries are skipped rather than
 * throwing, so a bad block only disables that entry's override. */
export function loadProjectProviderModels(
	projectDir: string,
): Record<string, Record<string, string>> {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return {};
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return {};
	const block = (raw as Record<string, unknown>).providers;
	if (block === null || typeof block !== "object") return {};

	const out: Record<string, Record<string, string>> = {};
	const collect = (name: string, models: unknown): void => {
		if (models === null || typeof models !== "object" || Array.isArray(models))
			return;
		const tiers: Record<string, string> = {};
		for (const [tier, id] of Object.entries(
			models as Record<string, unknown>,
		)) {
			if (typeof id === "string" && id.length > 0) tiers[tier] = id;
		}
		if (Object.keys(tiers).length > 0) out[name] = tiers;
	};

	if (Array.isArray(block)) {
		for (const entry of block) {
			if (entry === null || typeof entry !== "object") continue;
			const name = (entry as Record<string, unknown>).name;
			if (typeof name !== "string" || name.length === 0) continue;
			collect(name, (entry as Record<string, unknown>).models);
		}
	} else {
		for (const [name, value] of Object.entries(
			block as Record<string, unknown>,
		)) {
			if (value === null || typeof value !== "object" || Array.isArray(value))
				continue;
			collect(name, (value as Record<string, unknown>).models);
		}
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

/** vars.yaml paths whose `task_retention_days` we've already warned about, so the
 * per-tick engine sweep logs a bad value once rather than every pass. Keyed by
 * `${path}:${rawValue}` so a corrected value re-arms the warning. */
const warnedRetentionValues = new Set<string>();

/** The project's optional `task_retention_days` from vars.yaml — how many days a
 * finished (`done`/`cancelled`) task stays visible in the queue before the engine
 * auto-archives it. Returns `fallback` (the workspace-level `archive_after_days`)
 * when the file/key is absent, or when the value is not a positive integer (a
 * non-numeric, zero, negative, or fractional value logs once, then falls back).
 * Tolerant like the other loaders: a bad value only reverts to the default, never
 * wedges config loading or the archive sweep. */
export function loadProjectTaskRetentionDays(
	projectDir: string,
	fallback: number,
): number {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return fallback;
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw))
		return fallback;
	const value = (raw as Record<string, unknown>).task_retention_days;
	if (value === undefined) return fallback;
	if (typeof value !== "number" || !Number.isInteger(value) || value <= 0) {
		const warnKey = `${path}:${String(value)}`;
		if (!warnedRetentionValues.has(warnKey)) {
			warnedRetentionValues.add(warnKey);
			console.warn(
				`vars.yaml task_retention_days: not a positive integer (${String(value)}), using ${fallback}`,
			);
		}
		return fallback;
	}
	return value;
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
