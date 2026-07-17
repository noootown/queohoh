import { describe, expect, it } from "vitest";
import { DEFAULT_PROVIDERS, effectiveProviders } from "../config.js";

describe("providers config", () => {
	it("ships claude+grok enabled, codex disabled", () => {
		const byName = Object.fromEntries(
			DEFAULT_PROVIDERS.map((p) => [p.name, p]),
		);
		// Optional chains: `byName` is a lookup over a known-fixed literal
		// (DEFAULT_PROVIDERS), so every name below is guaranteed present; this
		// only satisfies `noUncheckedIndexedAccess`, same convention used for
		// `.find(...)` results in the tests below.
		expect(byName.claude?.enabled).toBe(true);
		expect(byName.grok?.enabled).toBe(true);
		expect(byName.codex?.enabled).toBe(false);
	});

	it("global overrides merge over defaults by provider name, order = global order", () => {
		const eff = effectiveProviders([
			{ name: "grok", enabled: false, bin: "grok-cli" },
		]);
		const grok = eff.find((p) => p.name === "grok");
		expect(grok?.enabled).toBe(false); // global wins
		expect(grok?.bin).toBe("grok-cli");
		// default-only names global doesn't mention stay present (additive),
		// appended after the global-declared order.
		expect(eff.map((p) => p.name)).toEqual(["grok", "claude", "codex"]);
	});

	it("absent global config yields the built-in defaults, in built-in order", () => {
		const eff = effectiveProviders(undefined);
		expect(eff.map((p) => p.name)).toEqual(["claude", "grok", "codex"]);
		expect(eff.find((p) => p.name === "claude")?.enabled).toBe(true);
	});
});
