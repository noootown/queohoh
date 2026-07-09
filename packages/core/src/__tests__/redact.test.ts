import { describe, expect, it } from "vitest";
import { buildSecretMap, makeRedactor, redact } from "../redact.js";

describe("buildSecretMap", () => {
	it("collects only secret-shaped keys with values >= 8 chars", () => {
		const map = buildSecretMap({
			SHLVL: "3",
			CLAUDECODE: "1",
			LESS: "-R",
			PATH: "/usr/bin",
			GITHUB_TOKEN: "ghp_abcdef123456",
			SHORT_KEY: "ab",
		});
		expect(map.get("ghp_abcdef123456")).toBe("GITHUB_TOKEN");
		expect(map.size).toBe(1);
	});

	it("skips empty/undefined values and non-secret-named keys", () => {
		const map = buildSecretMap({
			SECRET_VALUE: "supersecretvalue",
			EMPTY: "",
			MISSING: undefined,
			HOME: "/home/someuserwithalongname",
		});
		expect(map.get("supersecretvalue")).toBe("SECRET_VALUE");
		expect(map.size).toBe(1);
	});
});

describe("redact", () => {
	it("replaces longer values first", () => {
		const secrets = new Map([
			["abc", "SHORT"],
			["abc123", "LONG"],
		]);
		expect(redact("token abc123 and abc", secrets)).toBe(
			"token [REDACTED:LONG] and [REDACTED:SHORT]",
		);
	});

	it("no-ops on empty map", () => {
		expect(redact("hello", new Map())).toBe("hello");
	});

	it("redacts JSON-escaped form of secrets with quotes/newlines", () => {
		const secret = 'ab"cd\nef';
		const secrets = new Map([[secret, "TRICKY_TOKEN"]]);
		const serialized = JSON.stringify({ x: secret });
		const redacted = redact(serialized, secrets);
		expect(redacted).toContain("[REDACTED:TRICKY_TOKEN]");
		// the JSON-escaped bytes (ab\"cd\nef) must not survive
		expect(redacted).not.toContain('ab\\"cd');
		expect(redacted).not.toContain("cd\\nef");
	});
});

describe("makeRedactor", () => {
	it("returns a bound redact function", () => {
		const r = makeRedactor(new Map([["sekrit", "KEY"]]));
		expect(r("say sekrit")).toBe("say [REDACTED:KEY]");
	});
});
