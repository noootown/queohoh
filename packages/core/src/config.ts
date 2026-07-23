import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import yaml from "js-yaml";
import { z } from "zod";
import { type CatalogEntry, effectiveCatalog } from "./catalog.js";
import {
	definitionExists,
	loadDefinition,
	type TaskDefinition,
} from "./definition.js";

// A single entry in the `providers:` config block. `models` is deprecated
// (model catalog + provider switch design, Section 5): per-provider tier maps
// are replaced by the `catalog:` overlay (catalog.ts). Kept in the schema
// (typed `unknown`, not validated) only so a legacy block doesn't fail
// `.safeParse()` — `loadGlobalConfig` warns when it's present and never
// carries it into the parsed `ProviderConfig`.
const ProviderConfigSchema = z.object({
	name: z.string().min(1),
	enabled: z.boolean().default(true),
	bin: z.string().optional(),
	system_prompt: z.string().optional(),
	args: z.array(z.string()).optional(),
	models: z.unknown().optional(),
});

/** One agent CLI's config: enablement and spawn overrides. Fallback order
 * across providers is the array order this appears in. Per-provider model
 * tables are gone — models live in the flat `catalog:` (catalog.ts). */
export interface ProviderConfig {
	name: string;
	enabled: boolean;
	bin?: string;
	systemPrompt?: string;
	args?: string[];
}

/** Built-in provider table (Section 7 of the design spec): claude and grok
 * enabled, codex disabled (no subscription on this machine yet). Fallback
 * order is claude, grok, codex. This is the base every `effectiveProviders`
 * call layers onto — absent `providers:` in config.yaml yields exactly this. */
export const DEFAULT_PROVIDERS: ProviderConfig[] = [
	{ name: "claude", enabled: true },
	{ name: "grok", enabled: true },
	{ name: "codex", enabled: false },
];

/**
 * Layer `providers:` config over the built-in defaults. Additive (never
 * subtractive): `DEFAULT_PROVIDERS` is always the base — every known provider
 * name starts here even if `global` doesn't mention it (built-in ⊕ global,
 * not a replacement). `global` (config.yaml `providers:`, already validated)
 * merges per-name: global wins on `enabled`/`bin`/`systemPrompt`/`args`. A
 * name global introduces that isn't in the defaults becomes a new provider
 * entry. Ordering follows `global`'s array order when `global` is given, with
 * any default-only names appended after (still present, still
 * fallback-eligible); otherwise the default order.
 */
export function effectiveProviders(
	global: ProviderConfig[] | undefined,
): ProviderConfig[] {
	const defaultsByName = new Map(DEFAULT_PROVIDERS.map((p) => [p.name, p]));
	const merged = new Map<string, ProviderConfig>();
	for (const p of DEFAULT_PROVIDERS) {
		merged.set(p.name, { ...p });
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
			});
		}
		// Default-only names the global block didn't mention stay present
		// (additive layering), appended after the global-declared order.
		for (const p of DEFAULT_PROVIDERS) {
			if (!order.includes(p.name)) order.push(p.name);
		}
	}

	return order
		.map((name) => merged.get(name))
		.filter((p): p is ProviderConfig => p !== undefined);
}

/** One entry in the config `catalog:` overlay — same shape as `CatalogEntry`,
 * validated here so a malformed overlay (wrong shape, missing field) is
 * caught by `safeParse` rather than throwing out of `effectiveCatalog`. */
const CatalogEntrySchema = z.object({
	provider: z.string().min(1),
	id: z.string().min(1),
	label: z.string().min(1),
	hidden: z.boolean().optional(),
});

/** Global config's `default_models:` fallback list when unset — the initial
 * value from the design spec (Section 2): claude's precedence-head model,
 * then grok's. */
const DEFAULT_MODELS: string[] = ["claude/claude-opus-4.8", "grok/grok-4.5"];

