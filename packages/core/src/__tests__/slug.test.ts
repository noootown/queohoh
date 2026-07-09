import { describe, expect, it } from "vitest";
import { qooTempName, slugify } from "../slug.js";

describe("slugify", () => {
	it("lowercases and replaces non-alphanumeric runs with a single dash", () => {
		expect(slugify("Fix Login Redirect")).toBe("fix-login-redirect");
		expect(slugify("Add  OAuth / SSO!!")).toBe("add-oauth-sso");
	});

	it("trims leading and trailing dashes", () => {
		expect(slugify("  hello world  ")).toBe("hello-world");
		expect(slugify("...boom...")).toBe("boom");
	});

	it("truncates to 24 chars preferring a whole-word boundary", () => {
		expect(slugify("implement the new authentication flow for users")).toBe(
			"implement-the-new",
		);
	});

	it("truncates a single long word without a boundary to 24 chars", () => {
		expect(slugify("supercalifragilisticexpialidocious")).toBe(
			"supercalifragilisticexpi",
		);
	});

	it("returns an empty string when nothing usable remains", () => {
		expect(slugify("")).toBe("");
		expect(slugify("!!! @@@")).toBe("");
	});
});

describe("qooTempName", () => {
	it("builds qoo-<slug>-<suffix> from the prompt", () => {
		expect(qooTempName("fix login redirect")).toMatch(
			/^qoo-fix-login-redirect-[0-9a-z]{4}$/,
		);
	});

	it("appends a 4-char suffix so similar prompts stay unique", () => {
		const a = qooTempName("same prompt");
		const b = qooTempName("same prompt");
		expect(a).toMatch(/^qoo-same-prompt-[0-9a-z]{4}$/);
		expect(b).toMatch(/^qoo-same-prompt-[0-9a-z]{4}$/);
		expect(a).not.toBe(b);
	});

	it("falls back to qoo-<ulid6> for an unusable prompt", () => {
		expect(qooTempName("")).toMatch(/^qoo-[0-9a-z]{6}$/);
		expect(qooTempName("!!!")).toMatch(/^qoo-[0-9a-z]{6}$/);
	});
});
