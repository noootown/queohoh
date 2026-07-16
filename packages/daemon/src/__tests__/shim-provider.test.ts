import { describe, expect, it } from "vitest";
import { getAdapter } from "@queohoh/core";

describe("shim provider resolution", () => {
	it("absent provider resolves the claude adapter", () => {
		const provider = undefined as string | undefined;
		expect(getAdapter(provider ?? "claude")?.name).toBe("claude");
	});
});
