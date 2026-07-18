import {
	type CatalogEntry,
	findModel,
	groupHead,
	modelRef,
	unknownModelError,
} from "./catalog.js";
// Type-only import: models.ts must not pull in config.ts's runtime (fs/yaml
// loading) — only the ProviderConfig shape is needed here.
import type { ProviderConfig } from "./config.js";

/**
 * Model chain resolution (design spec Section 4) over the flat catalog
 * (`catalog.ts`). A task/definition names models by `provider/label` ref (or
 * a list of them, for an explicit fallback order); `resolveModelChain` turns
 * that spec into a concrete, provider-availability-filtered, active-provider-
 * first fallback chain the worker walks in order.
 */

/** One step in a model fallback chain: which provider to spawn, which
 * provider-specific model id to pass it, and the `provider/label` ref that
 * produced it (for logging/attempted-provider bookkeeping). */
export interface ChainEntry {
	provider: string;
	model: string;
	ref: string;
}

/** Result of resolving a model spec into a chain. `ok: false` means nothing
 * in the spec is runnable right now (unknown model, or every candidate's
 * provider is disabled) — the caller should fail the task fast. */
export type ChainResult =
	| { ok: true; chain: ChainEntry[] }
	| { ok: false; error: string };

function isEnabled(providers: ProviderConfig[], provider: string): boolean {
	return providers.find((p) => p.name === provider)?.enabled === true;
}

function toChainEntry(entry: CatalogEntry): ChainEntry {
	return { provider: entry.provider, model: entry.id, ref: modelRef(entry) };
}

/**
 * Resolve a model spec into an ordered fallback chain over `catalog`.
 *
 * Algorithm (design spec Section 4, implemented verbatim):
 * 1. `refs = spec === null ? defaultModels : (typeof spec === "string" ? [spec] : spec)`.
 * 2. Map each ref via `findModel`; any miss ⇒ `unknownModelError`.
 * 3. Drop entries whose provider is disabled/unknown in `providers`.
 * 4. Stable-partition: entries with `provider === activeProvider` first
 *    (keeping order), rest after.
 * 5. If no entry has `provider === activeProvider` AND that provider is
 *    enabled: prepend `groupHead(catalog, activeProvider)` (skip prepend if
 *    the group is empty).
 * 6. Dedup by `provider/id` keeping first occurrence. Empty final chain ⇒ an
 *    error.
 */
export function resolveModelChain(
	spec: string | string[] | null,
	catalog: CatalogEntry[],
	providers: ProviderConfig[],
	defaultModels: string[],
	activeProvider: string,
): ChainResult {
	const refs =
		spec === null ? defaultModels : typeof spec === "string" ? [spec] : spec;

	const entries: CatalogEntry[] = [];
	for (const ref of refs) {
		const entry = findModel(catalog, ref);
		if (entry === undefined) {
			return { ok: false, error: unknownModelError(catalog, ref) };
		}
		entries.push(entry);
	}

	const enabled = entries.filter((e) => isEnabled(providers, e.provider));
	const active = enabled.filter((e) => e.provider === activeProvider);
	const rest = enabled.filter((e) => e.provider !== activeProvider);
	let ordered = [...active, ...rest];

	if (active.length === 0 && isEnabled(providers, activeProvider)) {
		const head = groupHead(catalog, activeProvider);
		if (head !== undefined) {
			ordered = [head, ...ordered];
		}
	}

	const seen = new Set<string>();
	const chain: ChainEntry[] = [];
	for (const entry of ordered) {
		const key = `${entry.provider}/${entry.id}`;
		if (seen.has(key)) continue;
		seen.add(key);
		chain.push(toChainEntry(entry));
	}

	if (chain.length === 0) {
		return {
			ok: false,
			error:
				"no runnable model: all configured models are on disabled providers",
		};
	}

	return { ok: true, chain };
}

/**
 * Resolve an explicit TUI model pick (`task.model_pinned`) into an EXACT
 * 1-entry chain — no active-provider re-head, no fallback. Unlike
 * `resolveModelChain`, which prepends the active provider's group head when
 * `ref` names a different provider (step 5 above), a pinned pick must run
 * exactly what the operator selected in the dialog. `ok: false` when `ref` is
 * unknown or its provider is disabled — the caller fails the task fast
 * rather than silently substituting something else.
 */
export function resolvePinnedModel(
	ref: string,
	catalog: CatalogEntry[],
	providers: ProviderConfig[],
): ChainResult {
	const entry = findModel(catalog, ref);
	if (entry === undefined) {
		return { ok: false, error: unknownModelError(catalog, ref) };
	}
	if (!isEnabled(providers, entry.provider)) {
		return {
			ok: false,
			error: `pinned model ${modelRef(entry)} is on a disabled provider: ${entry.provider}`,
		};
	}
	return { ok: true, chain: [toChainEntry(entry)] };
}
