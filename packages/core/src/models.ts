// Type-only import: models.ts must not pull in config.ts's runtime (fs/yaml
// loading) — only the ProviderConfig shape is needed here.
import type { ProviderConfig } from "./config.js";

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

/** One step in a provider fallback chain: which provider to spawn and which
 * provider-specific model id to pass it. */
export interface ChainEntry {
	provider: string;
	model: string;
}

/** Result of resolving a model spec into a provider chain. `ok: false` means
 * the spec named a provider that can't be used right now (disabled or
 * unknown) — the caller should fail the task fast rather than silently
 * falling back. */
export type ChainResult =
	| { ok: true; chain: ChainEntry[] }
	| { ok: false; error: string };

/**
 * Resolve a model spec into an ordered `(provider, model)` fallback chain
 * over `providers` (already effective/ordered — see `effectiveProviders` in
 * config.ts — with the claude provider's tiers already merged with the
 * legacy `models:` table by the caller).
 *
 * Spec shapes (design spec Section 1):
 * - Bare tier (no `/`) that at least one enabled provider has: chain across
 *   every enabled provider that has that tier, in `providers` order.
 *   Providers lacking the tier are skipped. No enabled provider has it ⇒
 *   `{ ok: true, chain: [] }` (an empty, not an error — callers decide how to
 *   handle nothing being resolvable).
 * - `provider/x`: `provider` must be a known, enabled provider (unknown ⇒
 *   `unknown provider: <name>`, disabled ⇒ `provider disabled: <name>`). If
 *   `x` is one of that provider's tier keys, use its tier entry; otherwise
 *   `x` is an exact model id on that provider. Either way the chain is a
 *   single pinned entry — no fallback past a pinned provider.
 * - Anything else with no `/` (a raw id, or a bare token that isn't a tier
 *   any provider has) passes through unchanged as a claude-pinned entry —
 *   today's `resolveModel` back-compat behavior.
 */
export function resolveProviderChain(
	spec: string,
	providers: ProviderConfig[],
): ChainResult {
	const slashIndex = spec.indexOf("/");

	if (slashIndex !== -1) {
		const providerName = spec.slice(0, slashIndex);
		const rest = spec.slice(slashIndex + 1);
		const provider = providers.find((p) => p.name === providerName);
		if (!provider) {
			return { ok: false, error: `unknown provider: ${providerName}` };
		}
		if (!provider.enabled) {
			return { ok: false, error: `provider disabled: ${providerName}` };
		}
		const model = provider.models[rest] ?? rest;
		return { ok: true, chain: [{ provider: provider.name, model }] };
	}

	const hasTier = providers.some(
		(p) => p.enabled && Object.hasOwn(p.models, spec),
	);
	if (hasTier) {
		const chain: ChainEntry[] = [];
		for (const p of providers) {
			if (!p.enabled) continue;
			const model = p.models[spec];
			if (model !== undefined) chain.push({ provider: p.name, model });
		}
		return { ok: true, chain };
	}

	return { ok: true, chain: [{ provider: "claude", model: spec }] };
}
