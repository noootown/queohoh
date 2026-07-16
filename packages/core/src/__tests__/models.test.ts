import { describe, expect, it } from "vitest";
import type { ProviderConfig } from "../config.js";
import {
	DEFAULT_MODEL_ALIASES,
	effectiveModelTable,
	resolveModel,
	resolveProviderChain,
} from "../models.js";

describe("resolveModel", () => {
	it("resolves a known alias", () => {
		expect(resolveModel("sonnet", { sonnet: "claude-sonnet-5" })).toBe(
			"claude-sonnet-5",
		);
	});
	it("passes unknown names through untouched (full ids keep working)", () => {
		expect(resolveModel("claude-fable-5", { sonnet: "x" })).toBe(
			"claude-fable-5",
		);
	});
	it("passes through on an empty table", () => {
		expect(resolveModel("opus", {})).toBe("opus");
	});
});

describe("effectiveModelTable", () => {
	it("layers defaults <- global <- project per key", () => {
		const t = effectiveModelTable(
			{ sonnet: "claude-sonnet-4-6" },
			{ opus: "claude-opus-4-7" },
		);
		expect(t.sonnet).toBe("claude-sonnet-4-6"); // global override
		expect(t.opus).toBe("claude-opus-4-7"); // project override wins
		expect(t.fable).toBe(DEFAULT_MODEL_ALIASES.fable); // default inherited
		expect(t.haiku).toBe("claude-haiku-4-5");
	});
	it("project overrides global for the same key", () => {
		const t = effectiveModelTable({ sonnet: "a" }, { sonnet: "b" });
		expect(t.sonnet).toBe("b");
	});
});

describe("resolveProviderChain", () => {
	const PROVIDERS: ProviderConfig[] = [
		{
			name: "claude",
			enabled: true,
			models: { opus: "claude-opus-4-8", sonnet: "claude-sonnet-5" },
		},
		{
			name: "grok",
			enabled: true,
			models: { opus: "grok-4.5", sonnet: "grok-composer-2.5-fast" },
		},
		{ name: "codex", enabled: false, models: { opus: "gpt-5.6-terra" } },
	];

	it("bare tier walks enabled providers in order", () => {
		expect(resolveProviderChain("opus", PROVIDERS)).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-4-8" },
				{ provider: "grok", model: "grok-4.5" },
			],
		});
	});
	it("provider/tier pins one provider, no fallback", () => {
		expect(resolveProviderChain("grok/opus", PROVIDERS)).toEqual({
			ok: true,
			chain: [{ provider: "grok", model: "grok-4.5" }],
		});
	});
	it("provider/exact-id pins an exact model", () => {
		expect(resolveProviderChain("grok/grok-4.5", PROVIDERS)).toEqual({
			ok: true,
			chain: [{ provider: "grok", model: "grok-4.5" }],
		});
	});
	it("raw id with no slash is claude-pinned", () => {
		expect(resolveProviderChain("claude-opus-4-8", PROVIDERS)).toEqual({
			ok: true,
			chain: [{ provider: "claude", model: "claude-opus-4-8" }],
		});
	});
	it("pinning a disabled provider errors", () => {
		expect(resolveProviderChain("codex/opus", PROVIDERS)).toEqual({
			ok: false,
			error: "provider disabled: codex",
		});
	});
	it("unknown provider prefix that is not a known provider passes through as a claude raw id", () => {
		// 'claude-opus-4-8' has no slash; a slashed unknown like 'foo/bar' → unknown provider
		expect(resolveProviderChain("foo/bar", PROVIDERS)).toEqual({
			ok: false,
			error: "unknown provider: foo",
		});
	});
});
