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
 * Persisted operator setting: which provider is currently active (design spec
 * §4 chain resolution's `activeProvider`). Lives at
 * `<state>/daemon/settings.json` as `{ "active_provider": "claude" }`.
 *
 * On construction (daemon (re)start = a config load) the persisted value is
 * validated against the effective provider table: a missing/corrupt file, or a
 * value that names a disabled/unknown provider, SNAPS to the precedence-first
 * enabled provider (logged once) and is NOT written back until the next
 * explicit `setActiveProvider` — so a temporarily-disabled provider re-enabled
 * later is honored again rather than being permanently overwritten.
 */
export class SettingsStore {
	private readonly path: string;
	private active: string;

	constructor(stateDir: string, providers: ProviderConfig[]) {
		this.path = settingsPath(stateDir);
		this.active = this.loadAndSnap(providers);
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

	private loadAndSnap(providers: ProviderConfig[]): string {
		const persisted = this.read();
		if (persisted === null) return firstEnabledProvider(providers);
		if (providers.find((p) => p.name === persisted)?.enabled) return persisted;
		const snapped = firstEnabledProvider(providers);
		console.warn(
			`active_provider "${persisted}" is disabled/unknown; snapping to ${snapped}`,
		);
		return snapped;
	}

	private read(): string | null {
		try {
			if (!existsSync(this.path)) return null;
			const raw: unknown = JSON.parse(readFileSync(this.path, "utf-8"));
			if (raw === null || typeof raw !== "object") return null;
			const v = (raw as { active_provider?: unknown }).active_provider;
			return typeof v === "string" && v.length > 0 ? v : null;
		} catch {
			return null; // corrupt file → treat as unset
		}
	}

	private write(): void {
		mkdirSync(dirname(this.path), { recursive: true });
		writeFileSync(
			this.path,
			`${JSON.stringify({ active_provider: this.active }, null, 2)}\n`,
		);
	}
}
