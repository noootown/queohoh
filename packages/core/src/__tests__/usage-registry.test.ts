import { describe, expect, it } from "vitest";
import { getUsageProbe } from "../providers/usage-registry.js";

describe("getUsageProbe", () => {
	it("resolves known providers", () => {
		expect(getUsageProbe("claude")?.provider).toBe("claude");
		expect(getUsageProbe("grok")?.provider).toBe("grok");
		expect(getUsageProbe("codex")?.provider).toBe("codex");
	});

	it("codex always null", async () => {
		const probe = getUsageProbe("codex");
		expect(probe).not.toBeNull();
		expect(await probe?.fetch()).toBeNull();
	});

	it("unknown → null", () => {
		expect(getUsageProbe("nope")).toBeNull();
	});
});
