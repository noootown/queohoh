/**
 * Model alias resolution (agent247-style). Definitions and tasks name models
 * by short alias ("sonnet"); the worker resolves the alias against the
 * effective per-project table just before spawning claude. Unknown names —
 * including full model ids — pass through untouched, so nothing breaks when a
 * caller already supplies a concrete id.
 */

/** Built-in defaults; global config.yaml `models:` and a project vars.yaml
 * `models:` block layer on top (later wins, merged per key). */
export const DEFAULT_MODEL_ALIASES: Record<string, string> = {
	fable: "claude-fable-5",
	sonnet: "claude-sonnet-5",
	opus: "claude-opus-4-8",
	haiku: "claude-haiku-4-5",
};

export function resolveModel(
	name: string,
	table: Record<string, string>,
): string {
	return table[name] ?? name;
}

export function effectiveModelTable(
	global: Record<string, string>,
	project: Record<string, string>,
): Record<string, string> {
	return { ...DEFAULT_MODEL_ALIASES, ...global, ...project };
}
