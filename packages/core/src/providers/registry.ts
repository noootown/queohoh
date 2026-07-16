import type { ProviderAdapter } from "./types.js";

const registry = new Map<string, ProviderAdapter>();

export function registerAdapter(a: ProviderAdapter): void {
	registry.set(a.name, a);
}

export function getAdapter(name: string): ProviderAdapter | null {
	return registry.get(name) ?? null;
}
