import type { UsageProbe } from "./usage.js";
import { createClaudeUsageProbe } from "./usage-claude.js";
import { createCodexUsageProbe } from "./usage-codex.js";
import { createGrokUsageProbe } from "./usage-grok.js";

/**
 * Resolve a UsageProbe for a known provider id. Unknown names → null.
 * Fresh probe each call (no shared mutable state).
 */
export function getUsageProbe(provider: string): UsageProbe | null {
	switch (provider) {
		case "claude":
			return createClaudeUsageProbe();
		case "grok":
			return createGrokUsageProbe();
		case "codex":
			return createCodexUsageProbe();
		default:
			return null;
	}
}
