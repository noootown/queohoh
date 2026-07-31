import { describe, expect, it } from "vitest";
import { BUILTIN_CATALOG, unknownModelError } from "../catalog.js";
import type { ProviderConfig } from "../config.js";
import {
	captureModelForSchedule,
	resolveFrozenModelChain,
	resolveModelChain,
	resolvePinnedModel,
} from "../models.js";

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
				"claude/claude-opus-5",
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
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
				"claude/claude-opus-5",
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
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
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
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
				["codex/gpt-5.6-sol", "claude/claude-opus-5"],
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
			],
		});
	});

	it("stable-partitions active-provider entries first", () => {
		expect(
			resolveModelChain(
				["claude/claude-opus-5", "grok/grok-4.5"],
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"grok",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "grok", model: "grok-4.5", ref: "grok/grok-4.5" },
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
			],
		});
	});

	it("authored allowlist never injects an unlisted active provider", () => {
		// mail-check is grok-only: active=claude must NOT inject claude/opus.
		// Re-head only reorders within the authored list (partition step).
		expect(
			resolveModelChain(
				["grok/grok-4.5"],
				BUILTIN_CATALOG,
				PROVIDERS,
				["claude/claude-opus-5", "grok/grok-4.5"],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "grok", model: "grok-4.5", ref: "grok/grok-4.5" },
			],
		});
		expect(
			resolveModelChain(
				["claude/claude-opus-5"],
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"grok",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
			],
		});
	});

	it("null defaults still inject active when missing from the defaults pool", () => {
		// Unstamped path only: defaults lack the active provider → inject its
		// default_models entry (opus), not group head (fable).
		expect(
			resolveModelChain(
				null,
				BUILTIN_CATALOG,
				PROVIDERS,
				["grok/grok-4.5"],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				// default_models has no claude entry → group-head fallback (fable)
				{ provider: "claude", model: "claude-fable-5", ref: "claude/claude-fable-5" },
				{ provider: "grok", model: "grok-4.5", ref: "grok/grok-4.5" },
			],
		});
		// Active's default is in the pool → inject that, not group head.
		expect(
			resolveModelChain(
				null,
				BUILTIN_CATALOG,
				PROVIDERS,
				["claude/claude-opus-5"],
				"grok",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "grok", model: "grok-4.5", ref: "grok/grok-4.5" },
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
			],
		});
	});

	it("null defaults do NOT prepend when the active provider is disabled", () => {
		expect(
			resolveModelChain(
				null,
				BUILTIN_CATALOG,
				PROVIDERS,
				["claude/claude-opus-5"],
				"codex",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
			],
		});
	});

	it("dedups by provider/id, keeping the first occurrence", () => {
		expect(
			resolveModelChain(
				["claude/claude-opus-5", "claude/claude-opus-5"],
				BUILTIN_CATALOG,
				PROVIDERS,
				[],
				"claude",
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
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
			resolvePinnedModel("claude/claude-opus-5", BUILTIN_CATALOG, PROVIDERS),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
			],
		});
	});

	it("canonicalizes a provider/id-form ref to provider/label", () => {
		expect(
			resolvePinnedModel("claude/claude-opus-5", BUILTIN_CATALOG, PROVIDERS),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
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

describe("resolveFrozenModelChain", () => {
	it("honors stamp order without re-heading onto another provider", () => {
		// Stamped claude-first while the operator may now be on grok — frozen
		// chain must keep claude first (no inject of grok default).
		expect(
			resolveFrozenModelChain(
				["claude/claude-opus-5", "grok/grok-4.5"],
				BUILTIN_CATALOG,
				PROVIDERS,
			),
		).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
				{ provider: "grok", model: "grok-4.5", ref: "grok/grok-4.5" },
			],
		});
	});

	it("drops disabled providers from the stamp", () => {
		const result = resolveFrozenModelChain(
			["codex/gpt-5.6-sol", "claude/claude-opus-5"],
			BUILTIN_CATALOG,
			PROVIDERS,
		);
		expect(result).toEqual({
			ok: true,
			chain: [
				{ provider: "claude", model: "claude-opus-5", ref: "claude/claude-opus-5" },
			],
		});
	});
});

describe("captureModelForSchedule", () => {
	const defaults = ["claude/claude-opus-5", "grok/grok-4.5"];

	it("freezes the re-headed chain under the then-active provider", () => {
		// Active=grok re-heads the default list; stamp freezes that order.
		const captured = captureModelForSchedule(
			null,
			BUILTIN_CATALOG,
			PROVIDERS,
			defaults,
			"grok",
		);
		expect(captured).toEqual({
			ok: true,
			model: ["grok/grok-4.5", "claude/claude-opus-5"],
			modelPinned: false,
		});
	});

	it("explicit pin returns a single pinned ref", () => {
		const captured = captureModelForSchedule(
			"claude/claude-opus-5",
			BUILTIN_CATALOG,
			PROVIDERS,
			defaults,
			"grok",
			{ pinned: true },
		);
		expect(captured).toEqual({
			ok: true,
			model: "claude/claude-opus-5",
			modelPinned: true,
		});
	});

	it("preserveOrder freezes an explicit list without active-provider re-head", () => {
		// TUI preferred-first: grok then claude, while active=claude must NOT
		// re-order the stamp (re-run still sees both; head stays the pick).
		const captured = captureModelForSchedule(
			["grok/grok-4.5", "claude/claude-opus-5"],
			BUILTIN_CATALOG,
			PROVIDERS,
			defaults,
			"claude",
			{ preserveOrder: true },
		);
		expect(captured).toEqual({
			ok: true,
			model: ["grok/grok-4.5", "claude/claude-opus-5"],
			modelPinned: false,
		});
	});
});
