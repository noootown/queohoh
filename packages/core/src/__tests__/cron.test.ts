import { describe, expect, it } from "vitest";
import { cronDue, cronMatches, parseCron } from "../cron.js";

// A local-time Date at the given wall-clock parts.
const at = (y: number, mo: number, d: number, h: number, mi: number) =>
	new Date(y, mo - 1, d, h, mi, 0, 0);
const ms = (y: number, mo: number, d: number, h: number, mi: number) =>
	at(y, mo, d, h, mi).getTime();

describe("parseCron", () => {
	it("parses star fields", () => {
		const s = parseCron("* * * * *");
		expect(s.minute.size).toBe(60);
		expect(s.hour.size).toBe(24);
		expect(s.domRestricted).toBe(false);
		expect(s.dowRestricted).toBe(false);
	});

	it("parses number, list, range, and steps", () => {
		expect([...parseCron("0 * * * *").minute]).toEqual([0]);
		expect(
			[...parseCron("1,15,30 * * * *").minute].sort((a, b) => a - b),
		).toEqual([1, 15, 30]);
		expect([...parseCron("0 9-11 * * *").hour].sort((a, b) => a - b)).toEqual([
			9, 10, 11,
		]);
		expect([...parseCron("*/15 * * * *").minute].sort((a, b) => a - b)).toEqual(
			[0, 15, 30, 45],
		);
		expect(
			[...parseCron("0-30/10 * * * *").minute].sort((a, b) => a - b),
		).toEqual([0, 10, 20, 30]);
	});

	it("normalizes weekday 7 to Sunday (0)", () => {
		expect(parseCron("0 0 * * 7").dow.has(0)).toBe(true);
	});

	it("rejects wrong field count, out-of-range, and names", () => {
		expect(() => parseCron("* * * *")).toThrow();
		expect(() => parseCron("60 * * * *")).toThrow();
		expect(() => parseCron("0 0 * JAN *")).toThrow();
	});
});

describe("cronMatches", () => {
	it("matches top of every hour", () => {
		const s = parseCron("0 * * * *");
		expect(cronMatches(s, at(2026, 7, 14, 13, 0))).toBe(true);
		expect(cronMatches(s, at(2026, 7, 14, 13, 30))).toBe(false);
	});

	it("matches a daily local time", () => {
		const s = parseCron("30 15 * * *");
		expect(cronMatches(s, at(2026, 7, 14, 15, 30))).toBe(true);
		expect(cronMatches(s, at(2026, 7, 14, 15, 31))).toBe(false);
		expect(cronMatches(s, at(2026, 7, 14, 14, 30))).toBe(false);
	});

	it("matches weekdays with a dow range", () => {
		const s = parseCron("0 9 * * 1-5"); // Mon-Fri 09:00
		expect(cronMatches(s, at(2026, 7, 13, 9, 0))).toBe(true); // Mon
		expect(cronMatches(s, at(2026, 7, 18, 9, 0))).toBe(false); // Sat
	});

	it("uses OR-semantics when both dom and dow are restricted", () => {
		const s = parseCron("0 0 1 * 1"); // the 1st OR any Monday
		expect(cronMatches(s, at(2026, 7, 1, 0, 0))).toBe(true); // 1st (a Wed)
		expect(cronMatches(s, at(2026, 7, 13, 0, 0))).toBe(true); // a Monday
		expect(cronMatches(s, at(2026, 7, 14, 0, 0))).toBe(false); // neither
	});
});

describe("cronDue", () => {
	it("is not due when no minute in the window matches", () => {
		const s = parseCron("0 * * * *"); // top of hour
		expect(cronDue(s, ms(2026, 7, 14, 13, 1), ms(2026, 7, 14, 13, 59))).toBe(
			false,
		);
	});

	it("is due when the boundary is crossed", () => {
		const s = parseCron("0 * * * *");
		expect(cronDue(s, ms(2026, 7, 14, 13, 59), ms(2026, 7, 14, 14, 0))).toBe(
			true,
		);
	});

	it("fires once when the window spans many matching slots (catch-up-once)", () => {
		const s = parseCron("0 * * * *"); // hourly
		expect(cronDue(s, ms(2026, 7, 14, 8, 0), ms(2026, 7, 14, 14, 0))).toBe(
			true,
		);
	});

	it("returns false when now <= lastChecked", () => {
		const s = parseCron("* * * * *");
		expect(cronDue(s, ms(2026, 7, 14, 14, 0), ms(2026, 7, 14, 14, 0))).toBe(
			false,
		);
	});

	it("still fires with a far-past cursor (clamped look-back)", () => {
		const s = parseCron("30 15 * * *"); // daily 15:30
		const now = ms(2026, 7, 14, 15, 31);
		const yearAgo = now - 365 * 24 * 60 * 60 * 1000;
		expect(cronDue(s, yearAgo, now)).toBe(true);
	});
});
