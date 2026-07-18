import type { UsageProbe } from "./usage.js";

/**
 * Codex has no usage endpoint yet — always-null stub so the registry can
 * resolve "codex" without special-casing callers.
 */
export function createCodexUsageProbe(): UsageProbe {
	return {
		provider: "codex",
		async fetch() {
			return null;
		},
	};
}
