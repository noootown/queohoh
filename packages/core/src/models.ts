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
 *    enabled: prepend the active provider's `defaultModels` entry (its chosen
 *    default from the pool), falling back to `groupHead(catalog, activeProvider)`
 *    when `defaultModels` names no model for that provider (skip if neither
 *    exists).
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
		// Inject the active provider's DEFAULT from the pool: the `defaultModels`
		// entry whose provider is the active one (the model the operator chose as
		// that provider's default), NOT the provider's most-powerful group head.
		// Fall back to the group head only when `defaultModels` names no model for
		// the active provider (conservative — keeps the task runnable).
		const injected =
			defaultModels
				.map((r) => findModel(catalog, r))
				.find((e) => e !== undefined && e.provider === activeProvider) ??
			groupHead(catalog, activeProvider);
		if (injected !== undefined) {
			ordered = [injected, ...ordered];
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

/**
 * Honor a schedule-time model stamp: map the ordered ref list to chain entries
 * WITHOUT active-provider re-head or default-model injection. Used when
 * `task.model` was captured at enqueue/cron/instantiate — a later provider
 * switch must not change what a deferred / lane-blocked task will run.
 *
 * Disabled providers are dropped (same as step 3 of `resolveModelChain`); an
 * empty remainder is an error. Unknown refs fail fast.
 */
export function resolveFrozenModelChain(
	spec: string | string[],
	catalog: CatalogEntry[],
	providers: ProviderConfig[],
): ChainResult {
	const refs = typeof spec === "string" ? [spec] : spec;
	if (refs.length === 0) {
		return {
			ok: false,
			error:
				"no runnable model: all configured models are on disabled providers",
		};
	}
	const chain: ChainEntry[] = [];
	const seen = new Set<string>();
	for (const ref of refs) {
		const entry = findModel(catalog, ref);
		if (entry === undefined) {
			return { ok: false, error: unknownModelError(catalog, ref) };
		}
		if (!isEnabled(providers, entry.provider)) continue;
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

/** Result of capturing the model a newly scheduled task will run. */
export type CaptureModelResult =
	| { ok: true; model: string | string[]; modelPinned: boolean }
	| { ok: false; error: string };

/**
 * Resolve the model stamp for a task at **schedule time** under the operator's
 * current `activeProvider`, so a later provider switch cannot re-head a task
 * that is still queued (deferred, lane-blocked, etc.).
 *
 * - Explicit pin (`pinned: true` + a single string): validated and returned as
 *   a 1-entry pin (`modelPinned: true`) — exact pick, no fallback.
 * - Otherwise: full `resolveModelChain` under `activeProvider`, then freeze the
 *   resulting refs onto `task.model` (`modelPinned: false`). The worker uses
 *   `resolveFrozenModelChain` for a non-null unpinned stamp.
 */
export function captureModelForSchedule(
	spec: string | string[] | null,
	catalog: CatalogEntry[],
	providers: ProviderConfig[],
	defaultModels: string[],
	activeProvider: string,
	opts?: { pinned?: boolean },
): CaptureModelResult {
	if (opts?.pinned === true && typeof spec === "string") {
		const pinned = resolvePinnedModel(spec, catalog, providers);
		if (!pinned.ok) return pinned;
		// Canonical ref (provider/label) so the stamp matches catalog display.
		return {
			ok: true,
			model: pinned.chain[0]!.ref,
			modelPinned: true,
		};
	}
	const resolved = resolveModelChain(
		spec,
		catalog,
		providers,
		defaultModels,
		activeProvider,
	);
	if (!resolved.ok) return resolved;
	const refs = resolved.chain.map((e) => e.ref);
	return {
		ok: true,
		model: refs.length === 1 ? refs[0]! : refs,
		// Single-string operator pick without an explicit pin still freezes as
		// a non-pinned stamp (order is fixed; head is the scheduled provider).
		// Explicit pin is handled above.
		modelPinned: false,
	};
}
