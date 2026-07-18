import { describe, expect, it } from "vitest";
import {
	createGrokUsageProbe,
	parseGrokBilling,
} from "../providers/usage-grok.js";

const monthlyOk = {
	config: {
		monthlyLimit: { val: 10000 },
		used: { val: 4200 },
		billingPeriodEnd: "2026-08-01T00:00:00Z",
	},
};

const weeklyOk = {
	config: {
		currentPeriod: { type: "USAGE_PERIOD_TYPE_WEEKLY" },
		creditUsagePercent: 81,
		billingPeriodEnd: "2026-07-21T00:00:00Z",
	},
};

describe("parseGrokBilling", () => {
	it("monthly only → text and severity ok", () => {
		expect(parseGrokBilling(monthlyOk, null)).toEqual({
			text: "42% mo",
			severity: "ok",
		});
	});

	it("dual monthly+weekly → text and severity warn", () => {
		expect(parseGrokBilling(monthlyOk, weeklyOk)).toEqual({
			text: "42%/81%",
			severity: "warn",
		});
	});

	it("null on bad monthly shape", () => {
		expect(parseGrokBilling(null, null)).toBeNull();
		expect(parseGrokBilling({}, null)).toBeNull();
		expect(parseGrokBilling({ config: {} }, null)).toBeNull();
		expect(
			parseGrokBilling(
				{
					config: {
						monthlyLimit: { val: 0 },
						used: { val: 1 },
					},
				},
				null,
			),
		).toBeNull();
	});

	it("missing weekly creditUsagePercent → 0", () => {
		expect(
			parseGrokBilling(monthlyOk, {
				config: {
					currentPeriod: { type: "USAGE_PERIOD_TYPE_WEEKLY" },
				},
			}),
		).toEqual({ text: "42%/0%", severity: "ok" });
	});
});

describe("createGrokUsageProbe", () => {
	it("returns null when token missing", async () => {
		const probe = createGrokUsageProbe({
			readToken: async () => null,
			fetchImpl: async () => {
				throw new Error("should not fetch");
			},
		});
		expect(await probe.fetch()).toBeNull();
		expect(probe.provider).toBe("grok");
	});

	it("monthly ok + weekly ok → dual sample", async () => {
		const calls: string[] = [];
		const probe = createGrokUsageProbe({
			readToken: async () => "tok",
			baseUrl: "https://example.test/v1",
			fetchImpl: async (url, init) => {
				calls.push(url);
				expect(init.headers.Authorization).toBe("Bearer tok");
				expect(init.headers["x-xai-token-auth"]).toBe("xai-grok-cli");
				expect(init.headers.Accept).toBe("application/json");
				if (url.includes("format=credits")) {
					return { ok: true, status: 200, json: async () => weeklyOk };
				}
				return { ok: true, status: 200, json: async () => monthlyOk };
			},
		});
		expect(await probe.fetch()).toEqual({
			text: "42%/81%",
			severity: "warn",
		});
		expect(calls[0]).toBe("https://example.test/v1/billing");
		expect(calls[1]).toBe("https://example.test/v1/billing?format=credits");
	});

	it("weekly fail still returns monthly-only sample", async () => {
		const probe = createGrokUsageProbe({
			readToken: async () => "tok",
			fetchImpl: async (url) => {
				if (url.includes("format=credits")) {
					return { ok: false, status: 500, json: async () => ({}) };
				}
				return { ok: true, status: 200, json: async () => monthlyOk };
			},
		});
		expect(await probe.fetch()).toEqual({ text: "42% mo", severity: "ok" });
	});

	it("returns null on monthly HTTP error", async () => {
		const probe = createGrokUsageProbe({
			readToken: async () => "tok",
			fetchImpl: async () => ({
				ok: false,
				status: 401,
				json: async () => ({}),
			}),
		});
		expect(await probe.fetch()).toBeNull();
	});
});
