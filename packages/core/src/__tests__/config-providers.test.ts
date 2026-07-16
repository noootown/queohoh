import { describe, expect, it } from "vitest";
import { DEFAULT_PROVIDERS, effectiveProviders } from "../config.js";

describe("providers config", () => {
	it("ships claude+grok enabled, codex disabled, with tier tables", () => {
		const byName = Object.fromEntries(
			DEFAULT_PROVIDERS.map((p) => [p.name, p]),
		);
		// Non-null assertions: `byName` is a lookup over a known-fixed literal
		// (DEFAULT_PROVIDERS), so every name below is guaranteed present; this
		// only satisfies `noUncheckedIndexedAccess`, same convention as the
		// `.find(...)!` calls in the tests below.
		expect(byName.claude!.enabled).toBe(true);
		expect(byName.grok!.enabled).toBe(true);
		expect(byName.codex!.enabled).toBe(false);
		expect(byName.claude!.models.opus).toBe("claude-opus-4-8");
		expect(byName.grok!.models.sonnet).toBe("grok-composer-2.5-fast");
	});

	it("global overrides merge over defaults by provider name, order = global order", () => {
		const eff = effectiveProviders(
			[{ name: "grok", enabled: false, models: { opus: "grok-x" } }],
			{},
		);
		const grok = eff.find((p) => p.name === "grok")!;
		expect(grok.enabled).toBe(false); // global wins
		expect(grok.models.opus).toBe("grok-x"); // tier override
		expect(grok.models.sonnet).toBe("grok-composer-2.5-fast"); // default tier retained
	});

	it("project tier overrides layer on top of enablement but cannot change enabled", () => {
		const eff = effectiveProviders(undefined, { grok: { opus: "grok-proj" } });
		const grok = eff.find((p) => p.name === "grok")!;
		expect(grok.models.opus).toBe("grok-proj");
		expect(grok.enabled).toBe(true); // project can't flip enablement
	});
});