const GlobalConfigSchema = z
	.object({
		workspace: z.string().default("~/.config/queohoh"),
		projects: z
			.array(z.object({ name: z.string().min(1), path: z.string().min(1) }))
			.default([]),
		// Per-project concurrency cap (each registered project may run up to this
		// many tasks at once; the cap is independent per project, not a shared total).
		max_concurrent_tasks: z.number().int().positive().default(5),
		// Hard-delete terminal tasks (live or archived) after N days from
		// finished_at (fallback created). Def `purge_after_days` overrides.
		// Default 14. Legacy `archive_after_days` still accepted as fallback
		// when purge_after_days is absent (old meaning was soft-archive; we
		// only hard-purge by age now).
		purge_after_days: z.number().int().positive().optional(),
		archive_after_days: z.number().int().positive().optional(),
		vars: z.record(z.string(), z.string()).default({}),
		// Declares which agent CLIs (claude/grok/codex/...) are enabled, in
		// fallback order. Absent ⇒ DEFAULT_PROVIDERS. Left as `unknown` here
		// (validated separately in loadGlobalConfig via ProviderConfigSchema.
		// safeParse) so a malformed block warns and falls back rather than
		// failing the whole-config `.parse()` and wedging boot — mirrors the
		// `catalog:` tolerance below.
		//
		// NOTE: `goto_command` was removed — first-class TUI goto (new tmux
		// window + left|right split) replaced the workspace init-tab override.
		// A legacy yaml key is ignored by zod strip rather than reintroduced.
		providers: z.unknown().optional(),
		// Model catalog overlay (catalog.ts): add/hide/reorder entries on top of
		// BUILTIN_CATALOG. Left as `unknown` for the same reason as `providers:`
		// above — a malformed overlay (or one that collides two labels within a
		// provider) warns and falls back to the built-in catalog unchanged,
		// rather than crashing config loading.
		catalog: z.unknown().optional(),
		// Ordered fallback list a task/definition with no `model:` of its own
		// resolves against (design spec Section 2); a project's vars.yaml
		// `default_models:` overrides this per-project (loadProjectDefaultModels).
		default_models: z.array(z.string()).default(DEFAULT_MODELS),
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
	/** Hard-delete terminal tasks after this many days (global default). */
	purgeAfterDays: number;
	/** @deprecated Alias of purgeAfterDays for older call sites/tests. */
	archiveAfterDays: number;
	vars: Record<string, string>;
	/** Effective provider table (built-in ⊕ config.yaml `providers:`), fallback
	 * order. */
	providers: ProviderConfig[];
	/** Effective model catalog (BUILTIN_CATALOG ⊕ config.yaml `catalog:`, via
	 * `effectiveCatalog`), re-grouped by provider precedence. */
	catalog: CatalogEntry[];
	/** Ordered fallback model-ref list (config.yaml `default_models:`) for
	 * tasks/defs with no `model:` of their own — see `resolveModelChain`. */
	defaultModels: string[];
}

function expandTilde(path: string): string {
	return path.startsWith("~/") ? join(homedir(), path.slice(2)) : path;
}

export function loadGlobalConfig(path: string): GlobalConfig {
	if (!existsSync(path)) throw new Error(`config not found: ${path}`);
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	const config = GlobalConfigSchema.parse(raw);
	// Tolerant providers parse: validated separately from the main schema (see
	// the `providers: z.unknown()` field above) so a malformed block warns and
	// falls back to DEFAULT_PROVIDERS instead of throwing out of `.parse()`
	// and taking the whole config load down with it. `models` is deprecated
	// (Section 5 of the catalog design spec): still accepted by the schema so
	// a legacy block doesn't fail validation, but warned about per-provider and
	// never carried into the parsed `ProviderConfig`.
	let globalProviders: ProviderConfig[] | undefined;
	if (config.providers !== undefined) {
		const parsed = z.array(ProviderConfigSchema).safeParse(config.providers);
		if (parsed.success) {
			globalProviders = parsed.data.map((p) => {
				if (p.models !== undefined) {
					console.warn(
						`config.yaml providers.${p.name}.models is no longer read; use catalog: instead`,
					);
				}
				return {
					name: p.name,
					enabled: p.enabled,
					bin: p.bin,
					systemPrompt: p.system_prompt,
					args: p.args,
				};
			});
		} else {
			console.warn(
				"config.yaml providers: malformed block, falling back to built-in defaults",
			);
		}
	}
	const providers = effectiveProviders(globalProviders);

	// Tolerant catalog overlay parse: mirrors the providers tolerance above —
	// a malformed overlay (wrong shape, or one `effectiveCatalog` rejects for
	// colliding two labels within a provider) warns and falls back to the
	// built-in catalog, unmodified, instead of crashing config loading.
	let catalog: CatalogEntry[];
	if (config.catalog !== undefined) {
		const parsedCatalog = z.array(CatalogEntrySchema).safeParse(config.catalog);
		if (parsedCatalog.success) {
			const merged = effectiveCatalog(parsedCatalog.data);
			if ("error" in merged) {
				console.warn(
					`config.yaml ${merged.error}, falling back to built-in defaults`,
				);
				catalog = effectiveCatalog(undefined) as CatalogEntry[];
			} else {
				catalog = merged;
			}
		} else {
			console.warn(
				"config.yaml catalog: malformed block, falling back to built-in defaults",
			);
			catalog = effectiveCatalog(undefined) as CatalogEntry[];
		}
	} else {
		catalog = effectiveCatalog(undefined) as CatalogEntry[];
	}

	return {
		workspace: expandTilde(config.workspace),
		projects: config.projects.map((p) => ({
			name: p.name,
			path: expandTilde(p.path),
		})),
		maxConcurrentTasks: config.max_concurrent_tasks,
		purgeAfterDays:
			config.purge_after_days ?? config.archive_after_days ?? 14,
		// Keep archiveAfterDays equal for any residual readers/tests.
		archiveAfterDays:
			config.purge_after_days ?? config.archive_after_days ?? 14,
		vars: config.vars,
		providers,
		catalog,
		defaultModels: config.default_models,
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
		if (key === "models") continue; // reserved (legacy): pre-catalog tier map
		if (key === "github_id") continue; // reserved: read by loadProjectGithubId
		if (key === "default_model") continue; // reserved (legacy): pre-catalog single default
		if (key === "protected_worktrees") continue; // reserved: read by loadProjectProtectedWorktrees
		if (key === "default_branch") continue; // reserved: read by loadProjectDefaultBranch
		if (key === "task_retention_days") continue; // reserved: read by loadProjectTaskRetentionDays
		if (key === "providers") continue; // reserved (legacy): pre-catalog tier overrides
		if (key === "default_models") continue; // reserved: read by loadProjectDefaultModels
		if (value !== null && typeof value === "object") {
			throw new Error(`non-scalar var: ${key}`);
		}
		vars[key] = String(value);
	}
	return vars;
}

/** The project's optional `default_models:` list from vars.yaml — overrides
 * `GlobalConfig.defaultModels` for tasks/defs in this project with no
 * explicit `model:` of their own. Tolerant like loadProjectGithubId: absent
 * file, absent key, or a non-list value all yield undefined (callers fall
 * back to the global list); within a list any non-string or empty entry is
 * skipped. It never throws, so a bad value only disables the project
 * override rather than wedging config loading. */
export function loadProjectDefaultModels(
	projectDir: string,
): string[] | undefined {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return undefined;
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw))
		return undefined;
	const value = (raw as Record<string, unknown>).default_models;
	if (!Array.isArray(value)) return undefined;
	return value.filter(
		(v): v is string => typeof v === "string" && v.length > 0,
	);
}

/** The project's optional `github_id` from vars.yaml — the author identity used
 * by the TUI to sort the operator's own worktrees first. Tolerant like
 * loadProjectDefaultModels: absent file, absent key, a non-string, or an empty
 * string all yield undefined and it never throws, so a bad value only disables
 * the "mine-first" sort rather than wedging config loading. */
export function loadProjectGithubId(projectDir: string): string | undefined {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return undefined;
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw))
		return undefined;
	const value = (raw as Record<string, unknown>).github_id;
	return typeof value === "string" && value.length > 0 ? value : undefined;
}

/** The project's optional `protected_worktrees` from vars.yaml — worktree names
 * that queohoh must never delete (on top of the always-protected main checkout).
 * Tolerant like loadProjectDefaultModels/loadProjectGithubId: absent file, absent key,
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
