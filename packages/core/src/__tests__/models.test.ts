import { describe, expect, it } from "vitest";
import { BUILTIN_CATALOG, unknownModelError } from "../catalog.js";
import type { ProviderConfig } from "../config.js";
import { resolveModelChain, resolvePinnedModel } from "../models.js";

const PROVIDERS: ProviderConfig[] = [
	{ name: "claude", enabled: true },
	{ name: "grok", enabled: true },
	{ name: "codex", enabled: false },
];

describe("resolveModelChain", () => {
	it("null spec uses defaultModels", () => {
		expect(
			resolveModelChain(
				null,
				BUILTIN_CATALOG,
				PROVIDERS,
				["claude/claude-sonnet-5"],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-sonnet-5", ref: "claude/claude-sonnet-5" },
			],
		});
	});

	it("string spec resolves to a 1-entry chain", () => {
		expect(
			resolveModelChain(
				"claude/claude-opus-4.8",
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-4-8", ref: "claude/claude-opus-4.8" },
			],
		});
	});

	it("list spec keeps its given order (already active provider)", () => {
		expect(
			resolveModelChain(
				["claude/claude-sonnet-5", "claude/claude-haiku-4.5"],
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-sonnet-5", ref: "claude/claude-sonnet-5" },
				{ provider: "claude", model: "claude-haiku-4-5", ref: "claude/claude-haiku-4.5" },
			],
		});
	});

	it("canonicalizes a provider/id-form ref to provider/label in the chain", () => {
		// A ref naming the raw model id (not the short label) resolves via the
		// id-match fallback, and the chain entry's `ref` is the canonical
		// `provider/label` form — never the id the caller happened to type.
		expect(
			resolveModelChain(
				"claude/claude-opus-4-8",
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-4-8", ref: "claude/claude-opus-4.8" },
			],
		});
	});

	it("canonicalizes pre-versioned short family tokens to the current label", () => {
		// Workspace defs still author `claude/sonnet` / `claude/opus`; findModel's
		// family-token fallback maps them, and the chain ref is the versioned form.
		expect(
			resolveModelChain(
				["claude/sonnet", "grok/grok-4.5"],
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-sonnet-5", ref: "claude/claude-sonnet-5" },
				{ provider: "grok", model: "grok-4.5", ref: "grok/grok-4.5" },
			],
		});
		expect(
			resolveModelChain(
				null,
				BUILTIN_CATALOG,
				PROVIDERS,
				["claude/opus", "grok/grok-4.5"],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-4-8", ref: "claude/claude-opus-4.8" },
				{ provider: "grok", model: "grok-4.5", ref: "grok/grok-4.5" },
			],
		});
	});

	it("unknown ref produces the catalog's unknown-model error", () => {
		expect(
			resolveModelChain(
				"claude/nonexistent",
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"claude",
			),
		).toEqual({
			ok: false,
			error: unknownModelError(BUILTIN_CATALOG, "claude/nonexistent"),
		});
	});

	it("drops entries whose provider is disabled", () => {
		expect(
			resolveModelChain(
				["codex/gpt-5.6-sol", "claude/claude-opus-4.8"],
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-4-8", ref: "claude/claude-opus-4.8" },
			],
		});
	});

	it("stable-partitions active-provider entries first", () => {
		expect(
			resolveModelChain(
				["claude/claude-opus-4.8", "grok/grok-4.5"],
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"grok",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "grok", model: "grok-4.5", ref: "grok/grok-4.5" },
				{ provider: "claude", model: "claude-opus-4-8", ref: "claude/claude-opus-4.8" },
			],
		});
	});

	it("switch-miss injects the active provider's default_models entry, not its group head", () => {
		// default_models is the pool; claude's default is opus, but claude's
		// group head is fable (its first catalog entry). A grok-only spec under
		// active=claude must inject OPUS (the chosen default), never fable.
		expect(
			resolveModelChain(
				["grok/grok-4.5"],
				BUILTIN_CATALOG,
				PROVIDERS,
				["claude/claude-opus-4.8", "grok/grok-4.5"],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-4-8", ref: "claude/claude-opus-4.8" },
				{ provider: "grok", model: "grok-4.5", ref: "grok/grok-4.5" },
			],
		});
	});

	it("switch-miss falls back to the group head when default_models names no model for the active provider", () => {
		// default_models has only a grok entry; active=claude has no default in the
		// pool → fall back to claude's group head (fable). Conservative + runnable.
		expect(
			resolveModelChain(
				["grok/grok-4.5"],
				BUILTIN_CATALOG,
				PROVIDERS,
				["grok/grok-4.5"],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-fable-5", ref: "claude/claude-fable-5" },
				{ provider: "grok", model: "grok-4.5", ref: "grok/grok-4.5" },
			],
		});
	});

	it("switch-miss with empty default_models falls back to the group head", () => {
		// No pool at all → group-head fallback (grok's most powerful, grok-4.5).
		expect(
			resolveModelChain(
				["claude/claude-opus-4.8"],
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"grok",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "grok", model: "grok-4.5", ref: "grok/grok-4.5" },
				{ provider: "claude", model: "claude-opus-4-8", ref: "claude/claude-opus-4.8" },
			],
		});
	});

	it("switch-miss does NOT prepend when the active provider is disabled", () => {
		expect(
			resolveModelChain(
				["claude/claude-opus-4.8"],
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"codex",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-4-8", ref: "claude/claude-opus-4.8" },
			],
		});
	});

	it("dedups by provider/id, keeping the first occurrence", () => {
		expect(
			resolveModelChain(
				["claude/claude-opus-4.8", "claude/claude-opus-4.8"],
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-4-8", ref: "claude/claude-opus-4.8" },
			],
		});
	});

	it("all-disabled (and disabled active provider) yields the no-runnable-model error", () => {
		expect(
			resolveModelChain(["codex/gpt-5.6-sol"], BUILTIN_CATALOG, PROVIDERS, [], "codex"),
		).toEqual({
			ok: false,
			error:
				"no runnable model: all configured models are on disabled providers",
		});
	});
});

describe("resolvePinnedModel", () => {
	it("resolves to an exact 1-entry chain — no active-provider re-head", () => {
		// Active provider is grok, but a pinned pick names claude — unlike
		// resolveModelChain, no grok head is prepended.
		expect(
			resolvePinnedModel("claude/claude-opus-4.8", BUILTIN_CATALOG, PROVIDERS),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-4-8", ref: "claude/claude-opus-4.8" },
			],
		});
	});

	it("canonicalizes a provider/id-form ref to provider/label", () => {
		expect(
			resolvePinnedModel("claude/claude-opus-4-8", BUILTIN_CATALOG, PROVIDERS),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-4-8", ref: "claude/claude-opus-4.8" },
			],
		});
	});

	it("unknown ref fails fast with the catalog's unknown-model error", () => {
		expect(
			resolvePinnedModel("claude/nonexistent", BUILTIN_CATALOG, PROVIDERS),
		).toEqual({
			ok: false,
			error: unknownModelError(BUILTIN_CATALOG, "claude/nonexistent"),
		});
	});

	it("disabled-provider ref fails fast — no fallback to another provider", () => {
		const result = resolvePinnedModel("codex/gpt-5.6-sol", BUILTIN_CATALOG, PROVIDERS);
		expect(result.ok).toBe(false);
		if (!result.ok) {
			expect(result.error).toContain("codex/gpt-5.6-sol");
			expect(result.error).toContain("codex");
		}
	});
});
