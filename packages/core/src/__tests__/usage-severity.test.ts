import { describe, expect, it } from "vitest";
import { maxSeverity, severityFromPercent } from "../providers/usage.js";

describe("severityFromPercent", () => {
	it("buckets 0/69 ok, 70/89 warn, 90/100 crit", () => {
		expect(severityFromPercent(0)).toBe("ok");
		expect(severityFromPercent(69.9)).toBe("ok");
		expect(severityFromPercent(70)).toBe("warn");
		expect(severityFromPercent(89.9)).toBe("warn");
		expect(severityFromPercent(90)).toBe("crit");
		expect(severityFromPercent(100)).toBe("crit");
	});
	it("non-finite → unknown", () => {
		expect(severityFromPercent(Number.NaN)).toBe("unknown");
	});
});

describe("maxSeverity", () => {
	it("takes the worst of dual metrics", () => {
		expect(maxSeverity([10, 95])).toBe("crit");
		expect(maxSeverity([80, 10])).toBe("warn");
		expect(maxSeverity([])).toBe("unknown");
	});
});
