import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	DEFAULT_PROVIDERS,
	effectiveProviders,
	loadGlobalConfig,
} from "../config.js";

describe("providers config", () => {
	it("ships every built-in provider disabled — opt-in via config", () => {
		const byName = Object.fromEntries(
			DEFAULT_PROVIDERS.map((p) => [p.name, p]),
		);
		// Optional chains: `byName` is a lookup over a known-fixed literal
		// (DEFAULT_PROVIDERS), so every name below is guaranteed present; this
		// only satisfies `noUncheckedIndexedAccess`, same convention used for
		// `.find(...)` results in the tests below.
		expect(byName.claude?.enabled).toBe(false);
		expect(byName.grok?.enabled).toBe(false);
		expect(byName.codex?.enabled).toBe(false);
		expect(DEFAULT_PROVIDERS.map((p) => p.name)).toEqual([
			"claude",
			"grok",
			"codex",
		]);
	});

	it("global overrides merge over defaults by provider name, order = global order", () => {
		const eff = effectiveProviders([
			{ name: "grok", enabled: true, bin: "grok-cli" },
		]);
		const grok = eff.find((p) => p.name === "grok");
		expect(grok?.enabled).toBe(true); // global wins
		expect(grok?.bin).toBe("grok-cli");
		// default-only names global doesn't mention stay present (additive),
		// appended after the global-declared order — still disabled.
		expect(eff.map((p) => p.name)).toEqual(["grok", "claude", "codex"]);
		expect(eff.find((p) => p.name === "claude")?.enabled).toBe(false);
		expect(eff.find((p) => p.name === "codex")?.enabled).toBe(false);
	});

	it("listing only claude enables claude and leaves the rest off", () => {
		// Mirrors a workspace that only opts into one CLI — the top-bar must
		// not invent a sibling provider the operator never enabled.
		const eff = effectiveProviders([{ name: "claude", enabled: true }]);
		expect(eff.find((p) => p.name === "claude")?.enabled).toBe(true);
		expect(eff.find((p) => p.name === "grok")?.enabled).toBe(false);
		expect(eff.find((p) => p.name === "codex")?.enabled).toBe(false);
	});

	it("absent global config yields the built-in defaults, all disabled", () => {
		const eff = effectiveProviders(undefined);
		expect(eff.map((p) => p.name)).toEqual(["claude", "grok", "codex"]);
		expect(eff.every((p) => p.enabled === false)).toBe(true);
	});

	it("loadGlobalConfig: listing a name without enabled: opts it in only", () => {
		// Schema defaults `enabled: true` on listed entries; unmentioned
		// built-ins stay disabled. Matches a workspace that only enables claude.
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-prov-optin-"));
		const path = join(dir, "config.yaml");
		writeFileSync(path, "projects: []\nproviders:\n  - name: claude\n");
		const config = loadGlobalConfig(path);
		const byName = Object.fromEntries(
			config.providers.map((p) => [p.name, p.enabled]),
		);
		expect(byName.claude).toBe(true);
		expect(byName.grok).toBe(false);
		expect(byName.codex).toBe(false);
	});
});
