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

	it("start → fetch active → snapshot fresh non-stale", async () => {
		const onChange = vi.fn();
		const sample: UsageSample = { text: "10%/20%", severity: "ok" };
		const fetch = vi.fn().mockResolvedValue(sample);
		const poller = new UsagePoller({
			activeProvider: () => "claude",
			getProbe: () => makeProbe("claude", fetch),
			onChange,
			now: () => 1_000,
		});

		poller.start();
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
		await Promise.resolve(); // flush then-chain after fetch resolves

		expect(poller.snapshot()).toEqual({
			provider: "claude",
			text: "10%/20%",
			severity: "ok",
			fetchedAt: 1_000,
			stale: false,
		});
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
			activeProvider: () => "claude",
			getProbe: () => makeProbe("claude", fetch),
			onChange,
			now: () => 2_000,
			intervalMs: 60_000,
		});

		poller.start();
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
		await Promise.resolve();
		expect(poller.snapshot()?.stale).toBe(false);
		expect(poller.snapshot()?.text).toBe("40%/50%");

		// Interval re-fetch fails → last-good published as stale.
		await vi.advanceTimersByTimeAsync(60_000);
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
		await Promise.resolve();

		expect(poller.snapshot()).toEqual({
			provider: "claude",
			text: "40%/50%",
			severity: "ok",
			fetchedAt: 2_000,
			stale: true,
		});
		poller.stop();
	});

	it("fail without cache → snapshot null", async () => {
		const onChange = vi.fn();
		const fetch = vi.fn().mockResolvedValue(null);
		const poller = new UsagePoller({
			activeProvider: () => "claude",
			getProbe: () => makeProbe("claude", fetch),
			onChange,
			now: () => 3_000,
		});

		poller.start();
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
		await Promise.resolve();

		expect(poller.snapshot()).toBeNull();
		expect(onChange).toHaveBeenCalled();
		poller.stop();
	});

	it("switch A→B with B cached → immediately stale B, then success → fresh B", async () => {
		let active = "claude";
		const onChange = vi.fn();
		const clocks = { n: 10_000 };
		const claudeFetch = vi
			.fn()
			.mockResolvedValue({ text: "1%/1%", severity: "ok" });
		const grokFetch = vi
			.fn()
			.mockResolvedValueOnce({ text: "g-old", severity: "warn" })
			.mockResolvedValueOnce({ text: "g-new", severity: "ok" });

		const poller = new UsagePoller({
			activeProvider: () => active,
			getProbe: (p) => {
				if (p === "claude") return makeProbe("claude", claudeFetch);
				if (p === "grok") return makeProbe("grok", grokFetch);
				return null;
			},
			onChange,
			now: () => clocks.n,
		});

		// Seed cache for both: first start as claude, then as grok.
		poller.start();
		await vi.waitFor(() => expect(claudeFetch).toHaveBeenCalledTimes(1));
		await Promise.resolve();

		active = "grok";
		poller.onActiveProviderChanged();
		await vi.waitFor(() => expect(grokFetch).toHaveBeenCalledTimes(1));
		await Promise.resolve();
		expect(poller.snapshot()).toEqual({
			provider: "grok",
			text: "g-old",
			severity: "warn",
			fetchedAt: 10_000,
			stale: false,
		});

		// Switch away to claude and back so we exercise switch-with-cache path.
		active = "claude";
		poller.onActiveProviderChanged();
		await vi.waitFor(() => expect(claudeFetch).toHaveBeenCalledTimes(2));
		await Promise.resolve();

		clocks.n = 20_000;
		const changesBefore = onChange.mock.calls.length;
		active = "grok";
		poller.onActiveProviderChanged();

		// Immediate publish from cache: stale B, before fetch completes.
		expect(poller.snapshot()).toEqual({
			provider: "grok",
			text: "g-old",
			severity: "warn",
			fetchedAt: 10_000,
			stale: true,
		});
		expect(onChange.mock.calls.length).toBeGreaterThan(changesBefore);

		await vi.waitFor(() => expect(grokFetch).toHaveBeenCalledTimes(2));
		await Promise.resolve();

		expect(poller.snapshot()).toEqual({
			provider: "grok",
			text: "g-new",
			severity: "ok",
			fetchedAt: 20_000,
			stale: false,
		});
		poller.stop();
	});

	it("late A response after switch to B is discarded", async () => {
		let active = "claude";
		const onChange = vi.fn();
		const a = deferred<UsageSample | null>();
		const b = deferred<UsageSample | null>();
		const claudeFetch = vi.fn().mockReturnValue(a.promise);
		const grokFetch = vi.fn().mockReturnValue(b.promise);

		const poller = new UsagePoller({
			activeProvider: () => active,
			getProbe: (p) => {
				if (p === "claude") return makeProbe("claude", claudeFetch);
				if (p === "grok") return makeProbe("grok", grokFetch);
				return null;
			},
			onChange,
			now: () => 5_000,
		});

		poller.start();
		await vi.waitFor(() => expect(claudeFetch).toHaveBeenCalledTimes(1));

		active = "grok";
		poller.onActiveProviderChanged();
		await vi.waitFor(() => expect(grokFetch).toHaveBeenCalledTimes(1));

		// Late A completes after switch — must not overwrite B's published value.
		a.resolve({ text: "late-A", severity: "crit" });
		await Promise.resolve();
		await Promise.resolve();

		expect(poller.snapshot()).toBeNull(); // B still in flight, no B cache

		b.resolve({ text: "B-ok", severity: "ok" });
		await Promise.resolve();
		await Promise.resolve();

		expect(poller.snapshot()).toEqual({
			provider: "grok",
			text: "B-ok",
			severity: "ok",
			fetchedAt: 5_000,
			stale: false,
		});
		// Ensure late A never appeared as published after B won.
		expect(poller.snapshot()?.text).not.toBe("late-A");
		poller.stop();
	});

	it("late A1 after A→B→A does not overwrite newer A2", async () => {
		// Regression: activeProvider()===A is not enough — generation must
		// drop the superseded A1 flight (wrong text + fetchedAt, and clearing
		// inFlightFor would break A2 coalesce).
		let active = "claude";
		const clocks = { n: 1_000 };
		const a1 = deferred<UsageSample | null>();
		const a2 = deferred<UsageSample | null>();
		const b = deferred<UsageSample | null>();
		let claudeCalls = 0;
		const claudeFetch = vi.fn().mockImplementation(() => {
			claudeCalls += 1;
			return claudeCalls === 1 ? a1.promise : a2.promise;
		});
		const grokFetch = vi.fn().mockReturnValue(b.promise);

		const poller = new UsagePoller({
			activeProvider: () => active,
			getProbe: (p) => {
				if (p === "claude") return makeProbe("claude", claudeFetch);
				if (p === "grok") return makeProbe("grok", grokFetch);
				return null;
			},
			onChange: () => {},
			now: () => clocks.n,
		});

		poller.start();
		await vi.waitFor(() => expect(claudeFetch).toHaveBeenCalledTimes(1));

		active = "grok";
		poller.onActiveProviderChanged();
		await vi.waitFor(() => expect(grokFetch).toHaveBeenCalledTimes(1));

		active = "claude";
		poller.onActiveProviderChanged();
		await vi.waitFor(() => expect(claudeFetch).toHaveBeenCalledTimes(2));

		// Superseded A1 completes while A2 still in flight — must not publish.
		clocks.n = 2_000;
		a1.resolve({ text: "A1-stale", severity: "crit" });
		await Promise.resolve();
		await Promise.resolve();
		expect(poller.snapshot()?.text).not.toBe("A1-stale");

		// A2 wins: fresh sample + its fetchedAt.
		clocks.n = 3_000;
		a2.resolve({ text: "A2-fresh", severity: "ok" });
		await Promise.resolve();
		await Promise.resolve();

		expect(poller.snapshot()).toEqual({
			provider: "claude",
			text: "A2-fresh",
			severity: "ok",
			fetchedAt: 3_000,
			stale: false,
		});

		// A1 resolving first must not have cleared A2's in-flight slot in a
		// way that left published wrong; also resolve B so nothing leaks.
		b.resolve({ text: "B", severity: "ok" });
		await Promise.resolve();
		await Promise.resolve();
		expect(poller.snapshot()?.text).toBe("A2-fresh");
		poller.stop();
	});

	it("null probe → snapshot null", async () => {
		const onChange = vi.fn();
		const poller = new UsagePoller({
			activeProvider: () => "codex",
			getProbe: () => null,
			onChange,
			now: () => 1,
		});

		poller.start();
		await Promise.resolve();

		expect(poller.snapshot()).toBeNull();
		poller.stop();
	});

	it("interval re-fetches same provider", async () => {
		const onChange = vi.fn();
		const fetch = vi
			.fn()
			.mockResolvedValueOnce({ text: "a", severity: "ok" })
			.mockResolvedValueOnce({ text: "b", severity: "warn" });
		const clock = { n: 100 };
		const poller = new UsagePoller({
			activeProvider: () => "claude",
			getProbe: () => makeProbe("claude", fetch),
			onChange,
			now: () => clock.n,
			intervalMs: 60_000,
		});

		poller.start();
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
		await Promise.resolve();
		expect(poller.snapshot()?.text).toBe("a");

		clock.n = 200;
		await vi.advanceTimersByTimeAsync(60_000);
		await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
		await Promise.resolve();

		expect(poller.snapshot()).toEqual({
			provider: "claude",
			text: "b",
			severity: "warn",
			fetchedAt: 200,
			stale: false,
		});
		poller.stop();
	});

	it("coalesces in-flight fetch for same provider on interval", async () => {
		const gate = deferred<UsageSample | null>();
		const fetch = vi.fn().mockReturnValue(gate.promise);
		const poller = new UsagePoller({
			activeProvider: () => "claude",
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
		expect(poller.snapshot()?.text).toBe("x");
		poller.stop();
	});

	it("stop ignores late completions", async () => {
		const gate = deferred<UsageSample | null>();
		const fetch = vi.fn().mockReturnValue(gate.promise);
		const onChange = vi.fn();
		const poller = new UsagePoller({
			activeProvider: () => "claude",
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

		expect(poller.snapshot()).toBeNull();
		expect(onChange.mock.calls.length).toBe(calls);
	});
});
