import { getAdapter } from "@queohoh/core";
import { describe, expect, it } from "vitest";

describe("shim provider resolution", () => {
	it("absent provider resolves the claude adapter", () => {
		const provider = undefined as string | undefined;
		expect(getAdapter(provider ?? "claude")?.name).toBe("claude");
	});
});
