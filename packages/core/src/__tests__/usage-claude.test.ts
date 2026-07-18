import { describe, expect, it } from "vitest";
import {
	createClaudeUsageProbe,
	parseClaudeUsage,
} from "../providers/usage-claude.js";

describe("parseClaudeUsage", () => {
	it("formats session/week and severity = max", () => {
		expect(
			parseClaudeUsage({
				five_hour: { utilization: 100, resets_at: "x" },
				seven_day: { utilization: 73.4, resets_at: "y" },
			}),
		).toEqual({ text: "100%/73%", severity: "crit" });
	});
	it("warn when both mid", () => {
		expect(
			parseClaudeUsage({
				five_hour: { utilization: 70 },
				seven_day: { utilization: 50 },
			}),
		).toEqual({ text: "70%/50%", severity: "warn" });
	});
	it("null on missing fields", () => {
		expect(parseClaudeUsage({})).toBeNull();
		expect(parseClaudeUsage(null)).toBeNull();
	});
});

describe("createClaudeUsageProbe", () => {
	it("returns null when token missing", async () => {
		const probe = createClaudeUsageProbe({
			readToken: async () => null,
			fetchImpl: async () => {
				throw new Error("should not fetch");
			},
		});
		expect(await probe.fetch()).toBeNull();
	});

	it("GETs oauth usage and returns parsed sample", async () => {
		const calls: string[] = [];
		const probe = createClaudeUsageProbe({
			readToken: async () => "tok",
			fetchImpl: async (url, init) => {
				calls.push(url);
				expect(init.headers.Authorization).toBe("Bearer tok");
				expect(init.headers["anthropic-beta"]).toBe("oauth-2025-04-20");
				return {
					ok: true,
					status: 200,
					json: async () => ({
						five_hour: { utilization: 12 },
						seven_day: { utilization: 34 },
					}),
				};
			},
		});
		expect(await probe.fetch()).toEqual({ text: "12%/34%", severity: "ok" });
		expect(calls[0]).toContain("/api/oauth/usage");
	});

	it("returns null on HTTP error", async () => {
		const probe = createClaudeUsageProbe({
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
