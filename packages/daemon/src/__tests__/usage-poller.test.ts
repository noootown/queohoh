import type { UsageProbe, UsageSample } from "@queohoh/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { UsagePoller } from "../usage-poller.js";

function deferred<T>(): {
	promise: Promise<T>;
	resolve: (value: T) => void;
	reject: (err?: unknown) => void;
} {
	let resolve!: (value: T) => void;
	let reject!: (err?: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

function makeProbe(provider: string, fetch: UsageProbe["fetch"]): UsageProbe {
	return { provider, fetch };
}

describe("UsagePoller", () => {
	beforeEach(() => {
		vi.useFakeTimers();
	});

	afterEach(() => {
		vi.useRealTimers();
	});

	it("start → fetch all enabled → snapshot fresh non-stale in order", async () => {
		const onChange = vi.fn();
		const claudeFetch = vi
			.fn()
			.mockResolvedValue({ text: "10%/20%", severity: "ok" });
		const grokFetch = vi
			.fn()
			.mockResolvedValue({ text: "42% mo", severity: "warn" });
		const poller = new UsagePoller({
			providers: () => ["claude", "grok"],
			getProbe: (p) => {
				if (p === "claude") return makeProbe("claude", claudeFetch);
				if (p === "grok") return makeProbe("grok", grokFetch);
				return null;
			},
			onChange,
			now: () => 1_000,
		});

		poller.start();
		await vi.waitFor(() => {
			expect(claudeFetch).toHaveBeenCalledTimes(1);
			expect(grokFetch).toHaveBeenCalledTimes(1);
		});
		await Promise.resolve();
		await Promise.resolve();

		expect(poller.snapshot()).toEqual([
			{
				provider: "claude",
				text: "10%/20%",
				severity: "ok",
				fetchedAt: 1_000,
				stale: false,
			},
			{
				provider: "grok",
				text: "42% mo",
				severity: "warn",
				fetchedAt: 1_000,
				stale: false,
			},
		]);
		expect(onChange).toHaveBeenCalled();
		poller.stop();
	});

	it("fail with prior cache → stale true, same text", async () => {
		const onChange = vi.fn();
		const fetch = vi
			.fn()
			.mockResolvedValueOnce({ text: "40%/50%", severity: "ok" })
			.mockResolvedValueOnce(null);
		const poller = new UsagePoller({
			providers: () => ["claude"],
			getProbe: () => makeProbe("claude", fetch),
			onChange,
			now: () => 2_000,
			intervalMs: 60_000,
		});

		poller.start();
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
		await Promise.resolve();
		expect(poller.snapshot()[0]?.stale).toBe(false);
		expect(poller.snapshot()[0]?.text).toBe("40%/50%");

		// Interval re-fetch fails → last-good published as stale.
		await vi.advanceTimersByTimeAsync(60_000);
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
		await Promise.resolve();

		expect(poller.snapshot()).toEqual([
			{
				provider: "claude",
				text: "40%/50%",
				severity: "ok",
				fetchedAt: 2_000,
				stale: true,
			},
		]);
		poller.stop();
	});

	it("fail without cache → empty snapshot for that provider", async () => {
		const onChange = vi.fn();
		const fetch = vi.fn().mockResolvedValue(null);
		const poller = new UsagePoller({
			providers: () => ["claude"],
			getProbe: () => makeProbe("claude", fetch),
			onChange,
			now: () => 3_000,
		});

		poller.start();
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
		await Promise.resolve();

		expect(poller.snapshot()).toEqual([]);
		expect(onChange).toHaveBeenCalled();
		poller.stop();
	});

	it("interval replaces prior sample with the newer fetch", async () => {
		// Sequential flights for the same provider: second success overwrites.
		const a1 = deferred<UsageSample | null>();
		const a2 = deferred<UsageSample | null>();
		let calls = 0;
		const fetch = vi.fn().mockImplementation(() => {
			calls += 1;
			return calls === 1 ? a1.promise : a2.promise;
		});
		const clocks = { n: 1_000 };
		const poller = new UsagePoller({
			providers: () => ["claude"],
			getProbe: () => makeProbe("claude", fetch),
			onChange: () => {},
			now: () => clocks.n,
			intervalMs: 60_000,
		});

		poller.start();
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
		a1.resolve({ text: "A1", severity: "ok" });
		await Promise.resolve();
		await Promise.resolve();
		expect(poller.snapshot()[0]?.text).toBe("A1");

		clocks.n = 2_000;
		await vi.advanceTimersByTimeAsync(60_000);
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
		a2.resolve({ text: "A2", severity: "warn" });
		await Promise.resolve();
		await Promise.resolve();
		expect(poller.snapshot()).toEqual([
			{
				provider: "claude",
				text: "A2",
				severity: "warn",
				fetchedAt: 2_000,
				stale: false,
			},
		]);
		poller.stop();
	});

	it("null probe → empty snapshot for that provider", async () => {
		const onChange = vi.fn();
		const poller = new UsagePoller({
			providers: () => ["codex"],
			getProbe: () => null,
			onChange,
			now: () => 1,
		});

		poller.start();
		await Promise.resolve();

		expect(poller.snapshot()).toEqual([]);
		poller.stop();
	});

	it("interval re-fetches all providers", async () => {
		const onChange = vi.fn();
		const fetch = vi
			.fn()
			.mockResolvedValueOnce({ text: "a", severity: "ok" })
			.mockResolvedValueOnce({ text: "b", severity: "warn" });
		const clock = { n: 100 };
		const poller = new UsagePoller({
			providers: () => ["claude"],
			getProbe: () => makeProbe("claude", fetch),
			onChange,
			now: () => clock.n,
			intervalMs: 60_000,
		});

		poller.start();
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
		await Promise.resolve();
		expect(poller.snapshot()[0]?.text).toBe("a");

		clock.n = 200;
		await vi.advanceTimersByTimeAsync(60_000);
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
		await Promise.resolve();

		expect(poller.snapshot()).toEqual([
			{
				provider: "claude",
				text: "b",
				severity: "warn",
				fetchedAt: 200,
				stale: false,
			},
		]);
		poller.stop();
	});

	it("coalesces in-flight fetch for same provider on interval", async () => {
		const gate = deferred<UsageSample | null>();
		const fetch = vi.fn().mockReturnValue(gate.promise);
		const poller = new UsagePoller({
			providers: () => ["claude"],
			getProbe: () => makeProbe("claude", fetch),
			onChange: () => {},
			now: () => 1,
			intervalMs: 60_000,
		});

		poller.start();
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));

		// Interval while still in flight → no second fetch.
		await vi.advanceTimersByTimeAsync(60_000);
		expect(fetch).toHaveBeenCalledTimes(1);

		gate.resolve({ text: "x", severity: "ok" });
		await Promise.resolve();
		await Promise.resolve();
		expect(poller.snapshot()[0]?.text).toBe("x");
		poller.stop();
	});

	it("polls providers independently (A hang does not block B publish)", async () => {
		const a = deferred<UsageSample | null>();
		const claudeFetch = vi.fn().mockReturnValue(a.promise);
		const grokFetch = vi
			.fn()
			.mockResolvedValue({ text: "B-ok", severity: "ok" });
		const poller = new UsagePoller({
			providers: () => ["claude", "grok"],
			getProbe: (p) => {
				if (p === "claude") return makeProbe("claude", claudeFetch);
				if (p === "grok") return makeProbe("grok", grokFetch);
				return null;
			},
			onChange: () => {},
			now: () => 5_000,
		});

		poller.start();
		await vi.waitFor(() => expect(grokFetch).toHaveBeenCalledTimes(1));
		await Promise.resolve();
		await Promise.resolve();

		// B published while A still in flight.
		expect(poller.snapshot()).toEqual([
			{
				provider: "grok",
				text: "B-ok",
				severity: "ok",
				fetchedAt: 5_000,
				stale: false,
			},
		]);

		a.resolve({ text: "A-ok", severity: "ok" });
		await Promise.resolve();
		await Promise.resolve();

		expect(poller.snapshot()).toEqual([
			{
				provider: "claude",
				text: "A-ok",
				severity: "ok",
				fetchedAt: 5_000,
				stale: false,
			},
			{
				provider: "grok",
				text: "B-ok",
				severity: "ok",
				fetchedAt: 5_000,
				stale: false,
			},
		]);
		poller.stop();
	});

	it("drops published sample when provider leaves the enabled list", async () => {
		let names = ["claude", "grok"];
		const fetch = vi
			.fn()
			.mockResolvedValue({ text: "ok", severity: "ok" });
		const poller = new UsagePoller({
			providers: () => names,
			getProbe: (p) => makeProbe(p, fetch),
			onChange: () => {},
			now: () => 1,
			intervalMs: 60_000,
		});

		poller.start();
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
		await Promise.resolve();
		await Promise.resolve();
		expect(poller.snapshot().map((u) => u.provider)).toEqual([
			"claude",
			"grok",
		]);

		names = ["claude"];
		await vi.advanceTimersByTimeAsync(60_000);
		await Promise.resolve();
		await Promise.resolve();

		expect(poller.snapshot().map((u) => u.provider)).toEqual(["claude"]);
		poller.stop();
	});

	it("stop ignores late completions", async () => {
		const gate = deferred<UsageSample | null>();
		const fetch = vi.fn().mockReturnValue(gate.promise);
		const onChange = vi.fn();
		const poller = new UsagePoller({
			providers: () => ["claude"],
			getProbe: () => makeProbe("claude", fetch),
			onChange,
			now: () => 1,
		});

		poller.start();
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
		poller.stop();

		const calls = onChange.mock.calls.length;
		gate.resolve({ text: "too-late", severity: "ok" });
		await Promise.resolve();
		await Promise.resolve();

		expect(poller.snapshot()).toEqual([]);
		expect(onChange.mock.calls.length).toBe(calls);
	});
});
