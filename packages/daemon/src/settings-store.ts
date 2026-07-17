import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import type { ProviderConfig } from "@queohoh/core";
import { settingsPath } from "./paths.js";

/**
 * The precedence-first ENABLED provider from the effective provider table
 * (already in fallback order — built-in ⊕ config.yaml `providers:`). Falls
 * back to the first provider by name, then the literal `"claude"`, so a table
 * with everything disabled (or an empty one) still yields a non-empty string
 * the chain resolver can head onto.
 */
export function firstEnabledProvider(providers: ProviderConfig[]): string {
	// Invariant: chain resolution needs a non-empty provider string to head onto.
	// An all-disabled (or empty) table is degenerate, but we still must return
	// SOMETHING resolvable, so the literal `"claude"` is a last-resort floor —
	// never silently returns "".
	return (
		providers.find((p) => p.enabled)?.name ?? providers[0]?.name ?? "claude"
	);
}

/**
 * Persisted operator settings, at `<state>/daemon/settings.json`:
 *
 *   { "active_provider": "claude", "disabled_crons": ["platform/pr-review"] }
 *
 * - `active_provider` (design spec §4 chain resolution's `activeProvider`): the
 *   provider the operator is currently switched to.
 * - `disabled_crons`: the set of definition keys (`<repo>/<name>`) whose cron
 *   schedule the operator has PAUSED from the TUI. The def keeps its `cron:`
 *   expression on disk (untouched — the config repo is version-controlled); this
 *   runtime set is the only thing that gates whether the engine fires it. A key
 *   absent from the set means enabled (the default), so an old settings.json
 *   with no `disabled_crons` reads as "everything enabled".
 *
 * On construction (daemon (re)start = a config load) the persisted
 * `active_provider` is validated against the effective provider table: a
 * missing/corrupt file, or a value that names a disabled/unknown provider,
 * SNAPS to the precedence-first enabled provider (logged once) and is NOT
 * written back until the next explicit `setActiveProvider` — so a temporarily-
 * disabled provider re-enabled later is honored again rather than being
 * permanently overwritten. `disabled_crons` is a free-form key set with no such
 * validation: a key naming a def that no longer exists is simply inert.
 */
export class SettingsStore {
	private readonly path: string;
	private active: string;
	private disabledCrons: Set<string>;

	constructor(stateDir: string, providers: ProviderConfig[]) {
		this.path = settingsPath(stateDir);
		const persisted = this.read();
		this.active = this.snapProvider(persisted?.active ?? null, providers);
		this.disabledCrons = new Set(persisted?.disabledCrons ?? []);
	}

	activeProvider(): string {
		return this.active;
	}

	/**
	 * Validate + persist a provider switch (write-through). Throws when the
	 * provider is unknown or disabled — the API layer returns the message to the
	 * client (matching the existing error-return idiom). Returns the new value.
	 */
	setActiveProvider(provider: string, providers: ProviderConfig[]): string {
		const cfg = providers.find((p) => p.name === provider);
		if (cfg === undefined) throw new Error(`unknown provider: ${provider}`);
		if (!cfg.enabled) throw new Error(`provider disabled: ${provider}`);
		this.active = provider;
		this.write();
		return this.active;
	}

	/** True iff the definition keyed `<repo>/<name>` has its cron PAUSED. A key
	 * never toggled reads as enabled (not in the set). */
	isCronDisabled(key: string): boolean {
		return this.disabledCrons.has(key);
	}

	/**
	 * Pause (`disabled = true`) or resume (`disabled = false`) the cron for the
	 * definition keyed `<repo>/<name>` (write-through). Idempotent — toggling to
	 * the state it is already in still persists the same file. Returns the new
	 * ENABLED state (`!disabled`) so the caller can echo it back to the client.
	 */
	setCronDisabled(key: string, disabled: boolean): boolean {
		if (disabled) this.disabledCrons.add(key);
		else this.disabledCrons.delete(key);
		this.write();
		return !disabled;
	}

	private snapProvider(
		persisted: string | null,
		providers: ProviderConfig[],
	): string {
		if (persisted === null) return firstEnabledProvider(providers);
		if (providers.find((p) => p.name === persisted)?.enabled) return persisted;
		const snapped = firstEnabledProvider(providers);
		console.warn(
			`active_provider "${persisted}" is disabled/unknown; snapping to ${snapped}`,
		);
		return snapped;
	}

	private read(): { active: string | null; disabledCrons: string[] } | null {
		try {
			if (!existsSync(this.path)) return null;
			const raw: unknown = JSON.parse(readFileSync(this.path, "utf-8"));
			if (raw === null || typeof raw !== "object") return null;
			const obj = raw as {
				active_provider?: unknown;
				disabled_crons?: unknown;
			};
			const active =
				typeof obj.active_provider === "string" &&
				obj.active_provider.length > 0
					? obj.active_provider
					: null;
			const disabledCrons = Array.isArray(obj.disabled_crons)
				? obj.disabled_crons.filter(
						(k): k is string => typeof k === "string" && k.length > 0,
					)
				: [];
			return { active, disabledCrons };
		} catch {
			return null; // corrupt file → treat as unset
		}
	}

	private write(): void {
		mkdirSync(dirname(this.path), { recursive: true });
		writeFileSync(
			this.path,
			`${JSON.stringify(
				{
					active_provider: this.active,
					// Sorted for a stable on-disk order (deterministic diffs / tests).
					disabled_crons: [...this.disabledCrons].sort(),
				},
				null,
				2,
			)}\n`,
		);
	}
}
