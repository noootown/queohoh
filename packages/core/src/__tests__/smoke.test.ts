import { describe, expect, it } from "vitest";
import { QueueStore, render, schedule } from "../index.js";

describe("public API", () => {
	it("exports the core surface", () => {
		expect(typeof render).toBe("function");
		expect(typeof schedule).toBe("function");
		expect(typeof QueueStore).toBe("function");
	});
});
