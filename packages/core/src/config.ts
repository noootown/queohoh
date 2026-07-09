import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import yaml from "js-yaml";
import { z } from "zod";

const GlobalConfigSchema = z
	.object({
		workspace: z.string().default("~/.config/queohoh"),
		projects: z
			.array(z.object({ name: z.string().min(1), path: z.string().min(1) }))
			.default([]),
		max_concurrent_tasks: z.number().int().positive().default(3),
		archive_after_days: z.number().int().positive().default(7),
		vars: z.record(z.string(), z.string()).default({}),
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
	maxConcurrentTasks: number;
	archiveAfterDays: number;
	vars: Record<string, string>;
}

function expandTilde(path: string): string {
	return path.startsWith("~/") ? join(homedir(), path.slice(2)) : path;
}

export function loadGlobalConfig(path: string): GlobalConfig {
	if (!existsSync(path)) throw new Error(`config not found: ${path}`);
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	const config = GlobalConfigSchema.parse(raw);
	return {
		workspace: expandTilde(config.workspace),
		projects: config.projects.map((p) => ({
			name: p.name,
			path: expandTilde(p.path),
		})),
		maxConcurrentTasks: config.max_concurrent_tasks,
		archiveAfterDays: config.archive_after_days,
		vars: config.vars,
	};
}

export function projectWorkspaceDir(
	config: GlobalConfig,
	projectName: string,
): string {
	return join(config.workspace, projectName);
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
		if (value !== null && typeof value === "object") {
			throw new Error(`non-scalar var: ${key}`);
		}
		vars[key] = String(value);
	}
	return vars;
}
